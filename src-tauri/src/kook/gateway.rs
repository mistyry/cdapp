use std::future::Future;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use flate2::read::ZlibDecoder;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};
use tokio::time::{Instant, sleep_until, timeout};
use tokio_tungstenite::tungstenite::{self, Message};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub connect_timeout: Duration,
    pub hello_timeout: Duration,
    pub heartbeat: Duration,
    pub pong_timeout: Duration,
    pub resume_timeout: Duration,
    pub min_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            hello_timeout: Duration::from_secs(10),
            heartbeat: Duration::from_secs(15),
            pong_timeout: Duration::from_secs(10),
            resume_timeout: Duration::from_secs(30),
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadyKind {
    New,
    Resumed,
    Rebuilt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayEvent {
    Ready { session_id: String, kind: ReadyKind },
    Event(Value),
    Disconnected { reason: String, retry_in: Duration },
    Expired(String),
}

struct Session {
    url: String,
    id: String,
    sn: u64,
}

impl Session {
    fn resume_url(&self) -> String {
        let sep = if self.url.contains('?') { '&' } else { '?' };
        format!(
            "{}{sep}resume=1&sn={}&session_id={}",
            self.url, self.sn, self.id
        )
    }
}

fn decode(msg: Message) -> Option<Value> {
    match msg {
        Message::Binary(b) => {
            let mut s = String::new();
            ZlibDecoder::new(&b[..]).read_to_string(&mut s).ok()?;
            serde_json::from_str(&s).ok()
        }
        Message::Text(t) => serde_json::from_str(&t).ok(),
        _ => None,
    }
}

fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(" <- ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

fn fmt_dur(d: Option<Duration>) -> String {
    d.map_or_else(|| "-".into(), |d| format!("{}ms", d.as_millis()))
}

fn redact(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

async fn until(t: Option<Instant>) {
    match t {
        Some(t) => sleep_until(t).await,
        None => std::future::pending().await,
    }
}

#[derive(Default)]
struct Reorder {
    buf: std::collections::BTreeMap<u64, Value>,
    stale: bool,
}

const REORDER_LIMIT: usize = 256;

impl Reorder {
    fn push<F: FnMut(GatewayEvent)>(&mut self, n: u64, d: Value, sn: &mut u64, emit: &mut F) {
        if n <= *sn || self.buf.contains_key(&n) {
            return;
        }
        if *sn == 0 && self.buf.is_empty() {
            *sn = n - 1;
        }
        self.buf.insert(n, d);
        self.drain(sn, emit);
        if self.buf.len() > REORDER_LIMIT {
            self.skip(sn, emit);
        }
    }

    fn drain<F: FnMut(GatewayEvent)>(&mut self, sn: &mut u64, emit: &mut F) {
        while let Some(d) = self.buf.remove(&(*sn + 1)) {
            *sn += 1;
            emit(GatewayEvent::Event(d));
        }
        if self.buf.is_empty() {
            self.stale = false;
        }
    }

    fn skip<F: FnMut(GatewayEvent)>(&mut self, sn: &mut u64, emit: &mut F) {
        if let Some((&first, _)) = self.buf.first_key_value() {
            log::warn!("网关事件序号缺失 {}..{first}，跳过", *sn + 1);
            *sn = first - 1;
            self.drain(sn, emit);
        }
    }

    fn flush_stale<F: FnMut(GatewayEvent)>(&mut self, sn: &mut u64, emit: &mut F) {
        if self.stale {
            while !self.buf.is_empty() {
                self.skip(sn, emit);
            }
        }
        self.stale = !self.buf.is_empty();
    }
}

#[derive(Default)]
struct Outcome {
    opened: Option<Instant>,
    last_frame: Option<Instant>,
    stable: bool,
    immediate: bool,
}

pub async fn run<U, Fut, F>(
    mut fetch_url: U,
    cfg: Config,
    mut stop: watch::Receiver<bool>,
    wake: Arc<Notify>,
    mut emit: F,
) where
    U: FnMut() -> Fut + Send,
    Fut: Future<Output = AppResult<String>> + Send,
    F: FnMut(GatewayEvent) + Send,
{
    let mut session: Option<Session> = None;
    let mut had_session = false;
    let mut backoff = cfg.min_backoff;
    let mut last_immediate = false;
    while !*stop.borrow() {
        let mut out = Outcome::default();
        let result = tokio::select! {
            r = connect_once(&mut fetch_url, &cfg, &mut session, &mut had_session, &mut out, &mut emit) => r,
            _ = stop.changed() => break,
        };
        let lived = out.opened.map(|t| t.elapsed());
        let idle = out.last_frame.map(|t| t.elapsed());
        let err = match result {
            Err(AppError::Unauthorized(m)) => {
                log::warn!("网关凭据失效");
                emit(GatewayEvent::Expired(m));
                break;
            }
            Err(e) => e,
            Ok(()) => AppError::Other("连接结束".into()),
        };
        if out.stable {
            backoff = cfg.min_backoff;
        }
        let immediate = out.immediate && !last_immediate;
        last_immediate = immediate;
        let delay = if immediate {
            Duration::ZERO
        } else {
            let d = backoff;
            backoff = (backoff * 2).min(cfg.max_backoff);
            d
        };
        log::warn!(
            "网关断开：{}；连接持续 {}，距最后一帧 {}，{}ms 后重连",
            chain(&err),
            fmt_dur(lived),
            fmt_dur(idle),
            delay.as_millis(),
        );
        emit(GatewayEvent::Disconnected {
            reason: err.to_string(),
            retry_in: delay,
        });
        if delay.is_zero() {
            continue;
        }
        let waited = std::time::Instant::now();
        tokio::select! {
            () = tokio::time::sleep(delay) => {
                let real = waited.elapsed();
                if real > delay + Duration::from_secs(5) {
                    log::warn!("网关退避计划 {}ms，实际 {}ms", delay.as_millis(), real.as_millis());
                }
            }
            () = wake.notified() => {
                log::info!("网关收到唤醒，提前重连");
                backoff = cfg.min_backoff;
            }
            _ = stop.changed() => break,
        }
    }
}

async fn connect_once<U, Fut, F>(
    fetch_url: &mut U,
    cfg: &Config,
    session: &mut Option<Session>,
    had_session: &mut bool,
    out: &mut Outcome,
    emit: &mut F,
) -> AppResult<()>
where
    U: FnMut() -> Fut,
    Fut: Future<Output = AppResult<String>>,
    F: FnMut(GatewayEvent),
{
    let resuming = session.is_some();
    let url = match session.as_ref() {
        Some(s) => s.resume_url(),
        None => fetch_url().await?,
    };
    log::info!(
        "连接网关 {}{}",
        redact(&url),
        if resuming { "（续传）" } else { "" }
    );
    let started = Instant::now();
    let ws = match timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(url.as_str()),
    )
    .await
    {
        Err(_) => return Err(AppError::Other("连接网关超时".into())),
        Ok(Err(e)) => {
            if matches!(e, tungstenite::Error::Http(_)) && resuming {
                *session = None;
                out.immediate = true;
            }
            return Err(e.into());
        }
        Ok(Ok((ws, resp))) => {
            log::info!(
                "网关握手完成 {}，耗时 {}ms",
                resp.status(),
                started.elapsed().as_millis()
            );
            ws
        }
    };
    out.opened = Some(Instant::now());
    let (tx, mut rx) = ws.split();

    let hello = match timeout(cfg.hello_timeout, rx.next()).await {
        Err(_) => return Err(AppError::Other("等待网关 hello 超时".into())),
        Ok(None) => return Err(AppError::Other("网关未返回 hello".into())),
        Ok(Some(f)) => f?,
    };
    let hello = decode(hello).ok_or_else(|| AppError::Other("hello 帧无法解析".into()))?;
    let code = hello
        .pointer("/d/code")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    log::debug!("网关 hello：{hello}");
    if hello.get("s").and_then(Value::as_i64) != Some(1) || code != 0 {
        *session = None;
        out.immediate = resuming;
        return Err(AppError::Other(if resuming {
            format!("续传被拒绝 {code}")
        } else {
            format!("网关拒绝连接 {code}")
        }));
    }

    let mut resume_deadline = None;
    if resuming {
        resume_deadline = Some(Instant::now() + cfg.resume_timeout);
    } else {
        let id = hello
            .pointer("/d/session_id")
            .or_else(|| hello.pointer("/d/sessionId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        *session = Some(Session {
            url,
            id: id.clone(),
            sn: 0,
        });
        log::info!("网关新会话 {}", short(&id));
        let kind = if *had_session {
            ReadyKind::Rebuilt
        } else {
            ReadyKind::New
        };
        *had_session = true;
        emit(GatewayEvent::Ready {
            session_id: id,
            kind,
        });
    }

    pump((tx, rx), cfg, session, resume_deadline, out, emit).await
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Parts = (
    futures_util::stream::SplitSink<Ws, Message>,
    futures_util::stream::SplitStream<Ws>,
);

async fn pump<F: FnMut(GatewayEvent)>(
    (mut tx, mut rx): Parts,
    cfg: &Config,
    session: &mut Option<Session>,
    mut resume_deadline: Option<Instant>,
    out: &mut Outcome,
    emit: &mut F,
) -> AppResult<()> {
    let mut ticker = tokio::time::interval(cfg.heartbeat);
    ticker.tick().await;
    let mut pong_deadline: Option<Instant> = None;
    let mut ping_at: Option<Instant> = None;
    let mut last_tick = std::time::Instant::now();
    let mut order = Reorder::default();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let gap = last_tick.elapsed();
                last_tick = std::time::Instant::now();
                if gap > cfg.heartbeat + cfg.heartbeat / 2 {
                    log::warn!("网关心跳计时延迟：间隔 {}ms，计划 {}ms", gap.as_millis(), cfg.heartbeat.as_millis());
                }
                if let Some(s) = session.as_mut() {
                    order.flush_stale(&mut s.sn, emit);
                }
                let sn = session.as_ref().map_or(0, |s| s.sn);
                let ping = Message::text(json!({ "s": 2, "sn": sn }).to_string());
                match timeout(cfg.pong_timeout, tx.send(ping)).await {
                    Err(_) => return Err(AppError::Other("心跳发送超时".into())),
                    Ok(r) => r?,
                }
                log::trace!("网关心跳 sn={sn}");
                ping_at.get_or_insert_with(Instant::now);
                pong_deadline.get_or_insert(Instant::now() + cfg.pong_timeout);
            }
            () = until(pong_deadline) => {
                let since = last_tick.elapsed();
                return Err(AppError::Other(format!("心跳超时，距上次心跳 {}ms", since.as_millis())));
            }
            () = until(resume_deadline) => {
                resume_deadline = None;
                log::warn!("续传 {}ms 内未收到 s:6，按重建补偿", cfg.resume_timeout.as_millis());
                let id = session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
                emit(GatewayEvent::Ready { session_id: id, kind: ReadyKind::Rebuilt });
            }
            frame = rx.next() => {
                let Some(frame) = frame else { return Err(AppError::Other("连接关闭".into())) };
                let frame = frame?;
                out.last_frame = Some(Instant::now());
                if let Message::Close(c) = &frame {
                    return Err(AppError::Other(match c {
                        Some(c) => format!("服务端关闭连接 {} {}", u16::from(c.code), c.reason),
                        None => "服务端关闭连接".into(),
                    }));
                }
                let binary = matches!(frame, Message::Binary(_) | Message::Text(_));
                let Some(v) = decode(frame) else {
                    if binary {
                        log::warn!("网关帧无法解析");
                    }
                    continue;
                };
                log::trace!("网关帧 s={:?} sn={:?}", v.get("s"), v.get("sn"));
                match v.get("s").and_then(Value::as_i64) {
                    Some(0) => {
                        let d = v.get("d").cloned().unwrap_or(Value::Null);
                        match (v.get("sn").and_then(Value::as_u64), session.as_mut()) {
                            (Some(n), Some(s)) => order.push(n, d, &mut s.sn, emit),
                            _ => emit(GatewayEvent::Event(d)),
                        }
                    }
                    Some(3) => {
                        if let Some(t) = ping_at.take() {
                            let rtt = t.elapsed();
                            if rtt > Duration::from_secs(2) {
                                log::warn!("网关心跳回应缓慢 {}ms", rtt.as_millis());
                            } else {
                                log::trace!("网关心跳回应 {}ms", rtt.as_millis());
                            }
                        }
                        pong_deadline = None;
                        out.stable = true;
                    }
                    Some(5) => {
                        log::info!("网关要求重连：{}", v.get("d").unwrap_or(&Value::Null));
                        *session = None;
                        out.immediate = true;
                        return Err(AppError::Other("服务端要求重连".into()));
                    }
                    Some(6) if resume_deadline.take().is_some() => {
                        log::info!("网关续传完成");
                        out.stable = true;
                        let id = session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
                        emit(GatewayEvent::Ready { session_id: id, kind: ReadyKind::Resumed });
                    }
                    _ => {}
                }
            }
        }
    }
}
