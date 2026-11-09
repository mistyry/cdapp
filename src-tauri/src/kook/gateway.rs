//! 主网关。下行帧为 zlib 压缩 JSON，信令与开放平台 Bot 网关一致。
//!
//! 生命周期：
//! - 新会话：请求 `gateway/index` 取一次性 URL，hello 成功后上报 [`ReadyKind::New`]（之前有过会话时为
//!   [`ReadyKind::Rebuilt`]），上层须重新激活服务器并补拉消息。
//! - 续传：网络类断开保留会话，用原 URL 追加 `resume=1&sn&session_id` 重连，收到 `s:6` 上报
//!   [`ReadyKind::Resumed`]；限定时间内未收到 `s:6` 按重建处理。
//! - 续传失败：服务端拒绝 hello 或 HTTP 握手被拒时丢弃会话，立即改开新会话。
//! - 凭据失效：取地址返回 401 时上报 [`GatewayEvent::Expired`] 并停止，不再重试。
//! - 主动停止：`stop` 置为 true 后立即退出，不再重连。

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

/// 网关超时与退避参数。
#[derive(Debug, Clone)]
pub struct Config {
    /// 建立 WebSocket（含 TLS 握手）的上限。
    pub connect_timeout: Duration,
    /// 连接后等待 hello 的上限。
    pub hello_timeout: Duration,
    pub heartbeat: Duration,
    /// 发出心跳后等待 `s:3` 的上限。
    pub pong_timeout: Duration,
    /// 续传连接后等待 `s:6` 的上限，超时按重建处理。
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
    /// 本次运行的首个会话。
    New,
    /// 续传成功，断线期间的事件已补发。
    Resumed,
    /// 续传失败或超时后的新会话，断线期间的事件可能丢失，上层须补偿。
    Rebuilt,
}

/// 网关向上层投递的内容。
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayEvent {
    Ready {
        session_id: String,
        kind: ReadyKind,
    },
    Event(Value),
    /// 连接断开，`retry_in` 后重连（可被唤醒提前）。
    Disconnected {
        reason: String,
        retry_in: Duration,
    },
    /// 凭据被服务端确认失效，网关已停止。
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

/// 错误及其来源链。reqwest 等错误的 Display 不含底层原因。
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

/// 按 sn 排序投递事件：重复或已处理的序号丢弃，跳号时先缓冲等待缺失的序号。
#[derive(Default)]
struct Reorder {
    buf: std::collections::BTreeMap<u64, Value>,
    /// 上次心跳时缓冲区是否已有积压。
    stale: bool,
}

/// 缓冲上限，超过时放弃等待缺口。
const REORDER_LIMIT: usize = 256;

impl Reorder {
    fn push<F: FnMut(GatewayEvent)>(&mut self, n: u64, d: Value, sn: &mut u64, emit: &mut F) {
        if n <= *sn || self.buf.contains_key(&n) {
            return;
        }
        // 新会话尚无序号时以首个事件为起点。
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

/// 单次连接的结束方式（除错误外的附加信息）。
#[derive(Default)]
struct Outcome {
    /// 握手完成时刻，用于记录连接持续时长。
    opened: Option<Instant>,
    /// 最后收到任意帧的时刻。
    last_frame: Option<Instant>,
    /// 收到过心跳回应或续传完成，连接确认可用，退避回到基线。
    stable: bool,
    /// 续传被拒或服务端要求重连，应立即开新会话而不等待退避。
    immediate: bool,
}

/// 运行网关直到 `stop` 置为 true 或凭据失效。`wake` 可在退避等待中提前重连。
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
    // 上次是否已立即重连。连续的立即重连改走退避，避免服务端反复要求重连时空转。
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
                // 实际等待远超计划，通常是系统睡眠或节能暂停了计时器。
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
            // 服务端以 HTTP 响应拒绝握手：URL 或会话已不可用，改开新会话。网络类错误保留会话以便续传。
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
        // 续传被拒（例如会话过期）时立即重新取地址开新会话；新会话被拒走正常退避。
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

/// 会话 ID 只记录前缀，便于对照又不完整暴露。
fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Parts = (
    futures_util::stream::SplitSink<Ws, Message>,
    futures_util::stream::SplitStream<Ws>,
);

/// 已连接后的心跳与事件循环，直到断开。
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
    // 使用墙钟：tokio 的 Instant 在系统睡眠期间可能不前进。
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
                // 缺失的序号一个心跳周期内仍未到达，放弃等待，按序投递已缓冲事件。
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
                // 续传迟迟未完成：保留连接，但按重建通知上层补偿可能缺失的事件。
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
