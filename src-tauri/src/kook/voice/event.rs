use super::{DownStats, MicStatus, Quality, Serialize, UpStats, Value};

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub avatar: String,
    pub device: String,
    pub mic_off: bool,
    pub headset_off: bool,
    pub permission_say: bool,
    pub server_mute: bool,
    pub server_deafen: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_voice: Option<u64>,
}

pub(super) fn id_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Peer {
    #[must_use]
    pub fn parse(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
        let b = |k: &str| v.get(k).and_then(Value::as_bool);
        let name = [s("displayName"), s("username")]
            .into_iter()
            .find(|n| !n.is_empty())
            .unwrap_or_default();
        Some(Self {
            id: id_of(v.get("peerId")).or_else(|| id_of(v.get("id")))?,
            name: name.to_owned(),
            avatar: s("avatar").to_owned(),
            device: s("device").to_owned(),
            mic_off: b("pausedProducer").unwrap_or(false),
            headset_off: b("pausedHeadset").unwrap_or(false),
            permission_say: b("permissionSay").unwrap_or(true),
            server_mute: b("isServerMute").unwrap_or(false),
            server_deafen: b("isServerMuteHeadset").unwrap_or(false),
            join_voice: v
                .pointer("/extra/decorations/join_voice")
                .and_then(|x| x.as_u64().or_else(|| x.as_str()?.parse().ok()))
                .filter(|&x| x > 0),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerPatch {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mic_off: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headset_off: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_say: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_mute: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_deafen: Option<bool>,
}

impl PeerPatch {
    #[must_use]
    pub fn parse(v: &Value) -> Option<Self> {
        let b = |k: &str| v.get(k).and_then(Value::as_bool);
        Some(Self {
            id: id_of(v.get("peerId")).or_else(|| id_of(v.get("id")))?,
            name: v
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            mic_off: b("pausedProducer"),
            headset_off: b("pausedHeadset"),
            permission_say: b("permissionSay"),
            server_mute: b("isServerMute"),
            server_deafen: b("isServerMuteHeadset"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Left,
    Replaced,
    Exited,
    Kicked,
    JoinFailed,
    Lost,
    Unauthorized,
    Password,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceEvent {
    State {
        phase: Phase,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<EndReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_in_ms: Option<u64>,
    },
    Joined {
        me: Peer,
        peers: Vec<Peer>,
    },
    PeerJoined {
        peer: Peer,
    },
    PeerLeft {
        id: String,
    },
    PeerChanged {
        patch: PeerPatch,
    },
    Mic {
        status: MicStatus,
        muted: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    MicLevel {
        level: u8,
    },
    Output {
        deafened: bool,
        active: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Speaking {
        ids: Vec<String>,
    },
    Link {
        #[serde(skip_serializing_if = "Option::is_none")]
        quality: Option<Quality>,
        #[serde(skip_serializing_if = "Option::is_none")]
        up: Option<UpStats>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rtt: Option<u32>,
        down: DownStats,
    },
}

impl VoiceEvent {
    #[must_use]
    pub fn state(phase: Phase, reason: Option<EndReason>, message: Option<String>) -> Self {
        Self::State {
            phase,
            reason,
            message,
            attempt: None,
            max: None,
            retry_in_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kook::voice::*;

    #[test]
    fn parses_join_voice_decoration() {
        let p =
            Peer::parse(&json!({"peerId": "1", "extra": {"decorations": {"join_voice": 10148}}}))
                .unwrap();
        assert_eq!(p.join_voice, Some(10148));
        let p = Peer::parse(&json!({"peerId": "1", "extra": {"decorations": {"join_voice": "0"}}}))
            .unwrap();
        assert_eq!(p.join_voice, None);
        assert_eq!(
            Peer::parse(&json!({"peerId": "1"})).unwrap().join_voice,
            None
        );
    }

    #[test]
    fn parses_peers() {
        let p = Peer::parse(
            &json!({"peerId": 123, "displayName": "", "username": "成员甲",
            "device": "pc", "pausedProducer": true, "permissionSay": false}),
        )
        .unwrap();
        assert_eq!(p.id, "123");
        assert_eq!(p.name, "成员甲");
        assert!(p.mic_off && !p.permission_say && !p.headset_off);
        assert!(Peer::parse(&json!({"peerId": ""})).is_none());
        assert!(
            Peer::parse(&json!({"id": "9"})).unwrap().permission_say,
            "缺少 permissionSay 时视为允许发言"
        );
        let patch = PeerPatch::parse(&json!({"peerId": "1", "pausedHeadset": true})).unwrap();
        assert_eq!(patch.headset_off, Some(true));
        assert_eq!(patch.mic_off, None);
        let v = serde_json::to_value(&patch).unwrap();
        assert_eq!(v, json!({"id": "1", "headsetOff": true}));
        assert!(PeerPatch::parse(&json!({"peerId": ""})).is_none());
    }

    #[test]
    fn events_serialize_for_frontend() {
        let v = serde_json::to_value(VoiceEvent::state(
            Phase::Disconnected,
            Some(EndReason::Replaced),
            None,
        ))
        .unwrap();
        assert_eq!(
            v,
            json!({"type": "state", "phase": "disconnected", "reason": "replaced"})
        );
        let v = serde_json::to_value(VoiceEvent::PeerLeft { id: "1".into() }).unwrap();
        assert_eq!(v, json!({"type": "peer_left", "id": "1"}));
        let v = serde_json::to_value(VoiceEvent::State {
            phase: Phase::Reconnecting,
            reason: None,
            message: None,
            attempt: Some(2),
            max: Some(3),
            retry_in_ms: Some(2000),
        })
        .unwrap();
        assert_eq!(
            v,
            json!({"type": "state", "phase": "reconnecting", "attempt": 2, "max": 3, "retry_in_ms": 2000})
        );
    }
}
