//! 主网关生命周期回归：本地模拟网关，不需要账号。
#![allow(clippy::result_large_err)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::error::AppError;
use cdapp_lib::kook::gateway::{self, Config, GatewayEvent, ReadyKind};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

/// 模拟网关对单个连接的行为。收到连接 URI，返回脚本。
#[derive(Clone)]
enum Step {
    Send(Value),
    /// 等到客户端发来心跳后回复 s:3。
    Pong,
    Close,
    /// 保持连接直到客户端断开。
    Hold,
}

type Script = Arc<dyn Fn(usize, &str) -> Vec<Step> + Send + Sync>;

async fn serve(script: Script) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let uris = Arc::new(Mutex::new(Vec::new()));
    let seen = uris.clone();
    tokio::spawn(async move {
        let mut n = 0;
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let i = n;
            n += 1;
            let script = script.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut uri = String::new();
                let cb = |req: &Request, resp: Response| {
                    uri = req.uri().to_string();
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else {
                    return;
                };
                seen.lock().unwrap().push(uri.clone());
                for step in script(i, &uri) {
                    match step {
                        Step::Send(v) => {
                            if ws.send(Message::text(v.to_string())).await.is_err() {
                                return;
                            }
                        }
                        Step::Pong => loop {
                            match ws.next().await {
                                Some(Ok(Message::Text(t))) if t.contains("\"s\":2") => {
                                    let _ =
                                        ws.send(Message::text(json!({"s":3}).to_string())).await;
                                    break;
                                }
                                Some(Ok(_)) => {}
                                _ => return,
                            }
                        },
                        Step::Close => {
                            let _ = ws.close(None).await;
                            return;
                        }
                        Step::Hold => while let Some(Ok(_)) = ws.next().await {},
                    }
                }
            });
        }
    });
    (format!("ws://{addr}/gateway?compress=1&token=t"), uris)
}

fn cfg() -> Config {
    Config {
        connect_timeout: Duration::from_millis(500),
        hello_timeout: Duration::from_millis(300),
        heartbeat: Duration::from_millis(50),
        pong_timeout: Duration::from_millis(300),
        resume_timeout: Duration::from_millis(300),
        min_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(200),
    }
}

fn hello(id: &str) -> Step {
    Step::Send(json!({"s":1,"d":{"code":0,"session_id":id}}))
}
fn event(sn: u64) -> Step {
    Step::Send(json!({"s":0,"sn":sn,"d":{"n":sn}}))
}

struct Run {
    rx: mpsc::UnboundedReceiver<GatewayEvent>,
    stop: watch::Sender<bool>,
    wake: Arc<Notify>,
    fetches: Arc<Mutex<u32>>,
    done: tokio::task::JoinHandle<()>,
}

fn start(url: String, unauthorized_from: Option<u32>) -> Run {
    let (tx, rx) = mpsc::unbounded_channel();
    let (stop, stop_rx) = watch::channel(false);
    let wake = Arc::new(Notify::new());
    let fetches = Arc::new(Mutex::new(0u32));
    let f = fetches.clone();
    let fetch = move || {
        let url = url.clone();
        let f = f.clone();
        async move {
            let n = {
                let mut g = f.lock().unwrap();
                *g += 1;
                *g
            };
            if unauthorized_from.is_some_and(|k| n >= k) {
                return Err(AppError::Unauthorized("401".into()));
            }
            Ok(url)
        }
    };
    let done = tokio::spawn(gateway::run(
        fetch,
        cfg(),
        stop_rx,
        wake.clone(),
        move |e| {
            let _ = tx.send(e);
        },
    ));
    Run {
        rx,
        stop,
        wake,
        fetches,
        done,
    }
}

async fn next(r: &mut Run) -> GatewayEvent {
    tokio::time::timeout(Duration::from_secs(3), r.rx.recv())
        .await
        .expect("等待网关事件超时")
        .expect("网关已结束")
}

/// 跳过断开通知，取下一个非断开事件。
async fn next_live(r: &mut Run) -> GatewayEvent {
    loop {
        match next(r).await {
            GatewayEvent::Disconnected { .. } => {}
            e => return e,
        }
    }
}

fn ready(kind: ReadyKind) -> GatewayEvent {
    GatewayEvent::Ready {
        session_id: "S".into(),
        kind,
    }
}

#[tokio::test]
async fn resumes_after_drop_without_duplicates() {
    let (url, uris) = serve(Arc::new(|i, uri: &str| match i {
        0 => vec![hello("S"), event(1), event(2), event(2), Step::Close],
        _ if uri.contains("resume=1") => vec![
            hello("S"),
            event(2),
            event(3),
            Step::Send(json!({"s":6})),
            Step::Hold,
        ],
        _ => vec![Step::Close],
    }))
    .await;
    let mut r = start(url, None);
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    assert_eq!(next(&mut r).await, GatewayEvent::Event(json!({"n":1})));
    assert_eq!(next(&mut r).await, GatewayEvent::Event(json!({"n":2})));
    assert!(matches!(
        next(&mut r).await,
        GatewayEvent::Disconnected { .. }
    ));
    // 续传期间重发的 sn=2 被丢弃，补发的 sn=3 只出现一次，最后报告续传成功。
    assert_eq!(next(&mut r).await, GatewayEvent::Event(json!({"n":3})));
    assert_eq!(next(&mut r).await, ready(ReadyKind::Resumed));
    let resume = uris.lock().unwrap()[1].clone();
    assert!(resume.contains("resume=1&sn=2&session_id=S"), "{resume}");
    assert_eq!(*r.fetches.lock().unwrap(), 1, "续传不重新取地址");
    r.stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), r.done)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn rejected_resume_rebuilds_immediately() {
    let (url, _) = serve(Arc::new(|i, uri: &str| match i {
        0 => vec![hello("S"), event(5), Step::Close],
        _ if uri.contains("resume=1") => {
            vec![Step::Send(json!({"s":1,"d":{"code":40106}})), Step::Close]
        }
        _ => vec![hello("S"), Step::Hold],
    }))
    .await;
    let mut r = start(url, None);
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    next(&mut r).await;
    assert_eq!(next_live(&mut r).await, ready(ReadyKind::Rebuilt));
    assert_eq!(*r.fetches.lock().unwrap(), 2, "续传失败后重新取地址");
    r.stop.send(true).unwrap();
}

#[tokio::test]
async fn server_reset_rebuilds() {
    // s:5 要求重连：丢弃会话，立即开新会话。
    let (url, uris) = serve(Arc::new(|i, _: &str| match i {
        0 => vec![hello("S"), event(1), Step::Send(json!({"s":5})), Step::Hold],
        _ => vec![hello("S"), Step::Hold],
    }))
    .await;
    let mut r = start(url, None);
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    next(&mut r).await;
    assert_eq!(next_live(&mut r).await, ready(ReadyKind::Rebuilt));
    assert!(!uris.lock().unwrap()[1].contains("resume=1"));
    r.stop.send(true).unwrap();
}

#[tokio::test]
async fn resume_without_completion_is_treated_as_rebuilt() {
    let (url, _) = serve(Arc::new(|i, _: &str| match i {
        0 => vec![hello("S"), event(1), Step::Close],
        _ => vec![hello("S"), Step::Hold], // 续传连接接受但不发 s:6
    }))
    .await;
    let mut r = start(url, None);
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    next(&mut r).await;
    assert_eq!(next_live(&mut r).await, ready(ReadyKind::Rebuilt));
    r.stop.send(true).unwrap();
}

#[tokio::test]
async fn expired_credentials_stop_without_retry() {
    // 续传被拒后重新取地址时返回 401：上报失效并停止，不再重试。
    let (url, _) = serve(Arc::new(|i, _: &str| match i {
        0 => vec![hello("S"), Step::Close],
        _ => vec![Step::Send(json!({"s":1,"d":{"code":40106}})), Step::Close],
    }))
    .await;
    let mut r = start(url, Some(2));
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    assert_eq!(next_live(&mut r).await, GatewayEvent::Expired("401".into()));
    assert_eq!(*r.fetches.lock().unwrap(), 2);
    tokio::time::timeout(Duration::from_secs(1), r.done)
        .await
        .expect("凭据失效后网关应停止")
        .unwrap();
}

#[tokio::test]
async fn hello_timeout_and_heartbeat_timeout_disconnect() {
    let (url, _) = serve(Arc::new(|i, _: &str| match i {
        // 首个连接不发 hello。
        0 => vec![Step::Hold],
        // 第二个连接发 hello 后不回心跳。
        _ => vec![hello("S"), Step::Hold],
    }))
    .await;
    let mut r = start(url, None);
    match next(&mut r).await {
        GatewayEvent::Disconnected { reason, .. } => assert!(reason.contains("hello"), "{reason}"),
        e => panic!("{e:?}"),
    }
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    match next(&mut r).await {
        GatewayEvent::Disconnected { reason, .. } => assert!(reason.contains("心跳"), "{reason}"),
        e => panic!("{e:?}"),
    }
    r.stop.send(true).unwrap();
}

#[tokio::test]
async fn backoff_grows_and_resets_after_stable_connection() {
    let (url, _) = serve(Arc::new(|i, _: &str| match i {
        // 第 4 个连接心跳往返成功（稳定），其余握手后立即关闭（不稳定）。
        3 => vec![hello("S"), Step::Pong, Step::Close], // 心跳往返成功：稳定
        _ => vec![Step::Close],
    }))
    .await;
    let mut r = start(url, None);
    let mut delays = Vec::new();
    while delays.len() < 5 {
        if let GatewayEvent::Disconnected { retry_in, .. } = next(&mut r).await {
            delays.push(retry_in.as_millis());
        }
    }
    assert_eq!(&delays[..3], &[20, 40, 80]);
    // 稳定连接断开后回到基线。
    assert_eq!(delays[3], 20, "{delays:?}");
    r.stop.send(true).unwrap();
}

#[tokio::test]
async fn wake_skips_backoff_and_stop_ends_run() {
    let (url, uris) = serve(Arc::new(|_, _: &str| vec![Step::Close])).await;
    let mut r = start(url, None);
    // 退避上限 200ms 下等到退避足够长，再唤醒立即重连。
    loop {
        if let GatewayEvent::Disconnected { retry_in, .. } = next(&mut r).await
            && retry_in >= Duration::from_millis(160)
        {
            break;
        }
    }
    let before = uris.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(5)).await;
    r.wake.notify_waiters();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(uris.lock().unwrap().len() > before, "唤醒后应立即重连");
    r.stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), r.done)
        .await
        .expect("停止后不再重连")
        .unwrap();
}

#[tokio::test]
async fn out_of_order_events_are_reordered_not_dropped() {
    let (url, _) = serve(Arc::new(|i, _: &str| match i {
        0 => vec![
            hello("S"),
            event(1),
            event(3),
            event(2),
            event(2),
            Step::Hold,
        ],
        _ => vec![Step::Hold],
    }))
    .await;
    let mut r = start(url, None);
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    for n in 1..=3 {
        assert_eq!(next(&mut r).await, GatewayEvent::Event(json!({ "n": n })));
    }
    r.stop.send(true).unwrap();
}

#[tokio::test]
async fn missing_sequence_is_skipped_after_a_heartbeat() {
    let (url, _) = serve(Arc::new(|i, _: &str| match i {
        0 => vec![hello("S"), event(1), event(3), Step::Hold],
        _ => vec![Step::Hold],
    }))
    .await;
    let mut r = start(url, None);
    assert_eq!(next(&mut r).await, ready(ReadyKind::New));
    assert_eq!(next(&mut r).await, GatewayEvent::Event(json!({ "n": 1 })));
    assert_eq!(next(&mut r).await, GatewayEvent::Event(json!({ "n": 3 })));
    r.stop.send(true).unwrap();
}

#[tokio::test]
async fn repeated_server_reset_falls_back_to_backoff() {
    let (url, _) = serve(Arc::new(|_, _: &str| {
        vec![hello("S"), Step::Send(json!({"s":5})), Step::Hold]
    }))
    .await;
    let mut r = start(url, None);
    let mut delays = Vec::new();
    while delays.len() < 4 {
        if let GatewayEvent::Disconnected { retry_in, .. } = next(&mut r).await {
            delays.push(retry_in.as_millis());
        }
    }
    assert_eq!(delays, vec![0, 20, 0, 40]);
    r.stop.send(true).unwrap();
}
