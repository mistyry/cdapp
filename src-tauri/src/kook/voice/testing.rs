//! 单元测试共用的构造函数。

use super::{Arc, JoinParams, Mutex, Sink, Value, VoiceEvent};
use serde_json::json;

pub(in crate::kook::voice) fn params() -> JoinParams {
    let t = json!({
        "id": "t", "iceParameters": {"usernameFragment": "u", "password": "p"},
        "iceCandidates": [{"foundation": "f", "ip": "192.0.2.1", "port": 1, "priority": 1, "protocol": "udp", "type": "host"}],
        "dtlsParameters": {"fingerprints": [{"algorithm": "sha-256", "value": "AA"}]},
    });
    JoinParams::parse(&json!({"pTransport": t, "cTransport": t})).unwrap()
}

pub(in crate::kook::voice) fn recorder() -> (Sink, Arc<Mutex<Vec<VoiceEvent>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    (Arc::new(move |e| l.lock().unwrap().push(e)), log)
}

pub(in crate::kook::voice) fn new_consumer(id: &str, peer: &str, ssrc: u32) -> Value {
    json!({"id": id, "peerId": peer, "producerId": "p", "kind": "audio",
        "rtpParameters": {"codecs": [{"payloadType": 100, "clockRate": 48000, "channels": 2, "parameters": {}}],
            "encodings": [{"ssrc": ssrc}], "rtcp": {"cname": "c"}}})
}
