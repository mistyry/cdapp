//! 语音 SDP 与 RTP 参数。
//! 服务端以 mediasoup 传输参数描述连接，本模块据此构造 ice-lite 的推流应答与拉流 offer，并从本地 offer 取出推流参数。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{AppError, AppResult};

pub const MID_URI: &str = "urn:ietf:params:rtp-hdrext:sdes:mid";
pub const LEVEL_URI: &str = "urn:ietf:params:rtp-hdrext:ssrc-audio-level";
/// RTP 头部扩展 ID，与推流参数中声明的一致。
pub const MID_ID: u8 = 1;
pub const LEVEL_ID: u8 = 2;
pub const OPUS_PT: u8 = 111;

/// quickJoin 返回的传输参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    pub id: String,
    pub ice_parameters: IceParameters,
    pub ice_candidates: Vec<Candidate>,
    pub dtls_parameters: DtlsParameters,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceParameters {
    pub username_fragment: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Candidate {
    /// 服务端可能下发字符串或数字。
    pub foundation: Value,
    pub ip: String,
    pub port: u16,
    pub priority: u64,
    pub protocol: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DtlsParameters {
    pub fingerprints: Vec<Fingerprint>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Fingerprint {
    pub algorithm: String,
    pub value: String,
}

/// 服务端下发的推流音频参数。
#[derive(Debug, Clone, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "default_ptime")]
    pub ptime: u32,
    #[serde(default)]
    pub usedtx: u32,
    #[serde(default)]
    pub maxaveragebitrate: u32,
}

fn default_ptime() -> u32 {
    60
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            ptime: default_ptime(),
            usedtx: 1,
            maxaveragebitrate: 0,
        }
    }
}

/// quickJoin 响应中的推流与拉流参数。
#[derive(Debug, Clone)]
pub struct JoinParams {
    pub send: Transport,
    pub recv: Transport,
    pub audio: AudioConfig,
}

impl JoinParams {
    /// # Errors
    ///
    /// 缺少传输参数、没有候选地址或缺少 sha-256 指纹时返回错误。
    pub fn parse(v: &Value) -> AppResult<Self> {
        let transport = |key: &str| -> AppResult<Transport> {
            let t = v
                .get(key)
                .ok_or_else(|| AppError::Format(format!("quickJoin 响应缺少 {key}")))?;
            let t: Transport = serde_json::from_value(t.clone())
                .map_err(|e| AppError::Format(format!("quickJoin {key} 格式错误：{e}")))?;
            if t.ice_candidates.is_empty() {
                return Err(AppError::Format(format!("quickJoin {key} 没有候选地址")));
            }
            sha256(&t)?;
            Ok(t)
        };
        let audio = v
            .pointer("/rtpConfig/audio")
            .and_then(|a| serde_json::from_value(a.clone()).ok())
            .unwrap_or_default();
        Ok(Self {
            send: transport("pTransport")?,
            recv: transport("cTransport")?,
            audio,
        })
    }
}

fn sha256(t: &Transport) -> AppResult<&str> {
    t.dtls_parameters
        .fingerprints
        .iter()
        .find(|f| f.algorithm.eq_ignore_ascii_case("sha-256"))
        .map(|f| f.value.as_str())
        .ok_or_else(|| AppError::Format("缺少 sha-256 指纹".into()))
}

fn transport_lines(t: &Transport, setup: &str, out: &mut Vec<String>) -> AppResult<()> {
    out.push(format!(
        "a=ice-ufrag:{}",
        t.ice_parameters.username_fragment
    ));
    out.push(format!("a=ice-pwd:{}", t.ice_parameters.password));
    out.push(format!("a=fingerprint:sha-256 {}", sha256(t)?));
    out.push(format!("a=setup:{setup}"));
    for c in &t.ice_candidates {
        let foundation = c
            .foundation
            .as_str()
            .map_or_else(|| c.foundation.to_string(), str::to_owned);
        let tcp = if c.protocol == "tcp" {
            " tcptype passive"
        } else {
            ""
        };
        out.push(format!(
            "a=candidate:{foundation} 1 {} {} {} {} typ {}{tcp}",
            c.protocol, c.priority, c.ip, c.port, c.kind
        ));
    }
    out.push("a=end-of-candidates".into());
    Ok(())
}

fn join(lines: &[String]) -> String {
    let mut s = lines.join("\r\n");
    s.push_str("\r\n");
    s
}

/// 构造推流连接的远端应答。
///
/// # Errors
///
/// 缺少 sha-256 指纹时返回错误。
pub fn send_answer(t: &Transport, mid: &str) -> AppResult<String> {
    let mut l: Vec<String> = [
        "v=0",
        "o=- 1 1 IN IP4 0.0.0.0",
        "s=-",
        "t=0 0",
        "a=ice-lite",
    ]
    .map(str::to_owned)
    .to_vec();
    l.push(format!("a=group:BUNDLE {mid}"));
    l.push(format!("m=audio 9 UDP/TLS/RTP/SAVPF {OPUS_PT}"));
    l.push("c=IN IP4 0.0.0.0".into());
    l.push(format!("a=mid:{mid}"));
    transport_lines(t, "passive", &mut l)?;
    l.push(format!("a=extmap:{MID_ID} {MID_URI}"));
    l.push(format!("a=extmap:{LEVEL_ID} {LEVEL_URI}"));
    l.push("a=recvonly".into());
    l.push("a=rtcp-mux".into());
    l.push(format!("a=rtpmap:{OPUS_PT} opus/48000/2"));
    l.push(format!("a=fmtp:{OPUS_PT} minptime=10;useinbandfec=1"));
    l.push(format!("a=rtcp-fb:{OPUS_PT} transport-cc"));
    Ok(join(&l))
}

/// 服务端下发的拉流音轨。
#[derive(Debug, Clone, PartialEq)]
pub struct Consumer {
    pub id: String,
    pub peer_id: String,
    pub producer_id: String,
    pub ssrc: u32,
    pub payload_type: u8,
    pub clock_rate: u32,
    pub channels: u16,
    /// 编码参数，按 `a=fmtp` 格式拼接。
    pub fmtp: String,
    pub cname: String,
    /// 已关闭的音轨在 offer 中保留为 inactive，保持 mid 顺序。
    pub closed: bool,
}

impl Consumer {
    /// # Errors
    ///
    /// 缺少 ID、RTP 参数、编码、SSRC 或 payload type 时返回错误。
    pub fn parse(data: &Value) -> AppResult<Self> {
        let s = |p: &str| data.pointer(p).and_then(Value::as_str).map(str::to_owned);
        let id = s("/id").ok_or_else(|| AppError::Format("newConsumer 缺少 id".into()))?;
        let rp = data
            .get("rtpParameters")
            .ok_or_else(|| AppError::Format("newConsumer 缺少 rtpParameters".into()))?;
        let codec = rp
            .pointer("/codecs/0")
            .ok_or_else(|| AppError::Format("newConsumer 缺少编码参数".into()))?;
        let ssrc = rp
            .pointer("/encodings/0/ssrc")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| AppError::Format("newConsumer 缺少 ssrc".into()))?;
        let payload_type = codec
            .get("payloadType")
            .and_then(Value::as_u64)
            .and_then(|v| u8::try_from(v).ok())
            .ok_or_else(|| AppError::Format("newConsumer 缺少 payloadType".into()))?;
        let fmtp = codec
            .get("parameters")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| {
                        let v = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                        format!("{k}={v}")
                    })
                    .collect::<Vec<_>>()
                    .join(";")
            })
            .unwrap_or_default();
        Ok(Self {
            peer_id: s("/peerId")
                .or_else(|| s("/appData/peerId"))
                .unwrap_or_default(),
            producer_id: s("/producerId").unwrap_or_default(),
            ssrc,
            payload_type,
            clock_rate: codec
                .get("clockRate")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(48000),
            channels: codec
                .get("channels")
                .and_then(Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .unwrap_or(2),
            fmtp,
            cname: rp
                .pointer("/rtcp/cname")
                .and_then(Value::as_str)
                .unwrap_or("kook")
                .to_owned(),
            closed: false,
            id,
        })
    }
}

/// 构造拉流连接的远端 offer，每个音轨一个媒体段。
/// `version` 每次递增，作为 SDP 会话版本。
///
/// # Errors
///
/// 没有音轨或缺少 sha-256 指纹时返回错误。
pub fn recv_offer(t: &Transport, consumers: &[Consumer], version: u64) -> AppResult<String> {
    if consumers.is_empty() {
        return Err(AppError::Other("没有可接收的音轨".into()));
    }
    let mids: Vec<String> = (0..consumers.len()).map(|i| i.to_string()).collect();
    let mut l = vec![
        "v=0".to_owned(),
        format!("o=- 2 {version} IN IP4 0.0.0.0"),
        "s=-".into(),
        "t=0 0".into(),
        "a=ice-lite".into(),
        format!("a=group:BUNDLE {}", mids.join(" ")),
    ];
    for (i, c) in consumers.iter().enumerate() {
        let pt = c.payload_type;
        l.push(format!("m=audio 9 UDP/TLS/RTP/SAVPF {pt}"));
        l.push("c=IN IP4 0.0.0.0".into());
        l.push(format!("a=mid:{i}"));
        transport_lines(t, "actpass", &mut l)?;
        l.push(if c.closed { "a=inactive" } else { "a=sendonly" }.into());
        l.push("a=rtcp-mux".into());
        l.push(format!(
            "a=rtpmap:{pt} opus/{}/{}",
            c.clock_rate, c.channels
        ));
        if !c.fmtp.is_empty() {
            l.push(format!("a=fmtp:{pt} {}", c.fmtp));
        }
        if !c.closed {
            l.push(format!("a=ssrc:{} cname:{}", c.ssrc, c.cname));
            l.push(format!("a=msid:{0} {0}", c.id));
        }
    }
    Ok(join(&l))
}

/// 本地推流 offer 中的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalOffer {
    pub mid: String,
    pub ssrc: u32,
    pub cname: String,
}

impl LocalOffer {
    /// 从本地 offer 读取 mid、SSRC 与 cname。
    ///
    /// # Errors
    ///
    /// 缺少 mid 或 SSRC 时返回错误。
    pub fn parse(sdp: &str) -> AppResult<Self> {
        let mut mid = None;
        let mut ssrc = None;
        for line in sdp.lines().map(str::trim) {
            if let Some(m) = line.strip_prefix("a=mid:") {
                mid.get_or_insert_with(|| m.to_owned());
            } else if let Some(rest) = line.strip_prefix("a=ssrc:")
                && let Some((n, attr)) = rest.split_once(' ')
                && let Some(cname) = attr.strip_prefix("cname:")
                && let Ok(n) = n.parse::<u32>()
            {
                ssrc.get_or_insert((n, cname.to_owned()));
            }
        }
        let (Some(mid), Some((ssrc, cname))) = (mid, ssrc) else {
            return Err(AppError::Other("本地 offer 缺少 mid 或 ssrc".into()));
        };
        Ok(Self { mid, ssrc, cname })
    }
}

/// initSpeaking 的推流 RTP 参数。
#[must_use]
pub fn rtp_parameters(o: &LocalOffer, audio: &AudioConfig) -> Value {
    json!({
        "mid": o.mid,
        "codecs": [{
            "mimeType": "audio/opus", "payloadType": OPUS_PT, "clockRate": 48000, "channels": 2,
            "parameters": {
                "minptime": 10, "useinbandfec": 1, "sprop-stereo": 0,
                "usedtx": audio.usedtx, "ptime": audio.ptime,
            },
            "rtcpFeedback": [{"type": "transport-cc", "parameter": ""}],
        }],
        "headerExtensions": [
            {"uri": MID_URI, "id": MID_ID, "encrypt": false, "parameters": {}},
            {"uri": LEVEL_URI, "id": LEVEL_ID, "encrypt": false, "parameters": {}},
        ],
        "encodings": [{"ssrc": o.ssrc, "dtx": false}],
        "rtcp": {"cname": o.cname, "reducedSize": true},
    })
}

/// initSpeaking 的 DTLS 参数。服务端只接受大写指纹。
#[must_use]
pub fn dtls(fingerprint: &str) -> Value {
    json!({
        "role": "client",
        "fingerprints": [{"algorithm": "sha-256", "value": fingerprint.to_ascii_uppercase()}],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport() -> Value {
        json!({
            "id": "t1",
            "iceParameters": {"iceLite": true, "usernameFragment": "uf", "password": "pw"},
            "iceCandidates": [
                {"foundation": "udpcandidate", "ip": "192.0.2.1", "port": 40000, "priority": 1_076_302_079_u64, "protocol": "udp", "type": "host"},
                {"foundation": "tcpcandidate", "ip": "192.0.2.1", "port": 40001, "priority": 1_076_276_479_u64, "protocol": "tcp", "type": "host"},
            ],
            "dtlsParameters": {"role": "auto", "fingerprints": [
                {"algorithm": "sha-1", "value": "AA"},
                {"algorithm": "sha-256", "value": "AB:CD"},
            ]},
            "sctpParameters": {},
        })
    }

    fn join_params() -> JoinParams {
        JoinParams::parse(&json!({
            "peerId": "p", "pTransport": transport(), "cTransport": transport(),
            "rtpConfig": {"audio": {"stereo": 0, "ptime": 60, "maxaveragebitrate": 32000, "usedtx": 1}, "video": {}},
        }))
        .unwrap()
    }

    fn consumer(id: &str, ssrc: u64) -> Value {
        json!({
            "peerId": "u1", "producerId": "p1", "id": id, "kind": "audio", "type": "simple",
            "rtpParameters": {
                "codecs": [{"mimeType": "audio/opus", "payloadType": 100, "clockRate": 48000, "channels": 2,
                    "parameters": {"minptime": 10, "useinbandfec": 1}, "rtcpFeedback": []}],
                "headerExtensions": [], "encodings": [{"ssrc": ssrc}],
                "rtcp": {"cname": "cn", "reducedSize": true, "mux": true}, "mid": "0",
            },
        })
    }

    #[test]
    fn parses_join_params() {
        let p = join_params();
        assert_eq!(p.send.ice_candidates.len(), 2);
        assert_eq!(p.audio.ptime, 60);
        assert_eq!(p.audio.maxaveragebitrate, 32000);
        assert!(JoinParams::parse(&json!({"pTransport": transport()})).is_err());
        let mut no_fp = transport();
        no_fp["dtlsParameters"]["fingerprints"] = json!([]);
        assert!(
            JoinParams::parse(&json!({"pTransport": no_fp, "cTransport": transport()})).is_err()
        );
    }

    #[test]
    fn send_answer_is_ice_lite_passive_with_extensions() {
        let a = send_answer(&join_params().send, "0").unwrap();
        assert!(a.ends_with("\r\n"));
        let lines: Vec<&str> = a.lines().collect();
        for want in [
            "a=ice-lite",
            "a=group:BUNDLE 0",
            "a=mid:0",
            "a=setup:passive",
            "a=fingerprint:sha-256 AB:CD",
            "a=extmap:1 urn:ietf:params:rtp-hdrext:sdes:mid",
            "a=extmap:2 urn:ietf:params:rtp-hdrext:ssrc-audio-level",
            "a=recvonly",
            "a=rtpmap:111 opus/48000/2",
            "a=candidate:udpcandidate 1 udp 1076302079 192.0.2.1 40000 typ host",
            "a=candidate:tcpcandidate 1 tcp 1076276479 192.0.2.1 40001 typ host tcptype passive",
        ] {
            assert!(lines.contains(&want), "缺少 {want}");
        }
    }

    #[test]
    fn recv_offer_lists_consumers_and_inactive_sections() {
        let t = join_params().recv;
        let mut cs = vec![
            Consumer::parse(&consumer("c1", 11)).unwrap(),
            Consumer::parse(&consumer("c2", 22)).unwrap(),
        ];
        assert_eq!(cs[0].peer_id, "u1");
        assert_eq!(cs[0].fmtp, "minptime=10;useinbandfec=1");
        cs[0].closed = true;
        let o = recv_offer(&t, &cs, 3).unwrap();
        assert!(o.contains("o=- 2 3 IN IP4"));
        assert!(o.contains("a=group:BUNDLE 0 1"));
        assert!(o.contains("a=setup:actpass"));
        assert!(o.contains("a=rtpmap:100 opus/48000/2"));
        assert!(o.contains("a=fmtp:100 minptime=10;useinbandfec=1"));
        // 已关闭的音轨保留为 inactive 段。
        assert_eq!(o.matches("a=inactive").count(), 1);
        assert_eq!(o.matches("a=sendonly").count(), 1);
        assert!(!o.contains("a=ssrc:11 "));
        assert!(o.contains("a=ssrc:22 cname:cn"));
        assert!(o.contains("a=msid:c2 c2"));
        assert!(recv_offer(&t, &[], 1).is_err());
        assert!(Consumer::parse(&json!({"id": "x"})).is_err());
    }

    #[test]
    fn parses_local_offer_and_builds_init_params() {
        let sdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=msid:s t\r\n\
                   a=ssrc:12345 msid:s t\r\na=ssrc:12345 cname:abc\r\n";
        let o = LocalOffer::parse(sdp).unwrap();
        assert_eq!(
            o,
            LocalOffer {
                mid: "0".into(),
                ssrc: 12345,
                cname: "abc".into()
            }
        );
        assert!(LocalOffer::parse("v=0\r\na=mid:0\r\n").is_err());
        let rp = rtp_parameters(&o, &AudioConfig::default());
        assert_eq!(rp["mid"], "0");
        assert_eq!(rp["encodings"][0]["ssrc"], 12345);
        assert_eq!(rp["headerExtensions"][0]["uri"], MID_URI);
        assert_eq!(rp["headerExtensions"][1]["id"], 2);
        assert_eq!(rp["codecs"][0]["parameters"]["ptime"], 60);
    }

    #[test]
    fn dtls_fingerprint_is_uppercase_client() {
        let d = dtls("ab:cd:0f");
        assert_eq!(d["role"], "client");
        assert_eq!(d["fingerprints"][0]["value"], "AB:CD:0F");
        assert_eq!(d["fingerprints"][0]["algorithm"], "sha-256");
    }
}
