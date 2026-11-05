//! 语音成员、连接阶段与发往前端的事件。

use super::{DownStats, MicStatus, Quality, Serialize, UpStats, Value};

/// 语音成员。
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub avatar: String,
    pub device: String,
    /// 麦克风已关闭。
    pub mic_off: bool,
    /// 耳机已关闭。
    pub headset_off: bool,
    pub permission_say: bool,
    pub server_mute: bool,
}

pub(super) fn id_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Peer {
    /// 解析信令中的成员信息，缺少成员 ID 时返回 None。
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
            // 服务端未下发时视为允许发言。
            permission_say: b("permissionSay").unwrap_or(true),
            server_mute: b("isServerMute").unwrap_or(false),
        })
    }
}

/// 成员信息的部分更新，只包含通知中出现的字段。
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
}

impl PeerPatch {
    /// 解析成员信息变化，缺少成员 ID 时返回 None。
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
        })
    }
}

/// 连接阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

/// 会话结束原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// 用户主动退出。
    Left,
    /// 同一账号在其他设备加入语音，本会话被替换。
    Replaced,
    /// 服务端通知正常退出。
    Exited,
    /// 服务端拒绝加入。
    JoinFailed,
    /// 连接中断，重建次数已用尽。
    Lost,
    /// 登录凭据失效。
    Unauthorized,
}

/// 发往前端的语音事件。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceEvent {
    /// 连接阶段变化。重连时附带当前次数、上限与等待时长。
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
    /// 加入成功，附带本人与房间内已有成员。
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
    /// 麦克风状态。
    Mic {
        status: MicStatus,
        muted: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// 麦克风输入电平，范围 0 至 100。
    MicLevel {
        level: u8,
    },
    /// 扬声器输出状态。
    /// `active` 表示播放设备已打开，`deafened` 表示耳机已关闭。
    Output {
        deafened: bool,
        active: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// 正在说话的成员 ID，变化时上报。
    Speaking {
        ids: Vec<String>,
    },
    /// 链路质量。
    /// `up` 来自服务端推流统计，`down` 来自本地抖动缓冲。
    Link {
        #[serde(skip_serializing_if = "Option::is_none")]
        quality: Option<Quality>,
        #[serde(skip_serializing_if = "Option::is_none")]
        up: Option<UpStats>,
        down: DownStats,
    },
}

impl VoiceEvent {
    /// 不带重连信息的阶段事件。
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
