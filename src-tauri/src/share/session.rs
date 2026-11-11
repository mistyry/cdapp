//! 一次推流会话：取凭据、进房采集、首帧后按源宽高比提交编码配置并发布，之后处理续期与尺寸变化。
//! 会话在后台任务中运行，结束原因经事件上报。令牌只在内存中传递，不写入日志。

use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::bridge::{BridgeError, Event, Live, LiveParams, SourceKind};
use super::fit;
use crate::error::{AppError, AppResult};
use crate::kook::api::Api;
use crate::kook::golive::Tier;

/// 进房与首帧的等待上限。
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// 令牌续期兜底间隔。令牌 24 小时有效，SDK 在到期前提示。
const REFRESH_EVERY: Duration = Duration::from_secs(12 * 3600);
/// 本地视频流状态：采集失败。
const VIDEO_FAILED: i32 = 3;

/// 开播请求。
#[derive(Debug, Clone)]
pub struct Request {
    pub channel_id: String,
    pub user_id: String,
    pub source_kind: SourceKind,
    pub source_id: u64,
    pub tier: Tier,
    pub audio: bool,
}

/// 上报给前端的事件。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ShareEvent {
    /// 已发布，附实际编码尺寸。
    Live { width: u32, height: u32 },
    /// 发送统计。
    Stats {
        kbps: i32,
        fps: i32,
        width: i32,
        height: i32,
    },
    /// 已结束。`message` 为空表示主动停止。
    Ended { message: Option<String> },
}

pub type Sink = std::sync::Arc<dyn Fn(ShareEvent) + Send + Sync>;

/// 会话句柄。
pub struct Session {
    stop: Option<oneshot::Sender<()>>,
    done: tauri::async_runtime::JoinHandle<()>,
    pub channel_id: String,
    pub generation: u64,
}

impl Session {
    #[must_use]
    pub fn start(api: Api, req: Request, generation: u64, sink: Sink) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let channel_id = req.channel_id.clone();
        let done = tauri::async_runtime::spawn(async move {
            let mut push = Push::default();
            let r = run(&api, &req, stop_rx, &sink, &mut push).await;
            // 无论从哪条路径结束，都在阻塞线程中销毁引擎。
            if let Some(l) = push.live.take() {
                let _ = tauri::async_runtime::spawn_blocking(move || drop(l)).await;
            }
            let message = r.err().map(|e| {
                log::warn!("屏幕分享结束：{e}");
                e.to_string()
            });
            sink(ShareEvent::Ended { message });
        });
        Self {
            stop: Some(stop),
            done,
            channel_id,
            generation,
        }
    }

    /// 停止并等待引擎销毁。
    pub async fn stop(mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        let _ = self.done.await;
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.done.inner().is_finished()
    }
}

/// 推流状态，由会话任务持有。
#[derive(Default)]
struct Push {
    live: Option<Live>,
    /// 当前编码尺寸。
    size: (u32, u32),
    kbps: u32,
}

impl Push {
    /// 在阻塞线程中调用推流句柄。
    async fn call<T: Send + 'static>(
        &mut self,
        f: impl FnOnce(&Live) -> Result<T, BridgeError> + Send + 'static,
    ) -> AppResult<T> {
        let live = self
            .live
            .take()
            .ok_or_else(|| AppError::Other("推流已结束".into()))?;
        let (result, live) = blocking(move || (f(&live), live)).await?;
        self.live = Some(live);
        result.map_err(|e| AppError::Other(e.to_string()))
    }

    async fn encode(&mut self, size: (u32, u32), fps: u32) -> AppResult<()> {
        let (width, height, kbps) = (size.0, size.1, self.kbps);
        self.call(move |l| l.encoder(width, height, fps, kbps))
            .await?;
        self.size = size;
        Ok(())
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Other(e.to_string()))
}

/// 主动停止返回 `Ok`，其余结束原因返回错误。
async fn run(
    api: &Api,
    req: &Request,
    mut stop: oneshot::Receiver<()>,
    sink: &Sink,
    push: &mut Push,
) -> AppResult<()> {
    let Some(mut events) = begin(api, req, &mut stop, push).await? else {
        return Ok(());
    };
    let (width, height) = push.size;
    log::info!(
        "屏幕分享已发布：{width}x{height} {} 帧 {} kbps",
        req.tier.frame_rate,
        push.kbps
    );
    sink(ShareEvent::Live { width, height });

    let mut refresh = tokio::time::interval(REFRESH_EVERY);
    refresh.tick().await;
    loop {
        let renew = tokio::select! {
            _ = &mut stop => return Ok(()),
            _ = refresh.tick() => true,
            e = events.recv() => on_event(e, req, sink, push).await?,
        };
        if renew {
            let token = api.go_live_refresh(&req.channel_id).await?.token;
            push.call(move |l| l.token(&token)).await?;
            log::info!("屏幕分享令牌已续期");
        }
    }
}

/// 处理发布后的事件，返回是否需要续期令牌。
async fn on_event(
    e: Option<Event>,
    req: &Request,
    sink: &Sink,
    push: &mut Push,
) -> AppResult<bool> {
    match e {
        Some(Event::TokenWillExpire) => return Ok(true),
        Some(Event::SizeChanged { width, height }) => {
            let next = fit::fit(req.tier.resolution, (width, height));
            if next != push.size {
                push.encode(next, req.tier.frame_rate).await?;
                log::info!("屏幕分享编码尺寸改为 {}x{}", next.0, next.1);
            }
        }
        Some(Event::Stats {
            kbps,
            fps,
            width,
            height,
        }) => sink(ShareEvent::Stats {
            kbps,
            fps,
            width,
            height,
        }),
        Some(Event::VideoState {
            state: VIDEO_FAILED,
            error,
        }) => return Err(AppError::Other(format!("屏幕采集已停止：{error}"))),
        Some(Event::RoomState(code)) if code < 0 => {
            return Err(AppError::Other(format!("分享连接已断开：{code}")));
        }
        Some(_) => {}
        None => return Err(AppError::Other("推流已结束".into())),
    }
    Ok(false)
}

/// 取凭据、进房采集，等进房与首帧后按源宽高比提交编码配置并发布。主动停止时返回 `None`。
async fn begin(
    api: &Api,
    req: &Request,
    stop: &mut oneshot::Receiver<()>,
    push: &mut Push,
) -> AppResult<Option<mpsc::UnboundedReceiver<Event>>> {
    let got = api.go_live(&req.channel_id, &req.user_id, req.tier).await?;
    let volc = got
        .volc
        .ok_or_else(|| AppError::Other("当前账号分配的分享服务暂不支持".into()))?;
    push.kbps = fit::kbps(got.max_bitrate);
    push.size = fit::tier_box(req.tier.resolution);
    let (tx, mut events) = mpsc::unbounded_channel();
    let (r, kbps, (width, height)) = (req.clone(), push.kbps, push.size);
    let live = blocking(move || {
        Live::start(
            &LiveParams {
                app_id: &volc.app_id,
                room: &volc.channel_name,
                uid: &r.user_id,
                token: &volc.token,
                source_kind: r.source_kind,
                source_id: r.source_id,
                width,
                height,
                frame_rate: r.tier.frame_rate,
                max_kbps: kbps,
                audio: r.audio,
            },
            tx,
        )
    })
    .await?
    .map_err(|e| AppError::Other(e.to_string()))?;
    push.live = Some(live);

    let mut joined = false;
    let mut frame = None;
    let deadline = tokio::time::sleep(READY_TIMEOUT);
    tokio::pin!(deadline);
    while !joined || frame.is_none() {
        tokio::select! {
            _ = &mut *stop => return Ok(None),
            () = &mut deadline => return Err(AppError::Other("屏幕分享启动超时".into())),
            e = events.recv() => match e {
                Some(Event::RoomState(0)) => joined = true,
                Some(Event::RoomState(code)) if code < 0 => {
                    return Err(AppError::Other(format!("进入分享房间失败：{code}")));
                }
                Some(Event::FirstFrame { width, height }) => frame = Some((width, height)),
                Some(Event::VideoState { state: VIDEO_FAILED, error }) => {
                    return Err(AppError::Other(format!("屏幕采集失败：{error}")));
                }
                Some(_) => {}
                None => return Err(AppError::Other("推流已结束".into())),
            }
        }
    }
    push.encode(
        fit::fit(req.tier.resolution, frame.unwrap_or_default()),
        req.tier.frame_rate,
    )
    .await?;
    push.call(Live::publish).await?;
    Ok(Some(events))
}
