//! 多路远端音频混音。
//! 每路音轨有独立的抖动缓冲与 Opus 解码器，按成员音量与总音量混合后经限幅输出。
//! 开启成员降噪时，该成员的音轨在解码后、混合前经过降噪。
//! 同时统计说话状态与接收质量。

use std::collections::{HashMap, HashSet, VecDeque};

use bytes::Bytes;

use super::denoise::{Denoise, Stage};
use super::dsp::RATE;
use super::jitter::{Jitter, Pull, Stats};

/// 音量百分比上限。
pub const MAX_VOLUME: u16 = 200;
/// 判定为说话的 RMS 阈值。
const SPEAK_RMS: f32 = 0.005;
/// 说话状态的保持时长，300 ms。
const SPEAK_HOLD: u64 = 48 * 300;
/// 单包最大样本数，120 ms。
const MAX_FRAME: usize = 5760;

pub use super::dsp::soft_limit as limit;

/// 音量百分比转换为增益。
#[must_use]
pub fn gain(percent: u16) -> f32 {
    f32::from(percent.min(MAX_VOLUME)) / 100.0
}

/// 音频解码器。
pub trait Decode: Send {
    /// 解码一个包，返回样本数。`fec` 为 true 时从该包恢复前一包。
    fn decode(&mut self, packet: &[u8], out: &mut [f32], fec: bool) -> usize;
    /// 生成丢包补偿样本，返回样本数。
    fn conceal(&mut self, out: &mut [f32]) -> usize;
}

struct Opus(opus::Decoder);

impl Decode for Opus {
    fn decode(&mut self, packet: &[u8], out: &mut [f32], fec: bool) -> usize {
        match self.0.decode_float(packet, out, fec) {
            Ok(n) => n,
            Err(e) => {
                log::debug!("Opus 解码失败：{e}");
                0
            }
        }
    }

    fn conceal(&mut self, out: &mut [f32]) -> usize {
        self.0.decode_float(&[], out, false).unwrap_or(0)
    }
}

/// 创建 48 kHz 单声道 Opus 解码器。
///
/// # Errors
///
/// libopus 初始化失败时返回错误。
pub fn opus_decoder() -> Result<Box<dyn Decode>, opus::Error> {
    Ok(Box::new(Opus(opus::Decoder::new(
        RATE,
        opus::Channels::Mono,
    )?)))
}

struct Track {
    peer: String,
    jitter: Jitter,
    decoder: Box<dyn Decode>,
    /// 已解码、待混合的样本。
    pcm: VecDeque<f32>,
    /// 最近一包的样本数，用于 FEC 与丢包补偿。
    frame: usize,
    /// 最近一次说话结束时的混音时钟。
    spoke: Option<u64>,
    /// 成员降噪，未开启时为 None。
    denoise: Option<Stage>,
}

impl Track {
    /// 解码到至少 `n` 个样本。缓冲不足时返回 false。
    fn fill(&mut self, n: usize, scratch: &mut [f32]) -> bool {
        while self.pcm.len() < n {
            let got = match self.jitter.pull() {
                Pull::Packet(p) => self.decoder.decode(&p, scratch, false),
                Pull::Lost { next: Some(p) } => {
                    // 用下一包的 FEC 数据恢复丢失的包。
                    let len = self.frame.min(scratch.len());
                    self.decoder.decode(&p, &mut scratch[..len], true)
                }
                Pull::Lost { next: None } => {
                    let len = self.frame.min(scratch.len());
                    self.decoder.conceal(&mut scratch[..len])
                }
                Pull::Wait => return false,
            };
            if got == 0 {
                continue;
            }
            self.frame = got;
            if let Some(d) = self.denoise.as_mut() {
                let mut v = scratch[..got].to_vec();
                d.process(&mut v);
                self.pcm.extend(v);
            } else {
                self.pcm.extend(&scratch[..got]);
            }
        }
        true
    }
}

/// 多路音轨混音器。
pub struct Mixer {
    /// 按 SSRC 索引的音轨。
    tracks: HashMap<u32, Track>,
    /// 成员音量，100 不记录。
    volumes: HashMap<String, u16>,
    /// 开启降噪的成员及其降噪实例来源。
    denoised: HashMap<String, Denoise>,
    /// 成员降噪的方式与故障通知来源。
    denoise_base: Denoise,
    master: u16,
    /// 已输出的样本数。
    clock: u64,
    scratch: Vec<f32>,
    make: fn() -> Option<Box<dyn Decode>>,
    /// 已移除音轨的累计统计。
    retired: Stats,
    /// 混音输出的录制缓冲。
    #[cfg(any(test, feature = "test-media"))]
    pub tap: Option<std::sync::Arc<std::sync::Mutex<Vec<f32>>>>,
}

fn make_opus() -> Option<Box<dyn Decode>> {
    opus_decoder()
        .map_err(|e| log::warn!("Opus 解码器初始化失败：{e}"))
        .ok()
}

impl Default for Mixer {
    fn default() -> Self {
        Self::with_decoder(make_opus)
    }
}

impl Mixer {
    #[must_use]
    pub fn with_decoder(make: fn() -> Option<Box<dyn Decode>>) -> Self {
        Self {
            tracks: HashMap::new(),
            volumes: HashMap::new(),
            denoised: HashMap::new(),
            denoise_base: Denoise::default(),
            master: 100,
            clock: 0,
            scratch: vec![0.0; MAX_FRAME],
            make,
            retired: Stats::default(),
            #[cfg(any(test, feature = "test-media"))]
            tap: None,
        }
    }

    /// 添加音轨。解码器创建失败时忽略。
    pub fn add_track(&mut self, ssrc: u32, peer: &str) {
        let Some(decoder) = (self.make)() else {
            return;
        };
        self.tracks.insert(
            ssrc,
            Track {
                peer: peer.to_owned(),
                jitter: Jitter::new(),
                decoder,
                pcm: VecDeque::new(),
                frame: 960,
                spoke: None,
                denoise: self.denoised.get(peer).map(Denoise::stage),
            },
        );
    }

    /// 设置成员降噪的方式来源，通常为本机降噪设置。
    pub fn set_denoise_base(&mut self, base: Denoise) {
        self.denoise_base = base;
        let peers: HashSet<String> = self.denoised.keys().cloned().collect();
        self.denoised.clear();
        self.set_denoised(&peers);
    }

    /// 开启或关闭成员降噪，立即作用于该成员的音轨。
    pub fn set_peer_denoise(&mut self, peer: &str, on: bool) {
        if on == self.denoised.contains_key(peer) {
            return;
        }
        if on {
            self.denoised
                .insert(peer.to_owned(), self.denoise_base.child());
        } else {
            self.denoised.remove(peer);
        }
        let d = self.denoised.get(peer);
        for t in self.tracks.values_mut().filter(|t| t.peer == peer) {
            t.denoise = d.map(Denoise::stage);
        }
    }

    /// 按集合设置开启降噪的成员。
    pub fn set_denoised(&mut self, peers: &HashSet<String>) {
        let old: Vec<String> = self
            .denoised
            .keys()
            .filter(|p| !peers.contains(*p))
            .cloned()
            .collect();
        for p in old {
            self.set_peer_denoise(&p, false);
        }
        for p in peers {
            self.set_peer_denoise(p, true);
        }
    }

    /// 成员降噪实例来源，供播放线程在混音锁之外创建实例。
    #[must_use]
    pub fn denoisers(&self) -> Vec<Denoise> {
        self.denoised.values().cloned().collect()
    }

    /// 移除音轨，统计计入累计。
    pub fn remove_track(&mut self, ssrc: u32) {
        if let Some(t) = self.tracks.remove(&ssrc) {
            self.retired.add(&t.jitter.stats);
        }
    }

    /// 移除成员的全部音轨。
    pub fn remove_peer(&mut self, peer: &str) {
        let retired = &mut self.retired;
        self.tracks.retain(|_, t| {
            let keep = t.peer != peer;
            if !keep {
                retired.add(&t.jitter.stats);
            }
            keep
        });
    }

    /// 清空音轨的缓冲，保留音轨。
    pub fn clear_track(&mut self, ssrc: u32) {
        if let Some(t) = self.tracks.get_mut(&ssrc) {
            t.jitter.clear();
            t.pcm.clear();
            t.spoke = None;
            if let Some(d) = t.denoise.as_mut() {
                d.clear();
            }
        }
    }

    /// 清空全部音轨的缓冲。
    pub fn clear_all(&mut self) {
        for t in self.tracks.values_mut() {
            t.jitter.clear();
            t.pcm.clear();
            t.spoke = None;
            if let Some(d) = t.denoise.as_mut() {
                d.clear();
            }
        }
    }

    /// 移除全部音轨，统计计入累计。
    pub fn reset(&mut self) {
        for t in self.tracks.values() {
            self.retired.add(&t.jitter.stats);
        }
        self.tracks.clear();
    }

    /// 接收统计与最大缓冲深度，深度单位为毫秒。
    #[must_use]
    pub fn stats(&self) -> (Stats, u32) {
        let mut s = self.retired;
        let mut depth = 0;
        for t in self.tracks.values() {
            s.add(&t.jitter.stats);
            depth = depth.max(t.jitter.buffered() / 48);
        }
        (s, depth)
    }

    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// 放入一个 RTP 包，未知 SSRC 忽略。
    pub fn push(&mut self, ssrc: u32, seq: u16, timestamp: u32, payload: Bytes) {
        if let Some(t) = self.tracks.get_mut(&ssrc) {
            t.jitter.push(seq, timestamp, payload);
        }
    }

    pub fn set_volume(&mut self, peer: &str, percent: u16) {
        let v = percent.min(MAX_VOLUME);
        if v == 100 {
            self.volumes.remove(peer);
        } else {
            self.volumes.insert(peer.to_owned(), v);
        }
    }

    pub fn set_volumes(&mut self, volumes: &HashMap<String, u16>) {
        self.volumes.clone_from(volumes);
        self.volumes.retain(|_, v| *v != 100);
    }

    pub fn set_master(&mut self, percent: u16) {
        self.master = percent.min(MAX_VOLUME);
    }

    /// 混合各音轨填满输出。缓冲不足的音轨输出已有样本，其余补静音。
    #[allow(clippy::cast_precision_loss)]
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let n = out.len();
        let master = gain(self.master);
        let clock = self.clock;
        for t in self.tracks.values_mut() {
            let full = t.fill(n, &mut self.scratch);
            let take = if full { n } else { t.pcm.len().min(n) };
            let g = master * self.volumes.get(&t.peer).copied().map_or(1.0, gain);
            let mut energy = 0.0f32;
            for (o, s) in out.iter_mut().zip(t.pcm.drain(..take)) {
                energy += s * s;
                *o += s * g;
            }
            // 按整段输出长度计算 RMS。
            if take > 0 && (energy / n as f32).sqrt() >= SPEAK_RMS {
                t.spoke = Some(clock + n as u64);
            }
        }
        for x in out.iter_mut() {
            *x = limit(*x);
        }
        self.clock += n as u64;
        #[cfg(any(test, feature = "test-media"))]
        if let Some(tap) = &self.tap
            && let Ok(mut v) = tap.lock()
        {
            v.extend_from_slice(out);
        }
    }

    /// 正在说话的成员 ID，已排序去重。
    #[must_use]
    pub fn speaking(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .tracks
            .values()
            .filter(|t| t.spoke.is_some_and(|s| self.clock <= s + SPEAK_HOLD))
            .map(|t| t.peer.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// 说话状态上报节流。
pub struct Throttle {
    interval: std::time::Duration,
    last: Option<std::time::Instant>,
    sent: Vec<String>,
}

impl Throttle {
    #[must_use]
    pub fn new(interval: std::time::Duration) -> Self {
        Self {
            interval,
            last: None,
            sent: Vec::new(),
        }
    }

    /// 上次上报的列表为空。
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.sent.is_empty()
    }

    /// 列表变化且距上次上报超过间隔时返回新列表。
    pub fn update(&mut self, now: std::time::Instant, current: Vec<String>) -> Option<Vec<String>> {
        if current == self.sent {
            return None;
        }
        if self
            .last
            .is_some_and(|l| now.duration_since(l) < self.interval)
        {
            return None;
        }
        self.last = Some(now);
        self.sent.clone_from(&current);
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 按包首字节输出恒定值的解码器。
    struct Fake;
    impl Decode for Fake {
        fn decode(&mut self, p: &[u8], out: &mut [f32], fec: bool) -> usize {
            let n = if fec { out.len() } else { 960 };
            out[..n].fill(f32::from(p[0]) / 100.0);
            n
        }
        fn conceal(&mut self, out: &mut [f32]) -> usize {
            out.fill(0.0);
            out.len()
        }
    }

    fn mixer() -> Mixer {
        Mixer::with_decoder(|| Some(Box::new(Fake)))
    }

    /// 按 20 ms 帧送入 `count` 个包。
    fn feed(m: &mut Mixer, ssrc: u32, amp: u8, from: u16, count: u16) {
        for s in from..from + count {
            m.push(ssrc, s, u32::from(s) * 960, Bytes::from(vec![amp]));
        }
    }

    fn render(m: &mut Mixer, n: usize) -> Vec<f32> {
        let mut out = vec![0.0; n];
        m.render(&mut out);
        out
    }

    #[test]
    fn limiter_is_bounded_and_transparent_below_knee() {
        assert!((limit(0.5) - 0.5).abs() < f32::EPSILON);
        assert!((limit(-0.8) + 0.8).abs() < f32::EPSILON);
        for x in [1.0, 2.0, 10.0, -50.0] {
            assert!(limit(x).abs() <= 1.0, "{x}");
        }
        assert!(limit(1.5) > limit(1.0), "限幅单调");
    }

    #[test]
    fn three_tracks_are_all_mixed_with_volumes() {
        let mut m = mixer();
        m.add_track(1, "a");
        m.add_track(2, "b");
        m.add_track(3, "c");
        feed(&mut m, 1, 10, 0, 5);
        feed(&mut m, 2, 20, 0, 5);
        feed(&mut m, 3, 30, 0, 5);
        let out = render(&mut m, 960);
        // 0.1 + 0.2 + 0.3。
        assert!((out[0] - 0.6).abs() < 1e-5, "{}", out[0]);
        assert_eq!(m.speaking(), vec!["a", "b", "c"]);
        // a 静音，c 超过上限按 200% 计，总音量 50%。
        m.set_volume("a", 0);
        m.set_volume("c", 250);
        m.set_master(50);
        let out = render(&mut m, 960);
        assert!((out[0] - 0.4).abs() < 1e-5, "{}", out[0]);
        // 超出范围时由限幅器限制。
        m.set_master(200);
        let out = render(&mut m, 960);
        assert!(out[0] <= 1.0 && out[0] > 0.8);
    }

    #[test]
    fn removed_or_paused_tracks_leave_no_sound() {
        let mut m = mixer();
        m.add_track(1, "a");
        m.add_track(2, "b");
        feed(&mut m, 1, 50, 0, 10);
        feed(&mut m, 2, 50, 0, 10);
        assert!(render(&mut m, 960)[0] > 0.9);
        // 移除与清空后没有声音。
        m.remove_track(2);
        m.clear_track(1);
        assert!(render(&mut m, 4800).iter().all(|&x| x == 0.0));
        // 成员离开后，迟到的包不再播放。
        m.remove_peer("a");
        assert_eq!(m.track_count(), 0);
        feed(&mut m, 1, 50, 20, 10);
        assert!(render(&mut m, 960).iter().all(|&x| x == 0.0));
    }

    #[test]
    fn underrun_outputs_silence_without_repeating() {
        let mut m = mixer();
        m.add_track(1, "a");
        feed(&mut m, 1, 40, 0, 4);
        let out = render(&mut m, 960 * 6);
        let loud = out.iter().filter(|&&x| x != 0.0).count();
        assert_eq!(loud, 960 * 4, "只播放已到达的包，不重复");
        // 超过保持时长后不再显示说话。
        render(&mut m, 48 * 400);
        assert_eq!(m.speaking(), Vec::<String>::new());
        let (s, depth) = m.stats();
        assert_eq!((s.packets, s.underruns, depth), (4, 1, 0));
        // 移除音轨后统计仍保留。
        m.remove_peer("a");
        assert_eq!(m.stats().0.packets, 4);
    }

    #[test]
    fn lost_packet_is_concealed_in_place() {
        let mut m = mixer();
        m.add_track(1, "a");
        for s in [0u16, 1, 2, 4, 5] {
            m.push(1, s, u32::from(s) * 960, Bytes::from(vec![10]));
        }
        let out = render(&mut m, 960 * 6);
        // 丢包由 FEC 补齐，样本位置不变。
        assert!(out.iter().all(|&x| (x - 0.1).abs() < 1e-5));
    }

    #[test]
    fn throttle_limits_speaking_reports() {
        let t0 = Instant::now();
        let mut t = Throttle::new(Duration::from_millis(200));
        assert_eq!(t.update(t0, vec!["a".into()]), Some(vec!["a".into()]));
        assert_eq!(t.update(t0, vec!["a".into()]), None, "列表未变化");
        let t1 = t0 + Duration::from_millis(50);
        assert_eq!(t.update(t1, vec![]), None, "间隔内不上报");
        let t2 = t0 + Duration::from_millis(210);
        assert_eq!(t.update(t2, vec![]), Some(vec![]), "间隔后上报");
        assert_eq!(t.update(t2, vec![]), None);
    }

    #[test]
    fn peer_denoise_applies_to_existing_and_new_tracks() {
        let mut m = mixer();
        m.add_track(1, "a");
        m.set_peer_denoise("a", true);
        assert!(m.tracks[&1].denoise.is_some());
        m.add_track(2, "a");
        m.add_track(3, "b");
        assert!(m.tracks[&2].denoise.is_some());
        assert!(m.tracks[&3].denoise.is_none());
        assert_eq!(m.denoisers().len(), 1);
        m.set_denoised(&HashSet::from(["b".to_owned()]));
        assert!(m.tracks[&1].denoise.is_none());
        assert!(m.tracks[&3].denoise.is_some());
    }

    #[test]
    fn denoised_track_still_renders() {
        let mut m = mixer();
        m.denoise_base
            .set_mode(super::super::denoise::Mode::Rnnoise);
        m.add_track(1, "a");
        m.set_peer_denoise("a", true);
        feed(&mut m, 1, 50, 0, 20);
        let mut total = 0;
        for _ in 0..30 {
            total += render(&mut m, 480).len();
        }
        assert_eq!(total, 30 * 480);
    }
}
