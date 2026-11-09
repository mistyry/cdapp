//! 语音信令连接。
//! 信令为 mediasoup 风格的 JSON 请求与响应。请求按 ID 等待响应，超时或连接关闭时返回错误。
//! 服务端发起的请求立即回复确认，再转交会话处理。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::error::{AppError, AppResult};
use crate::kook::{BASE, USER_AGENT};

/// 信令请求的超时时长。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 一帧信令。
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Response {
        id: u64,
        ok: bool,
        data: Value,
        /// 失败原因。
        error: Option<String>,
        /// 失败时的错误码，成功时为 0。
        code: i64,
    },
    Request {
        id: Value,
        method: String,
        data: Value,
    },
    Notification {
        method: String,
        data: Value,
    },
}

impl Frame {
    /// 解析一帧信令，格式不符时返回 None。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let msg: Value = serde_json::from_str(text).ok()?;
        let flag = |k: &str| msg.get(k).and_then(Value::as_bool) == Some(true);
        let data = msg.get("data").cloned().unwrap_or(Value::Null);
        let method = || msg.get("method").and_then(Value::as_str).map(str::to_owned);
        if flag("response") {
            let ok = flag("ok");
            let error = (!ok).then(|| {
                ["errorReason", "error", "reason"]
                    .iter()
                    .find_map(|k| msg.get(*k).and_then(Value::as_str))
                    .unwrap_or("未知错误")
                    .to_owned()
            });
            let code = if ok {
                0
            } else {
                ["errorCode", "code"]
                    .iter()
                    .find_map(|k| msg.get(*k).and_then(Value::as_i64))
                    .unwrap_or(0)
            };
            return Some(Self::Response {
                id: msg.get("id")?.as_u64()?,
                ok,
                data,
                error,
                code,
            });
        }
        if flag("request") {
            return Some(Self::Request {
                id: msg.get("id")?.clone(),
                method: method()?,
                data,
            });
        }
        if flag("notification") {
            return Some(Self::Notification {
                method: method()?,
                data,
            });
        }
        None
    }
}

/// disconnect 通知的错误码分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectKind {
    /// 41100，加入失败。
    JoinFailed,
    /// 41101，被同账号的新会话替换。
    Replaced,
    /// 41106，被管理员移出语音频道。
    Kicked,
    /// 41110，正常退出。
    Exited,
    /// 其他错误码。
    Other(i64),
}

impl DisconnectKind {
    #[must_use]
    pub fn from_code(code: i64) -> Self {
        match code {
            41100 => Self::JoinFailed,
            41101 => Self::Replaced,
            41106 => Self::Kicked,
            41110 => Self::Exited,
            c => Self::Other(c),
        }
    }

    /// 是否可以重建连接。
    #[must_use]
    pub fn may_retry(self) -> bool {
        matches!(self, Self::Other(_))
    }
}

/// 转交会话的信令。
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Request {
        method: String,
        data: Value,
    },
    Notification {
        method: String,
        data: Value,
    },
    /// 连接已关闭。
    Closed,
}

type Reply = oneshot::Sender<AppResult<Value>>;

/// 等待响应的请求。
#[derive(Default)]
pub struct Pending {
    map: HashMap<u64, Reply>,
    closed: bool,
    /// 最后收到的 disconnect 错误码。
    disconnect: Option<i64>,
}

impl Pending {
    /// 登记请求，返回请求 ID。
    ///
    /// # Errors
    ///
    /// 连接已关闭时返回错误。
    pub fn insert(&mut self, reply: Reply) -> AppResult<u64> {
        if self.closed {
            return Err(closed_error());
        }
        let mut id = rand::random_range(1_000_000..10_000_000u64);
        while self.map.contains_key(&id) {
            id = rand::random_range(1_000_000..10_000_000u64);
        }
        self.map.insert(id, reply);
        Ok(id)
    }

    /// 以结果答复请求，请求不存在时返回 false。
    pub fn resolve(&mut self, id: u64, r: AppResult<Value>) -> bool {
        self.map.remove(&id).is_some_and(|tx| tx.send(r).is_ok())
    }

    /// 移除请求，不答复。
    pub fn remove(&mut self, id: u64) {
        self.map.remove(&id);
    }

    /// 关闭连接，所有等待中的请求返回错误。
    pub fn close(&mut self) {
        self.closed = true;
        for (_, tx) in self.map.drain() {
            let _ = tx.send(Err(closed_error()));
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn closed_error() -> AppError {
    AppError::Other("语音信令连接已关闭".into())
}

/// 信令连接句柄，可以复制。
#[derive(Clone)]
pub struct Signal {
    out: mpsc::UnboundedSender<Message>,
    pending: Arc<Mutex<Pending>>,
    timeout: Duration,
}

impl Signal {
    /// 连接信令服务器。
    ///
    /// # Errors
    ///
    /// 连接失败或超时时返回错误。
    pub async fn connect(url: &str) -> AppResult<(Self, mpsc::UnboundedReceiver<Inbound>)> {
        Self::connect_within(url, REQUEST_TIMEOUT).await
    }

    /// 在 `limit` 内连接信令服务器。
    /// 请求头使用桌面端 User-Agent，服务端据此判定设备类型。
    ///
    /// # Errors
    ///
    /// 连接失败或超时时返回错误。
    pub async fn connect_within(
        url: &str,
        limit: Duration,
    ) -> AppResult<(Self, mpsc::UnboundedReceiver<Inbound>)> {
        let mut req = url.into_client_request()?;
        let h = req.headers_mut();
        h.insert("user-agent", HeaderValue::from_static(USER_AGENT));
        h.insert("origin", HeaderValue::from_static(BASE));
        let (ws, _) = tokio::time::timeout(limit, tokio_tungstenite::connect_async(req))
            .await
            .map_err(|_| AppError::Other("连接语音信令超时".into()))??;
        Ok(Self::spawn(ws, REQUEST_TIMEOUT))
    }

    /// 在已建立的 WebSocket 上启动收发任务。
    pub fn spawn<S>(ws: S, timeout: Duration) -> (Self, mpsc::UnboundedReceiver<Inbound>)
    where
        S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + Send
            + Unpin
            + 'static,
    {
        let (out, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let pending = Arc::new(Mutex::new(Pending::default()));
        tauri::async_runtime::spawn(pump(ws, out_rx, out.clone(), in_tx, pending.clone()));
        (
            Self {
                out,
                pending,
                timeout,
            },
            in_rx,
        )
    }

    /// 发送请求并等待响应。
    ///
    /// # Errors
    ///
    /// 连接关闭、超时或服务端拒绝时返回错误。
    pub async fn request(&self, method: &str, data: Value) -> AppResult<Value> {
        let (tx, rx) = oneshot::channel();
        let id = lock(&self.pending).insert(tx)?;
        let frame = json!({ "request": true, "id": id, "method": method, "data": data });
        if self.out.send(Message::text(frame.to_string())).is_err() {
            lock(&self.pending).remove(id);
            return Err(closed_error());
        }
        if let Ok(r) = tokio::time::timeout(self.timeout, rx).await {
            r.unwrap_or_else(|_| Err(closed_error()))
        } else {
            lock(&self.pending).remove(id);
            Err(AppError::Other(format!("信令请求 {method} 超时")))
        }
    }

    /// 发送 exit 后关闭连接，不等待响应。服务端以 disconnect 通知后关闭。
    pub fn exit(&self) {
        let id = rand::random_range(1_000_000..10_000_000u64);
        let frame = json!({ "request": true, "id": id, "method": "exit", "data": {} });
        let _ = self.out.send(Message::text(frame.to_string()));
        let _ = self.out.send(Message::Close(None));
    }

    /// 关闭连接，不发送 exit。
    pub fn close(&self) {
        let _ = self.out.send(Message::Close(None));
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        lock(&self.pending).len()
    }

    /// 最后收到的 disconnect 错误码。
    #[must_use]
    pub fn disconnect_code(&self) -> Option<i64> {
        lock(&self.pending).disconnect
    }

    /// 连接是否已关闭。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock(&self.pending).closed
    }
}

fn lock(p: &Mutex<Pending>) -> std::sync::MutexGuard<'_, Pending> {
    p.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn pump<S>(
    ws: S,
    mut out_rx: mpsc::UnboundedReceiver<Message>,
    out: mpsc::UnboundedSender<Message>,
    in_tx: mpsc::UnboundedSender<Inbound>,
    pending: Arc<Mutex<Pending>>,
) where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin,
{
    let (mut tx, mut rx) = ws.split();
    loop {
        tokio::select! {
            m = out_rx.recv() => {
                let Some(m) = m else { break };
                let close = matches!(m, Message::Close(_));
                if tx.send(m).await.is_err() || close {
                    break;
                }
            }
            frame = rx.next() => {
                let text = match frame {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => continue,
                };
                match Frame::parse(&text) {
                    Some(Frame::Response { id, ok, data, error, code }) => {
                        // 服务端拒绝时返回 API 错误，保留错误码。
                        let r = if ok {
                            Ok(data)
                        } else {
                            Err(AppError::Api { code, message: format!("语音服务拒绝请求：{}", error.unwrap_or_default()) })
                        };
                        lock(&pending).resolve(id, r);
                    }
                    Some(Frame::Request { id, method, data }) => {
                        let ack = json!({ "response": true, "id": id, "ok": true, "data": {} });
                        let _ = out.send(Message::text(ack.to_string()));
                        let _ = in_tx.send(Inbound::Request { method, data });
                    }
                    Some(Frame::Notification { method, data }) => {
                        if method == "disconnect" {
                            lock(&pending).disconnect =
                                Some(data.get("errorCode").and_then(Value::as_i64).unwrap_or(0));
                        }
                        let _ = in_tx.send(Inbound::Notification { method, data });
                    }
                    None => {}
                }
            }
        }
    }
    let _ = tx.close().await;
    lock(&pending).close();
    let _ = in_tx.send(Inbound::Closed);
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn connect_times_out_when_server_never_handshakes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // 接受 TCP 连接但不完成握手。
        let hold = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(sock);
        });
        let r = tokio::time::timeout(
            Duration::from_secs(5),
            Signal::connect_within(&format!("ws://{addr}/"), Duration::from_millis(200)),
        )
        .await
        .expect("连接未按时超时");
        let e = r.err().expect("应返回错误");
        assert!(!matches!(e, AppError::Api { .. }), "{e}");
        hold.abort();
    }
    use super::*;

    #[test]
    fn parses_frames() {
        assert_eq!(
            Frame::parse(r#"{"response":true,"id":12,"ok":true,"data":{"a":1}}"#),
            Some(Frame::Response {
                id: 12,
                ok: true,
                data: json!({"a":1}),
                error: None,
                code: 0
            })
        );
        let Some(Frame::Response { ok, error, .. }) = Frame::parse(
            r#"{"response":true,"id":1,"ok":false,"errorReason":"unknown request.method"}"#,
        ) else {
            panic!("应为响应");
        };
        assert!(!ok);
        assert_eq!(error.as_deref(), Some("unknown request.method"));
        assert_eq!(
            Frame::parse(r#"{"request":true,"id":"x","method":"newConsumer","data":{}}"#),
            Some(Frame::Request {
                id: json!("x"),
                method: "newConsumer".into(),
                data: json!({})
            })
        );
        assert_eq!(
            Frame::parse(r#"{"notification":true,"method":"peerClosed","data":{"peerId":"1"}}"#),
            Some(Frame::Notification {
                method: "peerClosed".into(),
                data: json!({"peerId":"1"})
            })
        );
        assert_eq!(Frame::parse("not json"), None);
        assert_eq!(Frame::parse(r#"{"notification":true}"#), None);
        assert_eq!(Frame::parse(r#"{"response":true,"ok":true}"#), None);
    }

    #[test]
    fn classifies_disconnect_codes() {
        assert_eq!(DisconnectKind::from_code(41100), DisconnectKind::JoinFailed);
        assert_eq!(DisconnectKind::from_code(41101), DisconnectKind::Replaced);
        assert_eq!(DisconnectKind::from_code(41110), DisconnectKind::Exited);
        assert_eq!(DisconnectKind::from_code(41106), DisconnectKind::Kicked);
        assert!(!DisconnectKind::Kicked.may_retry());
        assert_eq!(DisconnectKind::from_code(1), DisconnectKind::Other(1));
        assert!(!DisconnectKind::Replaced.may_retry());
        assert!(!DisconnectKind::Exited.may_retry());
        assert!(!DisconnectKind::JoinFailed.may_retry());
        assert!(DisconnectKind::Other(50000).may_retry());
    }

    #[test]
    fn pending_close_fails_all_and_rejects_new() {
        let mut p = Pending::default();
        let (a, mut ra) = oneshot::channel();
        let (b, mut rb) = oneshot::channel();
        let ia = p.insert(a).unwrap();
        let ib = p.insert(b).unwrap();
        assert_ne!(ia, ib);
        assert!(p.resolve(ia, Ok(json!(1))));
        assert!(!p.resolve(ia, Ok(json!(2))), "重复答复无效");
        assert_eq!(ra.try_recv().unwrap().unwrap(), json!(1));
        p.close();
        assert!(rb.try_recv().unwrap().is_err());
        assert!(p.is_empty());
        let (c, _) = oneshot::channel();
        assert!(p.insert(c).is_err());
    }

    /// 建立本地 WebSocket 连接，返回服务端与客户端两端。
    async fn pair() -> (
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        Signal,
        mpsc::UnboundedReceiver<Inbound>,
    ) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = l.accept().await.unwrap();
            tokio_tungstenite::accept_async(tcp).await.unwrap()
        });
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let (sig, rx) = Signal::spawn(ws, Duration::from_millis(200));
        (server.await.unwrap(), sig, rx)
    }

    async fn next_text(s: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Value {
        loop {
            if let Some(Ok(Message::Text(t))) = s.next().await {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn request_timeout_removes_pending() {
        let (_server, sig, _rx) = pair().await;
        let e = sig
            .request("getProducerStats", json!({}))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("超时"));
        assert_eq!(sig.pending(), 0);
    }

    #[tokio::test]
    async fn close_fails_pending_and_replies_server_requests() {
        let (mut server, sig, mut rx) = pair().await;
        let s2 = sig.clone();
        let req = tokio::spawn(async move { s2.request("pauseProducer", json!({})).await });
        let sent = next_text(&mut server).await;
        assert_eq!(sent["method"], "pauseProducer");
        // 服务端请求立即确认。
        server
            .send(Message::text(
                json!({"request":true,"id":7,"method":"newConsumer","data":{"id":"c"}}).to_string(),
            ))
            .await
            .unwrap();
        let ack = next_text(&mut server).await;
        assert_eq!(ack, json!({"response":true,"id":7,"ok":true,"data":{}}));
        assert_eq!(
            rx.recv().await,
            Some(Inbound::Request {
                method: "newConsumer".into(),
                data: json!({"id":"c"})
            })
        );
        // 连接关闭时等待中的请求失败。
        server.close(None).await.unwrap();
        let e = req.await.unwrap().unwrap_err();
        assert!(e.to_string().contains("已关闭"));
        assert_eq!(rx.recv().await, Some(Inbound::Closed));
        assert!(sig.request("x", json!({})).await.is_err());
    }

    #[tokio::test]
    async fn exit_does_not_wait_for_reply() {
        let (mut server, sig, mut rx) = pair().await;
        let started = std::time::Instant::now();
        sig.exit();
        assert!(started.elapsed() < Duration::from_millis(50));
        assert_eq!(next_text(&mut server).await["method"], "exit");
        assert_eq!(rx.recv().await, Some(Inbound::Closed));
    }

    #[tokio::test]
    async fn rejection_is_api_error_and_disconnect_code_is_kept() {
        let (mut server, sig, mut rx) = pair().await;
        let s2 = sig.clone();
        let req = tokio::spawn(async move { s2.request("quickJoin", json!({})).await });
        let sent = next_text(&mut server).await;
        server
            .send(Message::text(
                json!({"response":true,"id":sent["id"],"ok":false,"errorCode":403,"errorReason":"无权限"})
                    .to_string(),
            ))
            .await
            .unwrap();
        let e = req.await.unwrap().unwrap_err();
        assert!(matches!(e, AppError::Api { code: 403, .. }), "{e:?}");
        assert_eq!(sig.disconnect_code(), None);
        server
            .send(Message::text(
                json!({"notification":true,"method":"disconnect","data":{"errorCode":41101}})
                    .to_string(),
            ))
            .await
            .unwrap();
        server.close(None).await.unwrap();
        while rx.recv().await != Some(Inbound::Closed) {}
        assert_eq!(sig.disconnect_code(), Some(41101));
        assert!(sig.is_closed());
    }
}
