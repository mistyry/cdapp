//! 一次推流会话：取凭据、进房采集、首帧后按源宽高比提交编码配置并发布，之后处理续期与尺寸变化。
//! 会话在后台任务中运行，结束原因经事件上报。令牌只在内存中传递，不写入日志。

use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::bridge::{self, BridgeError, Event, Live, LiveParams, SourceKind};
use super::fit;
use super::loopback::{Loopback, Scope};
use crate::error::{AppError, AppResult};
use crate::kook::api::Api;
use crate::kook::golive::Tier;

/// 进房与首帧的等待上限。
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// 检查语音会话的间隔。KOOK 语音断开后服务端约 3 至 6 秒结束分享，本地先停。
const VOICE_CHECK: Duration = Duration::from_secs(1);
/// 令牌续期兜底间隔。令牌 24 小时有效，SDK 在到期前提示。
const REFRESH_EVERY: Duration = Duration::from_secs(12 * 3600);
/// 本地视频流状态：采集失败。
const VIDEO_FAILED: i32 = 3;
/// 房间状态中可通过更换令牌恢复的错误：令牌无效、更新令牌时令牌无效。
const TOKEN_ERRORS: [i32; 2] = [-1000, -1010];

/// 房间状态码的处理方式。见 `bytertc_media_defines.h` 的 `ErrorCode`。
#[derive(Debug, PartialEq, Eq)]
enum RoomAction {
    /// 0 为已进房，正值为警告，都不处理。
    Ignore,
    /// 更换令牌后继续。
    Renew,
    /// 结束推流。
    End,
}

fn room_action(code: i32) -> RoomAction {
    if code >= 0 {
        RoomAction::Ignore
    } else if TOKEN_ERRORS.contains(&code) {
        RoomAction::Renew
    } else {
        RoomAction::End
    }
}

/// 推流中的控制命令。
enum Control {
    /// 改档，回传结果。
    Update(Tier, oneshot::Sender<AppResult<()>>),
    /// 立即续期令牌，用于测试。
    #[cfg(any(test, feature = "test-media"))]
    Renew(oneshot::Sender<AppResult<()>>),
}

/// 开播请求。
#[derive(Debug, Clone)]
pub struct Request {
    pub channel_id: String,
    pub user_id: String,
    pub source_kind: SourceKind,
    pub source_id: u64,
    /// 窗口所属进程，窗口共享带声音时只采该进程树。显示器共享不使用。
    pub source_pid: u32,
    pub tier: Tier,
    pub audio: bool,
}

impl Request {
    /// 声音采集范围：显示器排除本进程树，窗口只采目标进程树。
    fn audio_scope(&self) -> Scope {
        match self.source_kind {
            SourceKind::Screen => Scope::Exclude(std::process::id()),
            SourceKind::Window => Scope::Include(self.source_pid),
        }
    }
}

/// 上报给前端的事件。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ShareEvent {
    /// 已发布，附实际编码尺寸。
    Live { width: u32, height: u32 },
    /// 改档生效，附新的档位与编码尺寸。
    Tier {
        resolution: u32,
        frame_rate: u32,
        width: u32,
        height: u32,
    },
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

/// 语音会话是否仍在该频道。分享依附语音，返回 false 时结束推流。
pub type VoiceAlive = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync,
>;

/// 会话句柄。
pub struct Session {
    stop: Option<oneshot::Sender<()>>,
    control: mpsc::UnboundedSender<Control>,
    done: tauri::async_runtime::JoinHandle<()>,
    pub channel_id: String,
    pub generation: u64,
}

impl Session {
    #[must_use]
    pub fn start(api: Api, req: Request, generation: u64, sink: Sink, voice: VoiceAlive) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let (control, control_rx) = mpsc::unbounded_channel();
        let channel_id = req.channel_id.clone();
        let done = tauri::async_runtime::spawn(async move {
            let mut push = Push::default();
            let r = run(&api, req, stop_rx, control_rx, &sink, &voice, &mut push).await;
            // 无论从哪条路径结束，先停声音推送，再在阻塞线程中销毁引擎。
            let (sound, live) = (push.sound.take(), push.live.take());
            let _ = tauri::async_runtime::spawn_blocking(move || {
                drop(sound);
                drop(live);
            })
            .await;
            let message = r.err().map(|e| {
                log::warn!("屏幕分享结束：{e}");
                e.to_string()
            });
            sink(ShareEvent::Ended { message });
        });
        Self {
            stop: Some(stop),
            control,
            done,
            channel_id,
            generation,
        }
    }

    /// 改档。调用方只能传本人拥有的档位，服务端不校验。
    ///
    /// # Errors
    ///
    /// 推流已结束、接口失败或编码配置失败。
    pub async fn update(&self, tier: Tier) -> AppResult<()> {
        let (tx, rx) = oneshot::channel();
        self.control
            .send(Control::Update(tier, tx))
            .map_err(|_| AppError::Other("推流已结束".into()))?;
        rx.await.map_err(|_| AppError::Other("推流已结束".into()))?
    }

    /// 立即续期令牌。
    ///
    /// # Errors
    ///
    /// 推流已结束或续期失败。
    #[cfg(any(test, feature = "test-media"))]
    pub async fn renew(&self) -> AppResult<()> {
        let (tx, rx) = oneshot::channel();
        self.control
            .send(Control::Renew(tx))
            .map_err(|_| AppError::Other("推流已结束".into()))?;
        rx.await.map_err(|_| AppError::Other("推流已结束".into()))?
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
    /// 屏幕声音采集与推送，须先于 `live` 释放。
    sound: Option<Loopback>,
    /// 当前编码尺寸。
    size: (u32, u32),
    /// 最近读到的源实际尺寸。源已关闭、读不到时沿用。
    source: (u32, u32),
    kbps: u32,
}

impl Push {
    /// 向系统读取源的实际尺寸。
    fn read_source(&mut self, req: &Request) -> (u32, u32) {
        if let Some(s) = super::source::size(req.source_kind, req.source_id) {
            self.source = s;
        }
        self.source
    }

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
    mut req: Request,
    mut stop: oneshot::Receiver<()>,
    mut control: mpsc::UnboundedReceiver<Control>,
    sink: &Sink,
    voice: &VoiceAlive,
    push: &mut Push,
) -> AppResult<()> {
    let Some(mut events) = begin(api, &req, &mut stop, push).await? else {
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
    let mut voice_check = tokio::time::interval(VOICE_CHECK);
    loop {
        let renew = tokio::select! {
            _ = &mut stop => return Ok(()),
            _ = voice_check.tick() => {
                if !voice().await {
                    return Err(AppError::Other("已退出语音频道".into()));
                }
                false
            }
            _ = refresh.tick() => true,
            c = control.recv() => match c {
                Some(Control::Update(tier, reply)) => {
                    let r = update(api, &mut req, tier, push).await;
                    if r.is_ok() {
                        let (width, height) = push.size;
                        sink(ShareEvent::Tier {
                            resolution: tier.resolution,
                            frame_rate: tier.frame_rate,
                            width,
                            height,
                        });
                    }
                    let _ = reply.send(r);
                    false
                }
                #[cfg(any(test, feature = "test-media"))]
                Some(Control::Renew(reply)) => {
                    let _ = reply.send(renew(api, &req, push).await);
                    false
                }
                // 句柄已释放，等待停止信号。
                None => std::future::pending().await,
            },
            e = events.recv() => on_event(e, &req, sink, push).await?,
        };
        if renew {
            self::renew(api, &req, push).await?;
        }
    }
}

/// 取新令牌并交给 SDK。
async fn renew(api: &Api, req: &Request, push: &mut Push) -> AppResult<()> {
    let token = api.go_live_refresh(&req.channel_id).await?.token;
    push.call(move |l| l.token(&token)).await?;
    log::info!("屏幕分享令牌已续期");
    Ok(())
}

/// 改档：go-live-update 取新码率，按新档位与源的实际尺寸重新提交编码配置。
async fn update(api: &Api, req: &mut Request, tier: Tier, push: &mut Push) -> AppResult<()> {
    let bitrate = api
        .go_live_update(&req.channel_id, &req.user_id, tier)
        .await?;
    push.kbps = fit::kbps(bitrate);
    let source = push.read_source(req);
    push.encode(fit::fit(tier.resolution, source), tier.frame_rate)
        .await?;
    req.tier = tier;
    log::info!(
        "屏幕分享改档：{}p {} 帧，源 {}x{}，编码 {}x{} {} kbps",
        tier.resolution,
        tier.frame_rate,
        source.0,
        source.1,
        push.size.0,
        push.size.1,
        push.kbps
    );
    Ok(())
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
        // 回调中的尺寸是编码后的，不能作为源尺寸。窗口大小变化也会触发，此时重读源的实际尺寸。
        Some(Event::SizeChanged { .. }) => {
            let next = fit::fit(req.tier.resolution, push.read_source(req));
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
        Some(Event::RoomState(code)) => match room_action(code) {
            RoomAction::Ignore => {}
            RoomAction::Renew => {
                log::warn!("分享房间报告令牌无效：{code}");
                return Ok(true);
            }
            RoomAction::End => return Err(AppError::Other(format!("分享连接已断开：{code}"))),
        },
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
                Some(Event::RoomState(code)) if room_action(code) != RoomAction::Ignore => {
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
    // 首帧尺寸在设置编码前等于源尺寸，只在系统读不到时作为初值。
    push.source = frame.unwrap_or_default();
    let source = push.read_source(req);
    push.encode(fit::fit(req.tier.resolution, source), req.tier.frame_rate)
        .await?;
    push.call(Live::publish).await?;
    if req.audio {
        push.sound = Some(Loopback::start(req.audio_scope(), |pcm| {
            // 推流停止后桥接返回状态不符，忽略。
            if let Err(e) = bridge::push_audio(pcm)
                && e != BridgeError::State
            {
                log::debug!("屏幕声音推送失败：{e}");
            }
        }));
    }
    Ok(Some(events))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_room_codes() {
        assert_eq!(room_action(0), RoomAction::Ignore);
        assert_eq!(room_action(1), RoomAction::Ignore);
        assert_eq!(room_action(-1000), RoomAction::Renew);
        assert_eq!(room_action(-1010), RoomAction::Renew);
        for code in [-1004, -1006, -1009, -1011, -1084] {
            assert_eq!(room_action(code), RoomAction::End, "{code}");
        }
    }
}
