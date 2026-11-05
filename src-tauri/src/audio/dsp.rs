//! 音频处理基础：声道混合、重采样、分帧、电平计算与有界队列。
//! 只依赖标准库，在采集与播放回调中调用，不分配锁。

use std::collections::VecDeque;

/// 内部处理采样率，与 Opus 协商一致。
pub const RATE: u32 = 48_000;
/// 每帧时长，单位毫秒，与 `ptime=60` 一致。
pub const FRAME_MS: u32 = 60;
/// 每帧单声道样本数。
pub const FRAME_SAMPLES: usize = (RATE / 1000 * FRAME_MS) as usize;

/// 把交错多声道样本按帧取平均，混为单声道。
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn mixdown(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect()
}

/// 插值核单侧零点数。
const HALF: usize = 16;
/// 每个零点间隔的查表精度。
const STEPS: usize = 512;

/// 带限 sinc 重采样器，使用 Blackman 窗。
/// 降采样时按目标采样率收窄截止频率，抑制混叠。输入可分块送入，块间保持连续。
pub struct Resampler {
    /// 每个输出样本对应的输入步长。
    step: f64,
    /// 下一个输出样本在缓冲中的位置。
    pos: f64,
    buf: Vec<f32>,
    /// 插值核查表，下标为距离乘以 `STEPS`。
    table: Vec<f32>,
    passthrough: bool,
}

impl Resampler {
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn new(from: u32, to: u32) -> Self {
        let step = f64::from(from) / f64::from(to);
        // 截止频率取两侧较低的奈奎斯特频率，留出 5% 过渡带。
        let cutoff = (1.0 / step).min(1.0) * 0.95;
        let table = (0..=HALF * STEPS)
            .map(|i| {
                let d = i as f64 / STEPS as f64;
                let x = std::f64::consts::PI * d * cutoff;
                let sinc = if d == 0.0 { 1.0 } else { x.sin() / x };
                // Blackman 窗，以核中心为窗口中点。
                let w = 0.5 + d / (2.0 * HALF as f64);
                let win = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * w).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * w).cos();
                (cutoff * sinc * win) as f32
            })
            .collect();
        Self {
            step,
            pos: HALF as f64,
            buf: vec![0.0; HALF],
            table,
            passthrough: from == to,
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn kernel(&self, d: f64) -> f32 {
        let x = d.abs() * STEPS as f64;
        let i = x as usize;
        if i >= HALF * STEPS {
            return 0.0;
        }
        let t = (x - i as f64) as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * t
    }

    /// 转换一段输入，返回已能确定的输出样本。
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.passthrough {
            return input.to_vec();
        }
        self.buf.extend_from_slice(input);
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 2);
        while self.pos + (HALF as f64) < self.buf.len() as f64 {
            let center = self.pos.floor() as usize;
            let frac = self.pos - center as f64;
            let mut acc = 0.0f32;
            for k in (center + 1 - HALF)..=(center + HALF) {
                acc += self.buf[k] * self.kernel(k as f64 - center as f64 - frac);
            }
            out.push(acc);
            self.pos += self.step;
        }
        // 只保留下一次插值所需的历史样本。
        let keep_from = (self.pos.floor() as usize).saturating_sub(HALF);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
        out
    }
}

/// 把浮点样本转为 i16，并按 60 ms 切分为完整帧。
#[derive(Default)]
pub struct Framer {
    pending: Vec<i16>,
}

impl Framer {
    #[allow(clippy::cast_possible_truncation)]
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<i16>> {
        self.pending.extend(
            samples
                .iter()
                .map(|x| (x.clamp(-1.0, 1.0) * 32767.0).round() as i16),
        );
        let mut frames = Vec::new();
        while self.pending.len() >= FRAME_SAMPLES {
            frames.push(self.pending.drain(..FRAME_SAMPLES).collect());
        }
        frames
    }
}

/// 按 RFC 6464 计算帧电平，返回 0 至 127 的 -dBov 值，127 表示静音。
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn audio_level(frame: &[i16]) -> u8 {
    if frame.is_empty() {
        return 127;
    }
    let sum: f64 = frame
        .iter()
        .map(|&s| {
            let v = f64::from(s) / 32768.0;
            v * v
        })
        .sum();
    let rms = (sum / frame.len() as f64).sqrt();
    if rms <= 0.0 {
        return 127;
    }
    (-20.0 * rms.log10()).round().clamp(0.0, 127.0) as u8
}

/// 把 -dBov 电平换算为 0 至 100 的显示值，-60 dBov 及以下为 0。
#[must_use]
pub fn meter(level: u8) -> u8 {
    let l = level.min(60);
    // 60 - l 不超过 60，结果不超过 100。
    u8::try_from((u32::from(60 - l) * 100) / 60).unwrap_or(100)
}

/// 检测输入是否持续全零。
/// macOS 拒绝麦克风权限时不报错，只提供全零样本。窗口内没有非零样本即判定为无输入，结果确定后不再改变。
pub struct ZeroProbe {
    window: usize,
    seen: usize,
    done: Option<bool>,
}

impl ZeroProbe {
    #[must_use]
    pub fn new(window: usize) -> Self {
        Self {
            window,
            seen: 0,
            done: None,
        }
    }

    /// 送入样本，返回已确定的结果。`Some(true)` 表示判定为全零。
    pub fn feed(&mut self, samples: &[f32]) -> Option<bool> {
        if self.done.is_none() {
            if samples.iter().any(|&x| x != 0.0) {
                self.done = Some(false);
            } else {
                self.seen += samples.len();
                if self.seen >= self.window {
                    self.done = Some(true);
                }
            }
        }
        self.done
    }
}

/// 定长队列，满时丢弃最旧的元素。
pub struct Bounded<T> {
    items: VecDeque<T>,
    cap: usize,
    dropped: u64,
}

impl<T> Bounded<T> {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(cap),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    /// 追加元素。队列已满并丢弃了最旧元素时返回 true。
    pub fn push(&mut self, item: T) -> bool {
        let full = self.items.len() >= self.cap;
        if full {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back(item);
        full
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 累计丢弃的元素数。
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::cast_precision_loss)]
    fn tone(freq: f64, rate: u32, n: usize, amp: f64) -> Vec<f32> {
        (0..n)
            .map(|i| {
                #[allow(clippy::cast_possible_truncation)]
                let v = (amp
                    * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin())
                    as f32;
                v
            })
            .collect()
    }

    /// 单频点的幅度，用于检查频谱。
    #[allow(clippy::cast_precision_loss)]
    fn magnitude(x: &[f32], freq: f64, rate: u32) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / f64::from(rate);
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            re += f64::from(v) * (w * i as f64).cos();
            im += f64::from(v) * (w * i as f64).sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    #[test]
    fn mixdown_averages_channels() {
        assert_eq!(mixdown(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(mixdown(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }

    #[test]
    fn resampler_keeps_rate_and_tone() {
        for from in [44_100, 16_000, 96_000] {
            let mut r = Resampler::new(from, RATE);
            let input = tone(440.0, from, from as usize, 0.5);
            // 按非整数块长分段送入，检查块间连续。
            let mut out = Vec::new();
            for chunk in input.chunks(473) {
                out.extend(r.process(chunk));
            }
            let expect = RATE as usize;
            assert!(
                out.len().abs_diff(expect) <= HALF * 4,
                "{from}: {}",
                out.len()
            );
            // 去掉两端各 0.1 秒的过渡段。
            let seg = &out[4800..out.len() - 4800];
            let at = magnitude(seg, 440.0, RATE);
            assert!((at - 0.5).abs() < 0.03, "{from}: 440 Hz 幅度为 {at}");
            assert!(magnitude(seg, 1000.0, RATE) < 0.01);
        }
    }

    #[test]
    fn resampler_suppresses_aliasing_when_downsampling() {
        // 30 kHz 超出 48 kHz 的奈奎斯特频率，不得混叠到 18 kHz。
        let mut r = Resampler::new(96_000, RATE);
        let out = r.process(&tone(30_000.0, 96_000, 96_000, 0.5));
        let seg = &out[2000..out.len() - 2000];
        assert!(magnitude(seg, 18_000.0, RATE) < 0.01);
    }

    #[test]
    fn resampler_passthrough_at_48k() {
        let mut r = Resampler::new(RATE, RATE);
        assert_eq!(r.process(&[0.1, 0.2]), vec![0.1, 0.2]);
    }

    #[test]
    fn framer_splits_60ms_frames_across_pushes() {
        let mut f = Framer::default();
        assert_eq!(f.push(&vec![0.0; FRAME_SAMPLES - 1]).len(), 0);
        let frames = f.push(&[1.0; 3]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), 2880);
        assert_eq!(frames[0][FRAME_SAMPLES - 1], 32767);
        assert_eq!(f.push(&vec![2.0; FRAME_SAMPLES - 3]).len(), 0);
        assert_eq!(f.push(&[2.0]).len(), 1, "跨块累积满一帧时输出");
    }

    #[test]
    fn level_follows_dbov() {
        assert_eq!(audio_level(&[0; 100]), 127);
        assert_eq!(audio_level(&[]), 127);
        // 满幅方波为 0 dBov。
        let square: Vec<i16> = (0..960)
            .map(|i| if i % 2 == 0 { 32767 } else { -32767 })
            .collect();
        assert_eq!(audio_level(&square), 0);
        // 幅度 0.1 的正弦波约为 -23 dBov。
        #[allow(clippy::cast_possible_truncation)]
        let sine: Vec<i16> = tone(440.0, RATE, 2880, 0.1)
            .iter()
            .map(|x| (x * 32768.0) as i16)
            .collect();
        assert_eq!(audio_level(&sine), 23);
        assert_eq!(meter(0), 100);
        assert_eq!(meter(127), 0);
        assert_eq!(meter(30), 50);
    }

    #[test]
    fn zero_probe_detects_denied_input() {
        let mut p = ZeroProbe::new(1000);
        assert_eq!(p.feed(&[0.0; 600]), None);
        assert_eq!(p.feed(&[0.0; 600]), Some(true));
        // 结果确定后不再改变。
        assert_eq!(p.feed(&[0.5]), Some(true));
        let mut p = ZeroProbe::new(1000);
        assert_eq!(p.feed(&[0.0, 1e-6]), Some(false));
        assert_eq!(p.feed(&vec![0.0; 5000]), Some(false));
    }

    #[test]
    fn bounded_queue_drops_oldest() {
        let mut q = Bounded::new(3);
        for i in 0..5 {
            q.push(i);
        }
        assert_eq!(q.len(), 3);
        assert_eq!(q.dropped(), 2);
        assert_eq!(q.pop(), Some(2));
        assert!(!q.push(9));
        assert_eq!(q.pop(), Some(3));
        assert_eq!(q.pop(), Some(4));
        assert_eq!(q.pop(), Some(9));
        assert_eq!(q.len(), 0);
    }
}
