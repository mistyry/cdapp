use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde_json::Value;

pub const DISCONNECT_GRACE: Duration = Duration::from_secs(5);
pub const SLEEP_SKEW: Duration = Duration::from_secs(5);
pub const STALL_GAP: Duration = Duration::from_secs(10);
pub const STABLE_AFTER: Duration = Duration::from_secs(30);
pub const RECV_STALL: Duration = Duration::from_secs(15);
const BACKOFF_MAX: Duration = Duration::from_secs(8);

#[must_use]
pub fn backoff(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(3);
    (Duration::from_secs(1) * (1 << exp)).min(BACKOFF_MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Slept,
    Disconnected,
    RecvStalled,
}

impl Problem {
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Slept => "系统唤醒后重新连接",
            Self::Disconnected => "媒体连接已断开",
            Self::RecvStalled => "接收语音中断",
        }
    }

    #[must_use]
    pub fn urgent(self) -> bool {
        self == Self::Slept
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Checked {
    pub problem: Option<Problem>,
    pub gap: Option<(Duration, Duration)>,
}

#[derive(Debug, Default)]
pub struct Health {
    down: [Option<Instant>; 2],
    tick: Option<(Instant, SystemTime)>,
    recv: Option<(u64, Instant)>,
}

impl Health {
    pub fn on_recv(&mut self, total: u64, expecting: bool, now: Instant) -> Option<Duration> {
        if !expecting {
            self.recv = None;
            return None;
        }
        match self.recv {
            Some((n, since)) if n == total => {
                let idle = now.saturating_duration_since(since);
                (idle >= RECV_STALL).then_some(idle)
            }
            _ => {
                self.recv = Some((total, now));
                None
            }
        }
    }

    #[cfg(any(test, feature = "test-media"))]
    pub fn fake_sleep(&mut self) {
        if let Some((_, w)) = self.tick.as_mut() {
            *w -= Duration::from_secs(600);
        }
    }

    pub fn on_media(&mut self, leg: usize, connected: bool, now: Instant) -> Option<Duration> {
        let d = self.down.get_mut(leg)?;
        if connected {
            d.take().map(|t| now.saturating_duration_since(t))
        } else {
            d.get_or_insert(now);
            None
        }
    }

    #[must_use]
    pub fn down_for(&self, now: Instant) -> Vec<(usize, Duration)> {
        self.down
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.map(|t| (i, now.saturating_duration_since(t))))
            .collect()
    }

    #[must_use]
    pub fn disconnected(&self) -> bool {
        self.down.iter().any(Option::is_some)
    }

    pub fn check(&mut self, now: Instant, wall: SystemTime) -> Checked {
        let mut out = Checked::default();
        if let Some((m, w)) = self.tick.replace((now, wall)) {
            let mono = now.saturating_duration_since(m);
            let walled = wall.duration_since(w).unwrap_or_default();
            let skew = walled.saturating_sub(mono) > SLEEP_SKEW;
            if skew || mono > STALL_GAP {
                out.gap = Some((mono, walled));
            }
            if skew {
                out.problem = Some(Problem::Slept);
                return out;
            }
        }
        if self
            .down
            .iter()
            .flatten()
            .any(|t| now.saturating_duration_since(*t) >= DISCONNECT_GRACE)
        {
            out.problem = Some(Problem::Disconnected);
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Good,
    Fair,
    Poor,
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpStats {
    pub score: u8,
    pub loss: f64,
    pub packets: u64,
    pub lost: u64,
    pub bitrate: u64,
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownStats {
    pub packets: u64,
    pub lost: u64,
    pub late: u64,
    pub overflow: u64,
    pub underruns: u64,
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
        let mut h = Health::default();
        assert_eq!(h.check(t0, w0), Checked::default());
        assert_eq!(h.check(t0 + 2 * S, w0 + 2 * S), Checked::default());
        let c = h.check(t0 + 4 * S, w0 + 602 * S);
        assert_eq!(c.problem, Some(Problem::Slept));
        assert_eq!(c.gap, Some((2 * S, 600 * S)));
        assert!(Problem::Slept.urgent());
        let mut h = Health::default();
        h.check(t0, w0 + 100 * S);
        assert_eq!(h.check(t0 + 2 * S, w0), Checked::default());
    }

    #[test]
    fn long_gap_is_only_reported() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::default();
        h.check(t0, w0);
        let c = h.check(t0 + 30 * S, w0 + 30 * S);
        assert_eq!(c.problem, None, "计时器节流不重建");
        assert_eq!(c.gap, Some((30 * S, 30 * S)));
    }

    #[test]
    fn disconnected_media_gets_grace_period() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::default();
        h.on_media(1, false, t0);
        assert!(h.disconnected());
        assert_eq!(h.check(t0 + 2 * S, w0 + 2 * S).problem, None);
        h.on_media(1, true, t0 + 3 * S);
        assert_eq!(h.check(t0 + 6 * S, w0 + 6 * S).problem, None);
        h.on_media(0, false, t0 + 6 * S);
        h.on_media(0, false, t0 + 8 * S);
        assert_eq!(h.check(t0 + 8 * S, w0 + 8 * S).problem, None);
        assert_eq!(
            h.check(t0 + 11 * S, w0 + 11 * S).problem,
            Some(Problem::Disconnected)
        );
        assert!(!Problem::Disconnected.urgent());
    }

    #[test]
    fn fake_sleep_triggers_on_next_check() {
        let (t0, w0) = (Instant::now(), SystemTime::now());
        let mut h = Health::default();
        h.check(t0, w0);
        h.fake_sleep();
        assert_eq!(
            h.check(t0 + 2 * S, w0 + 2 * S).problem,
            Some(Problem::Slept)
        );
        assert_eq!(h.check(t0 + 4 * S, w0 + 4 * S).problem, None);
    }

    #[test]
    fn media_down_duration_is_tracked() {
        let t0 = Instant::now();
        let mut h = Health::default();
        assert_eq!(h.on_media(1, false, t0), None);
        assert_eq!(h.on_media(1, false, t0 + 2 * S), None);
        assert_eq!(h.down_for(t0 + 3 * S), vec![(1, 3 * S)]);
        assert_eq!(h.on_media(1, true, t0 + 4 * S), Some(4 * S));
        assert_eq!(h.down_for(t0 + 5 * S), Vec::new());
        assert_eq!(h.on_media(0, true, t0 + 5 * S), None);
    }

    #[test]
    fn stalled_receive_is_detected_only_while_expecting_audio() {
        let t0 = Instant::now();
        let mut h = Health::default();
        assert_eq!(h.on_recv(100, true, t0), None);
        assert_eq!(h.on_recv(150, true, t0 + 10 * S), None);
        assert_eq!(h.on_recv(150, true, t0 + 24 * S), None);
        assert_eq!(h.on_recv(150, true, t0 + 25 * S), Some(15 * S));
        assert_eq!(h.on_recv(150, false, t0 + 26 * S), None);
        assert_eq!(h.on_recv(150, true, t0 + 27 * S), None);
        assert_eq!(h.on_recv(150, true, t0 + 41 * S), None);
        assert_eq!(h.on_recv(150, true, t0 + 42 * S), Some(15 * S));
        assert_eq!(h.on_recv(151, true, t0 + 43 * S), None);
        assert!(!Problem::RecvStalled.urgent());
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
