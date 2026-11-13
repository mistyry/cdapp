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

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Response {
        id: u64,
        ok: bool,
        data: Value,
        error: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectKind {
    JoinFailed,
    Replaced,
    Kicked,
    Exited,
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

    #[must_use]
    pub fn may_retry(self) -> bool {
        matches!(self, Self::Other(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Request { method: String, data: Value },
    Notification { method: String, data: Value },
    Closed,
}

type Reply = oneshot::Sender<AppResult<Value>>;

#[derive(Default)]
pub struct Pending {
    map: HashMap<u64, Reply>,
    closed: bool,
    disconnect: Option<i64>,
}

impl Pending {
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

    pub fn resolve(&mut self, id: u64, r: AppResult<Value>) -> bool {
        self.map.remove(&id).is_some_and(|tx| tx.send(r).is_ok())
    }

    pub fn remove(&mut self, id: u64) {
        self.map.remove(&id);
    }

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

#[derive(Clone)]
pub struct Signal {
    out: mpsc::UnboundedSender<Message>,
    pending: Arc<Mutex<Pending>>,
    timeout: Duration,
}

impl Signal {
    /// # Errors
    ///
    /// 连接失败或超时时返回错误。
    pub async fn connect(url: &str) -> AppResult<(Self, mpsc::UnboundedReceiver<Inbound>)> {
        Self::connect_within(url, REQUEST_TIMEOUT).await
    }

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

    pub fn exit(&self) {
        let id = rand::random_range(1_000_000..10_000_000u64);
        let frame = json!({ "request": true, "id": id, "method": "exit", "data": {} });
        let _ = self.out.send(Message::text(frame.to_string()));
        let _ = self.out.send(Message::Close(None));
    }

    pub fn close(&self) {
        let _ = self.out.send(Message::Close(None));
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        lock(&self.pending).len()
    }

    #[must_use]
    pub fn disconnect_code(&self) -> Option<i64> {
        lock(&self.pending).disconnect
    }

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
