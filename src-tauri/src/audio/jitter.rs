//! 单路远端音频的抖动缓冲。
//! 按 RTP 序号排序，缓冲达到目标时长后开始输出，缺包时提供下一包供 FEC 恢复。
//! 时长以 48 kHz 时间戳单位计算。

use std::collections::BTreeMap;

use bytes::Bytes;

/// 开始播放所需的缓冲时长，80 ms。
pub const TARGET: u32 = 48 * 80;
/// 缓冲上限，200 ms。超过后丢弃最旧的包，降回目标时长。
pub const MAX: u32 = 48 * 200;
/// 连续缺包超过该数时直接跳到下一个已到达的包。
const MAX_GAP: u64 = 10;
/// 序号跳变超过该值时视为发送端重启，清空缓冲。
const RESET_JUMP: i64 = 1000;
/// 尚未从时间戳推断出帧长时使用的默认值，20 ms。
const DEFAULT_FRAME: u32 = 960;

/// 一次取包的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pull {
    /// 按序到达的包。
    Packet(Bytes),
    /// 当前包丢失。`next` 为紧随其后的包，可用于 FEC 恢复。
    Lost { next: Option<Bytes> },
    /// 缓冲不足，暂不输出。
    Wait,
}

#[derive(Debug, Clone)]
struct Item {
    timestamp: u32,
    payload: Bytes,
}

/// 缓冲统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub late: u64,
    pub duplicate: u64,
    pub overflow: u64,
    pub lost: u64,
    /// 已接收的包数。
    pub packets: u64,
    /// 播放中缓冲耗尽的次数。
    pub underruns: u64,
}

impl Stats {
    /// 累加另一份统计。
    pub fn add(&mut self, o: &Self) {
        self.late += o.late;
        self.duplicate += o.duplicate;
        self.overflow += o.overflow;
        self.lost += o.lost;
        self.packets += o.packets;
        self.underruns += o.underruns;
    }
}

#[derive(Debug, Default)]
pub struct Jitter {
    items: BTreeMap<u64, Item>,
    /// 已收到的最大扩展序号。
    highest: Option<u64>,
    /// 下一个应输出的扩展序号。
    next: Option<u64>,
    playing: bool,
    /// 相邻包的时间戳间隔，即每包样本数。
    frame: u32,
    pub stats: Stats,
}

impl Jitter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame: DEFAULT_FRAME,
            ..Self::default()
        }
    }

    /// 把 16 位序号扩展为单调递增的 64 位序号，处理回绕。
    fn extend(&self, seq: u16) -> u64 {
        match self.highest {
            None => (1 << 16) + u64::from(seq),
            Some(h) => {
                #[allow(clippy::cast_possible_truncation)]
                let diff = i64::from(seq.wrapping_sub(h as u16).cast_signed());
                h.saturating_add_signed(diff)
            }
        }
    }

    /// 缓冲中首尾包覆盖的时长。
    #[must_use]
    pub fn buffered(&self) -> u32 {
        match (self.items.first_key_value(), self.items.last_key_value()) {
            (Some((_, a)), Some((_, b))) => b.timestamp.wrapping_sub(a.timestamp) + self.frame,
            _ => 0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn playing(&self) -> bool {
        self.playing
    }

    /// 放入一个包。迟到与重复的包只计数后丢弃。
    pub fn push(&mut self, seq: u16, timestamp: u32, payload: Bytes) {
        let mut ext = self.extend(seq);
        if let Some(h) = self.highest {
            #[allow(clippy::cast_possible_wrap)]
            let jump = ext as i64 - h as i64;
            if jump.abs() > RESET_JUMP {
                self.clear();
                ext = self.extend(seq);
            }
        }
        if self.next.is_some_and(|n| ext < n) {
            self.stats.late += 1;
            return;
        }
        if self.items.contains_key(&ext) {
            self.stats.duplicate += 1;
            return;
        }
        // 用相邻包的时间戳差更新帧长。
        if let Some(prev) = self.items.get(&(ext.wrapping_sub(1))) {
            let d = timestamp.wrapping_sub(prev.timestamp);
            if (120..=5760).contains(&d) {
                self.frame = d;
            }
        }
        self.items.insert(ext, Item { timestamp, payload });
        self.stats.packets += 1;
        self.highest = Some(self.highest.map_or(ext, |h| h.max(ext)));
        // 超过上限时丢弃最旧的包，降回目标时长。
        if self.buffered() > MAX {
            while self.buffered() > TARGET && self.items.len() > 1 {
                if let Some((k, _)) = self.items.pop_first() {
                    self.stats.overflow += 1;
                    if self.next.is_some_and(|n| n <= k) {
                        self.next = Some(k + 1);
                    }
                }
            }
            if let (Some(n), Some((&first, _))) = (self.next, self.items.first_key_value())
                && first > n
            {
                self.next = Some(first);
            }
        }
    }

    /// 取下一个应播放的包。
    pub fn pull(&mut self) -> Pull {
        if !self.playing {
            if self.items.is_empty() || self.buffered() < TARGET {
                return Pull::Wait;
            }
            self.playing = true;
            // 从缓冲中最早的包开始播放。
            self.next = self.items.first_key_value().map(|(k, _)| *k);
        }
        let Some((&first, _)) = self.items.first_key_value() else {
            // 缓冲耗尽，重新等待达到目标时长。
            self.playing = false;
            self.stats.underruns += 1;
            return Pull::Wait;
        };
        let next = self.next.unwrap_or(first);
        if first == next {
            self.next = Some(next + 1);
            let item = self.items.remove(&first).map(|i| i.payload);
            return item.map_or(Pull::Wait, Pull::Packet);
        }
        // 当前序号缺失。
        self.stats.lost += 1;
        if first - next > MAX_GAP {
            self.next = Some(first);
            return Pull::Lost { next: None };
        }
        self.next = Some(next + 1);
        let fec = (first == next + 1)
            .then(|| self.items.get(&first).map(|i| i.payload.clone()))
            .flatten();
        Pull::Lost { next: fec }
    }

    /// 清空缓冲与序号状态，统计保留。
    pub fn clear(&mut self) {
        self.items.clear();
        self.highest = None;
        self.next = None;
        self.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u8) -> Bytes {
        Bytes::from(vec![n])
    }

    /// 按序号送入，时间戳按 20 ms 帧递增。
    #[allow(clippy::cast_possible_truncation)]
    fn feed(j: &mut Jitter, seqs: &[u16]) {
        for &s in seqs {
            j.push(s, u32::from(s) * 960, b(s as u8));
        }
    }

    fn drain(j: &mut Jitter) -> Vec<Pull> {
        let mut out = Vec::new();
        loop {
            let p = j.pull();
            if p == Pull::Wait {
                return out;
            }
            out.push(p);
        }
    }

    fn packets(seqs: &[u8]) -> Vec<Pull> {
        seqs.iter().map(|&s| Pull::Packet(b(s))).collect()
    }

    #[test]
    fn waits_for_target_then_plays_in_order() {
        let mut j = Jitter::new();
        feed(&mut j, &[10, 11, 12]);
        assert_eq!(j.pull(), Pull::Wait, "60 ms 缓冲不足 80 ms 时等待");
        feed(&mut j, &[14, 13]);
        assert_eq!(drain(&mut j), packets(&[10, 11, 12, 13, 14]));
        assert!(!j.playing(), "缓冲耗尽后停止播放");
        feed(&mut j, &[15]);
        assert_eq!(j.pull(), Pull::Wait, "重新缓冲到目标时长前不输出");
    }

    #[test]
    fn drops_duplicates_and_late_packets() {
        let mut j = Jitter::new();
        feed(&mut j, &[1, 2, 3, 4, 4]);
        assert_eq!(j.stats.duplicate, 1);
        assert_eq!(j.pull(), Pull::Packet(b(1)));
        feed(&mut j, &[1]);
        assert_eq!(j.stats.late, 1);
        assert_eq!(drain(&mut j), packets(&[2, 3, 4]));
    }

    #[test]
    fn loss_offers_fec_from_next_packet() {
        let mut j = Jitter::new();
        feed(&mut j, &[1, 2, 4, 7]);
        assert_eq!(
            drain(&mut j),
            vec![
                Pull::Packet(b(1)),
                Pull::Packet(b(2)),
                Pull::Lost { next: Some(b(4)) },
                Pull::Packet(b(4)),
                Pull::Lost { next: None },
                Pull::Lost { next: Some(b(7)) },
                Pull::Packet(b(7)),
            ]
        );
        assert_eq!(j.stats.lost, 3);
    }

    #[test]
    fn sequence_wraparound_keeps_order() {
        let mut j = Jitter::new();
        // 跨越 65535 回绕并乱序到达。
        for (seq, i) in [(65534u16, 0u32), (1, 3), (65535, 1), (0, 2)] {
            #[allow(clippy::cast_possible_truncation)]
            j.push(seq, i * 960, b(seq as u8));
        }
        assert_eq!(drain(&mut j), packets(&[254, 255, 0, 1]));
        assert_eq!(j.stats.late + j.stats.lost, 0);
    }

    #[test]
    fn overflow_drops_oldest_down_to_target() {
        let mut j = Jitter::new();
        // 一次送入 1 秒的包，超过上限。
        feed(&mut j, &(0..50).collect::<Vec<_>>());
        assert!(j.buffered() <= MAX, "{}", j.buffered());
        assert!(j.stats.overflow > 30);
        // 保留的包按序输出到最后一包。
        let out = drain(&mut j);
        assert_eq!(out.last(), Some(&Pull::Packet(b(49))));
        assert!(out.iter().all(|p| matches!(p, Pull::Packet(_))));
    }

    #[test]
    fn long_gap_skips_and_restart_resets() {
        let mut j = Jitter::new();
        feed(&mut j, &[1, 2, 3, 4]);
        assert_eq!(j.pull(), Pull::Packet(b(1)));
        // 跨度超过上限，丢弃旧包后从新包继续，不计丢包。
        feed(&mut j, &[30]);
        assert_eq!(j.pull(), Pull::Packet(b(30)));
        assert_eq!(j.stats.lost, 0);
        // 序号跳变过大，视为发送端重启。
        feed(&mut j, &[40000, 40001, 40002, 40003]);
        assert_eq!(drain(&mut j), packets(&[64, 65, 66, 67]));
    }

    #[test]
    fn clear_discards_everything() {
        let mut j = Jitter::new();
        feed(&mut j, &[1, 2, 3, 4]);
        j.clear();
        assert!(j.is_empty());
        assert_eq!(j.pull(), Pull::Wait);
        // 清空后可以重新开始。
        feed(&mut j, &[1, 2, 3, 4]);
        assert_eq!(j.pull(), Pull::Packet(b(1)));
    }
}
