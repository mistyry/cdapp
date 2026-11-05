//! 语音连接健康检查与链路质量。
//!
//! 按固定间隔检查以下情况：
//! - 系统时间与单调时间的差值突增，或两次检查间隔过长，判定为系统睡眠后唤醒。
//! - 媒体连接断开超过宽限期。
//! - 有成员推流，但长时间收不到媒体包。
//!
//! 判定结果交由会话重建连接。

use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde_json::Value;

/// 有成员推流时允许的最长无包时长。
pub const SILENCE_LIMIT: Duration = Duration::from_secs(10);
/// 媒体连接断开后的宽限期。
pub const DISCONNECT_GRACE: Duration = Duration::from_secs(5);
/// 系统时间超出单调时间的阈值，超过即判定为睡眠。
pub const SLEEP_SKEW: Duration = Duration::from_secs(5);
/// 两次检查的最大间隔，超过即判定为进程被挂起。
pub const STALL_GAP: Duration = Duration::from_secs(10);
/// 连接持续超过该时长视为稳定，重建次数重新计数。
pub const STABLE_AFTER: Duration = Duration::from_secs(30);
/// 重建退避上限。
const BACKOFF_MAX: Duration = Duration::from_secs(8);

/// 第 `attempt` 次重建前的等待时长：1、2、4、8 秒，之后保持 8 秒。
#[must_use]
pub fn backoff(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(3);
    (Duration::from_secs(1) * (1 << exp)).min(BACKOFF_MAX)
}

/// 连接异常。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// 系统睡眠后唤醒。
    Slept,
    /// 媒体连接断开。
    Disconnected,
    /// 有成员推流，但收不到媒体包。
    Silent,
}

impl Problem {
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Slept => "系统唤醒后重新连接",
            Self::Disconnected => "媒体连接已断开",
            Self::Silent => "长时间未收到语音数据",
        }
    }

    /// 是否立即重建，不等待退避。
    #[must_use]
    pub fn urgent(self) -> bool {
        self == Self::Slept
    }
}

/// 连接健康状态。
#[derive(Debug)]
pub struct Health {
    last_rx: Instant,
    /// 是否收到过媒体包。
    seen: bool,
    pushing: bool,
    /// 推流与拉流连接开始断开的时间。
    down: [Option<Instant>; 2],
    /// 上次检查的单调时间与系统时间。
    tick: Option<(Instant, SystemTime)>,
}

impl Health {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            last_rx: now,
            seen: false,
            pushing: false,
            down: [None; 2],
            tick: None,
        }
    }

    /// 收到媒体包。
    pub fn on_packet(&mut self, now: Instant) {
        self.last_rx = now;
        self.seen = true;
    }

    /// 把上次检查的系统时间提前 10 分钟，下一次检查判定为睡眠。
    #[cfg(any(test, feature = "test-media"))]
    pub fn fake_sleep(&mut self) {
        if let Some((_, w)) = self.tick.as_mut() {
            *w -= Duration::from_secs(600);
        }
    }

    /// 设置是否有成员推流。开始推流时重新计时。
    pub fn set_pushing(&mut self, pushing: bool, now: Instant) {
        if pushing && !self.pushing {
            self.last_rx = now;
        }
        self.pushing = pushing;
    }

    /// 媒体连接状态变化。`leg` 为 0 表示推流，1 表示拉流。
    pub fn on_media(&mut self, leg: usize, connected: bool, now: Instant) {
        if let Some(d) = self.down.get_mut(leg) {
            if connected {
                *d = None;
            } else if d.is_none() {
                *d = Some(now);
            }
        }
    }

    /// 是否有媒体连接处于断开状态。
    #[must_use]
    pub fn disconnected(&self) -> bool {
        self.down.iter().any(Option::is_some)
    }

    /// 检查一次，返回发现的异常。
    pub fn check(&mut self, now: Instant, wall: SystemTime) -> Option<Problem> {
        if let Some((m, w)) = self.tick.replace((now, wall)) {
            let mono = now.saturating_duration_since(m);
            let walled = wall.duration_since(w).unwrap_or_default();
            if walled.saturating_sub(mono) > SLEEP_SKEW || mono > STALL_GAP {
                return Some(Problem::Slept);
            }
        }
        if self
            .down
            .iter()
            .flatten()
            .any(|t| now.saturating_duration_since(*t) >= DISCONNECT_GRACE)
        {
            return Some(Problem::Disconnected);
        }
        if self.pushing && self.seen && now.saturating_duration_since(self.last_rx) >= SILENCE_LIMIT
        {
            return Some(Problem::Silent);
        }
        None
    }
}

/// 链路质量等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Good,
    Fair,
    Poor,
}

/// 按服务端评分与丢包率评定质量，两者都没有时返回 None。
#[must_use]
pub fn quality(score: Option<u8>, loss: Option<f64>) -> Option<Quality> {
    if score.is_none() && loss.is_none() {
        return None;
    }
    let (s, l) = (score.unwrap_or(10), loss.unwrap_or(0.0));
    Some(if s <= 4 || l >= 0.10 {
        Quality::Poor
    } else if s <= 7 || l >= 0.02 {
        Quality::Fair
    } else {
        Quality::Good
    })
}

/// 服务端推流统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpStats {
    pub score: u8,
    /// 丢包率，范围 0 至 1。
    pub loss: f64,
    pub packets: u64,
    pub lost: u64,
    pub bitrate: u64,
}

/// 解析 getProducerStats 响应。
#[must_use]
pub fn up_stats(v: &Value) -> Option<UpStats> {
    let item = v
        .as_array()?
        .iter()
        .find(|x| x.get("type").and_then(Value::as_str) == Some("inbound-rtp"))?;
    let n = |k: &str| item.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(UpStats {
        score: u8::try_from(item.get("score").and_then(Value::as_u64)?.min(10)).ok()?,
        #[allow(clippy::cast_precision_loss)]
        loss: n("fractionLost").min(256) as f64 / 256.0,
        packets: n("packetCount"),
        lost: n("packetsLost"),
        bitrate: n("bitrate"),
    })
}

/// 本地接收统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownStats {
    pub packets: u64,
    /// 丢失的包数。
    pub lost: u64,
    /// 迟到丢弃的包数。
    pub late: u64,
    /// 缓冲溢出丢弃的包数。
    pub overflow: u64,
    /// 缓冲耗尽次数。
    pub underruns: u64,
    /// 最大缓冲深度，单位毫秒。
    pub depth_ms: u32,
}

impl DownStats {
    #[must_use]
    pub fn from_jitter(s: &crate::audio::jitter::Stats, depth_ms: u32) -> Self {
        Self {
            packets: s.packets,
            lost: s.lost,
            late: s.late,
            overflow: s.overflow,
            underruns: s.underruns,
            depth_ms,
        }
    }
}

/// 接收丢包率，样本少于 20 个包时返回 None。
#[must_use]
pub fn down_loss(packets: u64, lost: u64) -> Option<f64> {
    let total = packets + lost;
    #[allow(clippy::cast_precision_loss)]
    (total >= 20).then(|| lost as f64 / total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn backoff_grows_and_caps() {
        let v: Vec<u64> = (1..=6).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(v, [1, 2, 4, 8, 8, 8]);
    }

    #[test]
    fn wall_clock_jump_means_sleep() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::new(t0);
        assert_eq!(h.check(t0, w0), None);
        // 正常间隔。
        assert_eq!(h.check(t0 + 2 * S, w0 + 2 * S), None);
        // 系统时间跳过 10 分钟。
        assert_eq!(h.check(t0 + 4 * S, w0 + 602 * S), Some(Problem::Slept));
        assert!(Problem::Slept.urgent());
    }

    #[test]
    fn long_gap_between_checks_means_suspended() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::new(t0);
        h.check(t0, w0);
        assert_eq!(h.check(t0 + 30 * S, w0 + 30 * S), Some(Problem::Slept));
        // 系统时间回拨不判定为睡眠。
        let mut h = Health::new(t0);
        h.check(t0, w0 + 100 * S);
        assert_eq!(h.check(t0 + 2 * S, w0), None);
    }

    #[test]
    fn disconnected_media_gets_grace_period() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::new(t0);
        h.on_media(1, false, t0);
        assert!(h.disconnected());
        assert_eq!(h.check(t0 + 2 * S, w0 + 2 * S), None);
        // 宽限期内恢复不算异常。
        h.on_media(1, true, t0 + 3 * S);
        assert_eq!(h.check(t0 + 6 * S, w0 + 6 * S), None);
        h.on_media(0, false, t0 + 6 * S);
        h.on_media(0, false, t0 + 8 * S);
        assert_eq!(h.check(t0 + 8 * S, w0 + 8 * S), None);
        assert_eq!(
            h.check(t0 + 11 * S, w0 + 11 * S),
            Some(Problem::Disconnected)
        );
        assert!(!Problem::Disconnected.urgent());
    }

    #[test]
    fn silence_only_counts_while_someone_pushes() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::new(t0);
        // 无人推流时不检查。
        for k in 1..=10 {
            assert_eq!(h.check(t0 + 2 * k * S, w0 + 2 * k * S), None);
        }
        // 开始推流后，超过时限未收到包即判定异常。
        let t1 = t0 + 20 * S;
        h.set_pushing(true, t1);
        assert_eq!(h.check(t1 + 8 * S, w0 + 28 * S), None);
        h.on_packet(t1 + 9 * S);
        assert_eq!(h.check(t1 + 10 * S, w0 + 30 * S), None);
        assert_eq!(h.check(t1 + 19 * S, w0 + 39 * S), Some(Problem::Silent));
        // 停止推流后不再检查。
        h.set_pushing(false, t1 + 19 * S);
        assert_eq!(h.check(t1 + 21 * S, w0 + 41 * S), None);
        // 重新推流时重新计时。
        h.set_pushing(true, t1 + 22 * S);
        assert_eq!(h.check(t1 + 30 * S, w0 + 50 * S), None);
        assert_eq!(h.check(t1 + 32 * S, w0 + 52 * S), Some(Problem::Silent));
    }

    #[test]
    fn fake_sleep_triggers_on_next_check() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::new(t0);
        h.check(t0, w0);
        h.fake_sleep();
        assert_eq!(h.check(t0 + 2 * S, w0 + 2 * S), Some(Problem::Slept));
        assert_eq!(h.check(t0 + 4 * S, w0 + 4 * S), None);
    }

    #[test]
    fn quality_levels() {
        assert_eq!(quality(None, None), None);
        assert_eq!(quality(Some(10), Some(0.0)), Some(Quality::Good));
        assert_eq!(quality(Some(10), None), Some(Quality::Good));
        assert_eq!(quality(Some(7), Some(0.0)), Some(Quality::Fair));
        assert_eq!(quality(None, Some(0.05)), Some(Quality::Fair));
        assert_eq!(quality(Some(10), Some(0.2)), Some(Quality::Poor));
        assert_eq!(quality(Some(3), None), Some(Quality::Poor));
    }

    #[test]
    fn parses_producer_stats_and_loss() {
        let v = json!([{"type": "inbound-rtp", "score": 10, "fractionLost": 64,
            "packetCount": 380, "packetsLost": 2, "bitrate": 72400}]);
        let s = up_stats(&v).unwrap();
        assert_eq!((s.score, s.packets, s.lost, s.bitrate), (10, 380, 2, 72400));
        assert!((s.loss - 0.25).abs() < 1e-9);
        assert_eq!(up_stats(&json!([])), None);
        assert_eq!(up_stats(&json!({})), None);
        assert_eq!(down_loss(10, 0), None);
        assert_eq!(down_loss(90, 10), Some(0.1));
    }
}
