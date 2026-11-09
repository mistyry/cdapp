//! 拉流音轨、成员事件与链路质量统计。

use super::{
    AppResult, Arc, Consumer, DownStats, HashMap, JoinParams, Mixer, Mutex, Peer, PeerPatch, Sink,
    UpStats, Value, VoiceEvent, health, id_of, sdp,
};

/// 链路质量统计，按两次统计之间的增量计算接收丢包率。
#[derive(Default)]
pub(super) struct LinkMeter {
    pub(super) last: DownStats,
}

impl LinkMeter {
    pub(super) fn event(
        &mut self,
        up: Option<UpStats>,
        rtt: Option<u32>,
        down: DownStats,
    ) -> VoiceEvent {
        let dl = health::down_loss(
            down.packets.saturating_sub(self.last.packets),
            down.lost.saturating_sub(self.last.lost),
        );
        self.last = down;
        let loss = match (up.map(|u| u.loss), dl) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        VoiceEvent::Link {
            quality: health::quality(up.map(|u| u.score), loss),
            up,
            rtt,
            down,
        }
    }
}

/// 房间内的拉流音轨与成员状态。
#[derive(Default)]
pub(super) struct Room {
    pub(super) consumers: Vec<Consumer>,
    pub(super) version: u64,
    /// 音轨 ID 到成员 ID 的映射。
    pub(super) owners: HashMap<String, String>,
    /// 已暂停的音轨。
    pub(super) paused: std::collections::HashSet<String>,
    /// 音轨的增删同步到混音器。
    pub(super) mixer: Option<Arc<Mutex<Mixer>>>,
}

impl Room {
    pub(super) fn mix(&self, f: impl FnOnce(&mut Mixer)) {
        if let Some(m) = &self.mixer
            && let Ok(mut g) = m.lock()
        {
            f(&mut g);
        }
    }

    /// 是否有成员正在推流。
    pub(super) fn ssrc_of(&self, consumer: &str) -> Option<u32> {
        self.consumers
            .iter()
            .find(|c| c.id == consumer && !c.closed)
            .map(|c| c.ssrc)
    }

    /// 未暂停且超过 `after` 未收到包的音轨。从未收到包的音轨自 `since`，即媒体连接建立时起算。
    pub(super) fn quiet_tracks(
        &self,
        tracks: &HashMap<u32, (std::time::Instant, u64)>,
        since: std::time::Instant,
        now: std::time::Instant,
        after: std::time::Duration,
    ) -> Vec<u32> {
        self.consumers
            .iter()
            .filter(|c| !c.closed && !self.paused.contains(&c.id))
            .filter(|c| {
                let last = tracks.get(&c.ssrc).map_or(since, |(t, _)| *t);
                now.saturating_duration_since(last) >= after
            })
            .map(|c| c.ssrc)
            .collect()
    }

    /// 诊断摘要：每条未关闭音轨的成员、暂停状态、累计包数与距最后一包时长。
    pub(super) fn describe(
        &self,
        tracks: &HashMap<u32, (std::time::Instant, u64)>,
        now: std::time::Instant,
    ) -> String {
        let items: Vec<String> = self
            .consumers
            .iter()
            .filter(|c| !c.closed)
            .map(|c| {
                let state = if self.paused.contains(&c.id) {
                    "暂停"
                } else {
                    "开启"
                };
                match tracks.get(&c.ssrc) {
                    Some((t, n)) => format!(
                        "{} {state} {n} 包 {}ms 前",
                        c.peer_id,
                        now.saturating_duration_since(*t).as_millis()
                    ),
                    None => format!("{} {state} 无包", c.peer_id),
                }
            })
            .collect();
        format!("音轨 [{}]", items.join("；"))
    }

    pub(super) fn offer(&mut self, params: &JoinParams) -> AppResult<String> {
        self.version += 1;
        sdp::recv_offer(&params.recv, &self.consumers, self.version)
    }

    /// 处理服务端请求，收到新音轨时返回新的拉流 offer。
    pub(super) fn on_request(
        &mut self,
        method: &str,
        data: &Value,
        params: &JoinParams,
    ) -> Option<AppResult<String>> {
        if method != "newConsumer" {
            return None;
        }
        let c = match Consumer::parse(data) {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        if self.consumers.iter().any(|x| x.id == c.id) {
            return None;
        }
        self.owners.insert(c.id.clone(), c.peer_id.clone());
        // 加入时对方麦克风可能已暂停。
        let b = |k: &str| data.get(k).and_then(Value::as_bool).unwrap_or(false);
        let paused = b("paused") || b("producerPaused");
        if paused {
            self.paused.insert(c.id.clone());
        }
        log::info!("语音新音轨 成员 {} 暂停 {paused}", c.peer_id);
        self.mix(|m| m.add_track(c.ssrc, &c.peer_id));
        self.consumers.push(c);
        Some(self.offer(params))
    }

    /// 处理服务端通知，音轨关闭时返回新的拉流 offer。
    pub(super) fn on_notification(
        &mut self,
        method: &str,
        data: &Value,
        params: &JoinParams,
        sink: &Sink,
    ) -> Option<AppResult<String>> {
        let consumer = || data.get("consumerId").and_then(Value::as_str);
        let peer = || id_of(data.get("peerId"));
        let patch = |id: String| PeerPatch {
            id,
            ..PeerPatch::default()
        };
        match method {
            "newPeer" => {
                if let Some(p) = Peer::parse(data) {
                    sink(VoiceEvent::PeerJoined { peer: p });
                }
            }
            "peerClosed" => {
                if let Some(id) = peer() {
                    // 成员离开时移除其全部音轨。
                    self.mix(|m| m.remove_peer(&id));
                    sink(VoiceEvent::PeerLeft { id });
                }
            }
            "peerInfoChanged" => {
                if let Some(p) = PeerPatch::parse(data) {
                    sink(VoiceEvent::PeerChanged { patch: p });
                }
            }
            "pauseHeadset" | "resumeHeadset" => {
                if let Some(id) = peer() {
                    sink(VoiceEvent::PeerChanged {
                        patch: PeerPatch {
                            headset_off: Some(method == "pauseHeadset"),
                            ..patch(id)
                        },
                    });
                }
            }
            "consumerPaused" | "consumerResumed" => {
                let owner = consumer().and_then(|c| self.owners.get(c));
                log::debug!("语音 {method} 成员 {owner:?}");
                if let Some(c) = consumer() {
                    if method == "consumerPaused" {
                        self.paused.insert(c.to_owned());
                    } else {
                        self.paused.remove(c);
                    }
                }
                if method == "consumerPaused"
                    && let Some(ssrc) = consumer().and_then(|c| self.ssrc_of(c))
                {
                    self.mix(|m| m.clear_track(ssrc));
                }
                let owner = consumer().and_then(|c| self.owners.get(c)).cloned();
                if let Some(id) = owner.filter(|s| !s.is_empty()).or_else(peer) {
                    sink(VoiceEvent::PeerChanged {
                        patch: PeerPatch {
                            mic_off: Some(method == "consumerPaused"),
                            ..patch(id)
                        },
                    });
                }
            }
            "consumerClosed" => {
                let id = consumer()?;
                let c = self
                    .consumers
                    .iter_mut()
                    .find(|c| c.id == id && !c.closed)?;
                c.closed = true;
                log::info!("语音音轨关闭 成员 {}", c.peer_id);
                let ssrc = c.ssrc;
                self.owners.remove(id);
                self.paused.remove(id);
                self.mix(|m| m.remove_track(ssrc));
                return Some(self.offer(params));
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kook::voice::testing::*;
    use crate::kook::voice::*;

    #[test]
    fn quiet_tracks_skip_paused_and_closed() {
        use std::time::{Duration, Instant};
        let (sink, _) = recorder();
        let mut room = Room::default();
        let p = params();
        let t0 = Instant::now();
        let after = Duration::from_secs(10);
        let (early, late) = (t0 + Duration::from_secs(9), t0 + Duration::from_secs(11));
        let mut tracks = HashMap::new();
        room.on_request("newConsumer", &new_consumer("c1", "u1", 11), &p);
        // 从未收到包的音轨自连接建立起算。
        assert_eq!(
            room.quiet_tracks(&tracks, t0, early, after),
            Vec::<u32>::new()
        );
        assert_eq!(room.quiet_tracks(&tracks, t0, late, after), vec![11]);
        tracks.insert(11, (t0 + Duration::from_secs(5), 3));
        assert_eq!(
            room.quiet_tracks(&tracks, t0, late, after),
            Vec::<u32>::new()
        );
        tracks.insert(11, (t0, 3));
        assert_eq!(room.quiet_tracks(&tracks, t0, late, after), vec![11]);
        room.on_notification("consumerPaused", &json!({"consumerId": "c1"}), &p, &sink);
        assert_eq!(
            room.quiet_tracks(&tracks, t0, late, after),
            Vec::<u32>::new(),
            "暂停的音轨不记录"
        );
        room.on_notification("consumerResumed", &json!({"consumerId": "c1"}), &p, &sink);
        assert_eq!(room.quiet_tracks(&tracks, t0, late, after), vec![11]);
        room.on_notification("consumerClosed", &json!({"consumerId": "c1"}), &p, &sink);
        assert_eq!(
            room.quiet_tracks(&tracks, t0, late, after),
            Vec::<u32>::new(),
            "关闭的音轨不记录"
        );
        let mut paused = new_consumer("c2", "u2", 12);
        paused["producerPaused"] = json!(true);
        room.on_request("newConsumer", &paused, &p);
        assert_eq!(
            room.quiet_tracks(&tracks, t0, late, after),
            Vec::<u32>::new(),
            "以暂停状态加入的音轨不记录"
        );
    }

    #[test]
    fn link_meter_uses_down_loss_delta() {
        let mut m = LinkMeter::default();
        let down = |packets, lost| DownStats {
            packets,
            lost,
            ..DownStats::default()
        };
        let up = UpStats {
            score: 10,
            ..UpStats::default()
        };
        let q = |e: VoiceEvent| match e {
            VoiceEvent::Link { quality, .. } => quality,
            _ => None,
        };
        assert_eq!(
            q(m.event(Some(up), None, down(100, 50))),
            Some(Quality::Poor)
        );
        // 增量没有丢包。
        assert_eq!(
            q(m.event(Some(up), None, down(200, 50))),
            Some(Quality::Good)
        );
        assert_eq!(q(m.event(None, None, down(200, 50))), None);
    }

    #[test]
    fn room_tracks_consumers_and_member_events() {
        let p = params();
        let (sink, log) = recorder();
        let mut room = Room::default();
        let o1 = room
            .on_request("newConsumer", &new_consumer("c1", "u1", 1), &p)
            .unwrap()
            .unwrap();
        assert!(o1.contains("a=mid:0") && o1.contains("o=- 2 1 "));
        // 重复的 newConsumer 不生成新 offer。
        assert!(
            room.on_request("newConsumer", &new_consumer("c1", "u1", 1), &p)
                .is_none()
        );
        let o2 = room
            .on_request("newConsumer", &new_consumer("c2", "u2", 2), &p)
            .unwrap()
            .unwrap();
        assert!(o2.contains("a=group:BUNDLE 0 1") && o2.contains("o=- 2 2 "));
        assert!(room.on_request("other", &json!({}), &p).is_none());

        assert!(
            room.on_notification("consumerPaused", &json!({"consumerId": "c2"}), &p, &sink)
                .is_none()
        );
        let o3 = room
            .on_notification("consumerClosed", &json!({"consumerId": "c1"}), &p, &sink)
            .unwrap()
            .unwrap();
        assert!(o3.contains("a=group:BUNDLE 0 1") && o3.contains("a=inactive"));
        assert!(
            room.on_notification("consumerClosed", &json!({"consumerId": "c1"}), &p, &sink)
                .is_none(),
            "重复关闭不生成新 offer"
        );
        room.on_notification(
            "newPeer",
            &json!({"peerId": "u3", "device": "pc"}),
            &p,
            &sink,
        );
        room.on_notification("peerClosed", &json!({"peerId": "u3"}), &p, &sink);
        room.on_notification("pauseHeadset", &json!({"peerId": "u2"}), &p, &sink);
        room.on_notification("peerInfoChanged", &json!({"peerId": ""}), &p, &sink);

        let log = log.lock().unwrap();
        assert_eq!(
            log[0],
            VoiceEvent::PeerChanged {
                patch: PeerPatch {
                    id: "u2".into(),
                    mic_off: Some(true),
                    ..PeerPatch::default()
                }
            }
        );
        assert!(matches!(&log[1], VoiceEvent::PeerJoined { peer } if peer.id == "u3"));
        assert_eq!(log[2], VoiceEvent::PeerLeft { id: "u3".into() });
        assert!(
            matches!(&log[3], VoiceEvent::PeerChanged { patch } if patch.headset_off == Some(true))
        );
        assert_eq!(log.len(), 4, "无效的成员信息不产生事件");
    }

    #[test]
    fn room_adds_and_removes_mixer_tracks() {
        let p = params();
        let (sink, _log) = recorder();
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let mut room = Room {
            mixer: Some(mixer.clone()),
            ..Room::default()
        };
        for (c, u, ssrc) in [("c1", "u1", 1), ("c2", "u2", 2), ("c3", "u3", 3)] {
            room.on_request("newConsumer", &new_consumer(c, u, ssrc), &p);
        }
        let count = || mixer.lock().unwrap().track_count();
        assert_eq!(count(), 3, "每个音轨一条");
        room.on_notification("consumerPaused", &json!({"consumerId": "c1"}), &p, &sink);
        assert_eq!(count(), 3, "暂停不移除音轨");
        room.on_notification("consumerClosed", &json!({"consumerId": "c2"}), &p, &sink);
        assert_eq!(count(), 2);
        room.on_notification("peerClosed", &json!({"peerId": "u3"}), &p, &sink);
        assert_eq!(count(), 1, "成员离开移除其音轨");
    }
}
