//! 观看他人的屏幕分享：取观众凭据、进房，分享者发布后订阅，把编码后的画面交给前端解码。
//! 画面不经 SDK 解码，前端用 `WebCodecs` 解码并绘制；屏幕声音由 SDK 在本进程播放。
//! 令牌只在内存中传递，不写入日志。

use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::bridge::{Frame, Vendor, Watch, WatchEvent, WatchParams, WatchSink};
use crate::error::{AppError, AppResult};
use crate::kook::api::Api;

/// 进房的等待上限。
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
/// 发往前端的帧队列长度，约 1 秒的 60 帧。前端跟不上时丢弃新帧，并在下一个关键帧前恢复。
const QUEUE: usize = 60;
/// 解码失败或丢帧后请求关键帧的最短间隔。
const KEY_FRAME_EVERY: Duration = Duration::from_secs(1);
/// Agora 没有请求关键帧的接口，以重新进房代替，画面中断约 4 秒，因此间隔更长。
const AGORA_REJOIN_EVERY: Duration = Duration::from_secs(10);
/// 令牌续期兜底间隔。令牌 24 小时有效，SDK 在到期前提示。
const REFRESH_EVERY: Duration = Duration::from_secs(12 * 3600);

/// 发往前端的一帧：小端头部 u8 类型（1 关键帧，0 非关键帧）、u8 编码、u16 宽、u16 高、i64 时间戳微秒，随后为
/// Annex B 格式的数据。头部 14 字节。
fn pack(f: &Frame<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + f.data.len());
    out.push(u8::from(f.key));
    out.push(u8::try_from(f.codec).unwrap_or(0));
    out.extend_from_slice(&u16::try_from(f.width).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&u16::try_from(f.height).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&f.ts_us.to_le_bytes());
    out.extend_from_slice(f.data);
    out
}

/// 上报给前端的观看事件。画面经单独的通道发送，不在此列。
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WatchState {
    /// 已进房，等待分享者的画面。
    Waiting,
    /// 已订阅，画面开始送达。
    Playing,
    /// 接收统计。
    Stats {
        kbps: i32,
        width: i32,
        height: i32,
        stalls: i32,
    },
    /// 已结束。`message` 为空表示主动停止。
    Ended { message: Option<String> },
}

pub type StateSink = std::sync::Arc<dyn Fn(WatchState) + Send + Sync>;
/// 画面出口。在转发线程调用，可阻塞。返回错误时观看结束，例如页面已关闭。
pub type FrameSink = Box<dyn Fn(Vec<u8>) -> Result<(), String> + Send>;

/// 可替换的画面出口。悬浮窗与主窗口各有自己的通道，切换承载窗口时替换出口，观看不中断。
#[derive(Clone)]
pub struct Frames(std::sync::Arc<std::sync::Mutex<FrameSink>>);

impl Frames {
    #[must_use]
    pub fn new(sink: FrameSink) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(sink)))
    }

    /// 替换出口，之后的帧发往新出口。
    pub fn replace(&self, sink: FrameSink) {
        if let Ok(mut s) = self.0.lock() {
            *s = sink;
        }
    }

    fn send(&self, msg: Vec<u8>) -> Result<(), String> {
        let s = self.0.lock().map_err(|e| e.to_string())?;
        s(msg)
    }
}

/// 观看请求。
#[derive(Debug, Clone)]
pub struct Request {
    pub channel_id: String,
    pub user_id: String,
    /// 分享者用户 ID。
    pub liver: String,
}

/// 观看中的控制命令。
enum Control {
    /// 请求关键帧，前端解码器重建后使用。
    KeyFrame,
    /// 画面改发到新的承载窗口，新解码器须从关键帧开始。用户操作触发，不限流。
    Redirected,
}

/// 观看句柄。
pub struct Session {
    stop: Option<oneshot::Sender<()>>,
    control: mpsc::UnboundedSender<Control>,
    done: tauri::async_runtime::JoinHandle<()>,
    frames: Frames,
    pub liver: String,
    pub generation: u64,
}

impl Session {
    /// 开始观看。`voice` 完成时结束，观看依附语音。
    #[must_use]
    pub fn start(
        api: Api,
        req: Request,
        generation: u64,
        state: StateSink,
        frames: FrameSink,
        voice: super::session::VoiceEnded,
    ) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let (control, control_rx) = mpsc::unbounded_channel();
        let liver = req.liver.clone();
        let frames = Frames::new(frames);
        let out = frames.clone();
        let done = tauri::async_runtime::spawn(async move {
            let mut held = None;
            let r = run(
                &api, &req, stop_rx, control_rx, &state, out, voice, &mut held,
            )
            .await;
            // 在阻塞线程中离房，离房后桥接不再交出画面，转发线程随之退出。
            let _ = tauri::async_runtime::spawn_blocking(move || drop(held)).await;
            let message = r.err().map(|e| {
                log::warn!("观看屏幕分享结束：{e}");
                e.to_string()
            });
            state(WatchState::Ended { message });
        });
        Self {
            stop: Some(stop),
            control,
            done,
            frames,
            liver,
            generation,
        }
    }

    /// 把画面改发到新的出口，例如悬浮窗，并让新出口的解码器从关键帧开始。
    pub fn redirect(&self, sink: FrameSink) {
        self.frames.replace(sink);
        let _ = self.control.send(Control::Redirected);
    }

    /// 请求关键帧。观看已结束时无操作。
    pub fn key_frame(&self) {
        let _ = self.control.send(Control::KeyFrame);
    }

    /// 停止并等待离房。
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

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Other(e.to_string()))
}

/// 在阻塞线程中调用观看句柄。
async fn call<T: Send + 'static>(
    held: &mut Option<Watch>,
    f: impl FnOnce(&Watch) -> Result<T, super::bridge::BridgeError> + Send + 'static,
) -> AppResult<T> {
    let w = held
        .take()
        .ok_or_else(|| AppError::Other("观看已结束".into()))?;
    let (r, w) = blocking(move || (f(&w), w)).await?;
    *held = Some(w);
    r.map_err(|e| AppError::Other(e.to_string()))
}

/// 画面转发：SDK 线程只做复制与入队，转发线程负责发给前端。队列满时丢弃，并记下需要关键帧。
/// 发送失败多为承载窗口刚关闭、出口尚未替换：丢弃该帧并要求关键帧，转发继续，替换出口后即恢复。
/// 观看何时结束由停止命令、语音结束与界面上的分享状态决定，不由发送失败决定。
fn forwarder(
    frames: Frames,
    need_key: mpsc::UnboundedSender<()>,
) -> (SyncSender<Vec<u8>>, std::thread::JoinHandle<()>) {
    let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE);
    let handle = std::thread::spawn(move || {
        let mut failing = false;
        for msg in rx {
            match frames.send(msg) {
                Ok(()) => failing = false,
                Err(e) => {
                    if !failing {
                        log::warn!("屏幕分享画面发送失败：{e}");
                        failing = true;
                    }
                    let _ = need_key.send(());
                }
            }
        }
    });
    (tx, handle)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run(
    api: &Api,
    req: &Request,
    mut stop: oneshot::Receiver<()>,
    mut control: mpsc::UnboundedReceiver<Control>,
    state: &StateSink,
    frames: Frames,
    mut voice: super::session::VoiceEnded,
    held: &mut Option<Watch>,
) -> AppResult<()> {
    let cred = api.go_live_watch(&req.channel_id, &req.liver).await?;
    let vendor = match cred.vendor {
        crate::kook::golive::Vendor::Volc => Vendor::Volc,
        crate::kook::golive::Vendor::Agora => {
            // Agora 运行文件在启动时与火山一同准备；未就绪时在此再试一次。
            super::agora_sdk::sync().await;
            if !super::agora_sdk::ready() {
                return Err(AppError::Other("Agora 观看组件不可用".into()));
            }
            Vendor::Agora
        }
    };
    log::info!("观看厂商：{vendor:?}");
    let volc = cred.options;
    // 丢帧后需要关键帧才能继续解码；转发线程失败也经此通知。
    let (need_key_tx, mut need_key) = mpsc::unbounded_channel::<()>();
    let (queue, forward) = forwarder(frames, need_key_tx.clone());
    // 等到关键帧前丢弃非关键帧，前端解码器只能从关键帧开始。
    let waiting_key = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let wk = waiting_key.clone();
    let on_frame = move |f: &Frame<'_>| {
        use std::sync::atomic::Ordering;
        if wk.load(Ordering::Relaxed) {
            if !f.key {
                return;
            }
            wk.store(false, Ordering::Relaxed);
        }
        // 转发线程已退出时不再入队，观看由 need_key 通知结束。
        if let Err(TrySendError::Full(_)) = queue.try_send(pack(f)) {
            wk.store(true, Ordering::Relaxed);
            let _ = need_key_tx.send(());
        }
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let (r, v) = (req.clone(), volc.clone());
    let watch = blocking(move || {
        Watch::start(
            vendor,
            &WatchParams {
                app_id: &v.app_id,
                room: &v.channel_name,
                uid: &r.user_id,
                token: &v.token,
                liver: &r.liver,
            },
            WatchSink {
                events: tx,
                frame: Box::new(on_frame),
            },
        )
    })
    .await?
    .map_err(|e| AppError::Other(e.to_string()))?;
    *held = Some(watch);

    let result = async {
        let deadline = tokio::time::sleep(JOIN_TIMEOUT);
        tokio::pin!(deadline);
        let mut joined = false;
        let mut subscribed = false;
        let mut refresh = tokio::time::interval(REFRESH_EVERY);
        refresh.tick().await;
        let mut last_key: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                _ = &mut stop => return Ok(()),
                () = &mut voice => return Err(AppError::Other("已退出语音频道".into())),
                () = &mut deadline, if !joined => return Err(AppError::Other("进入分享房间超时".into())),
                _ = refresh.tick() => renew(api, req, held).await?,
                c = control.recv() => match c {
                    Some(Control::KeyFrame) => {
                        waiting_key.store(true, std::sync::atomic::Ordering::Relaxed);
                        request_key(held, &mut last_key, Why::Decoder).await;
                    }
                    Some(Control::Redirected) => {
                        waiting_key.store(true, std::sync::atomic::Ordering::Relaxed);
                        request_key(held, &mut last_key, Why::Redirected).await;
                    }
                    None => std::future::pending::<()>().await,
                },
                Some(()) = need_key.recv() => {
                    if forward.is_finished() {
                        return Err(AppError::Other("画面已无法送达界面".into()));
                    }
                    request_key(held, &mut last_key, Why::Dropped).await;
                }
                e = events.recv() => match e {
                    Some(WatchEvent::RoomState(0)) => {
                        if !joined {
                            joined = true;
                            log::info!("已进入分享房间，等待画面");
                            state(WatchState::Waiting);
                        }
                    }
                    Some(WatchEvent::RoomState(code)) if code < 0 => {
                        if code == -1000 || code == -1010 {
                            log::warn!("分享房间报告令牌无效：{code}");
                            renew(api, req, held).await?;
                        } else {
                            return Err(AppError::Other(format!("分享连接已断开：{code}")));
                        }
                    }
                    Some(WatchEvent::RoomState(_)) => {}
                    Some(WatchEvent::Published) => {
                        waiting_key.store(true, std::sync::atomic::Ordering::Relaxed);
                        call(held, super::bridge::Watch::subscribe).await?;
                        if !subscribed {
                            subscribed = true;
                            log::info!("已订阅屏幕分享");
                        }
                        state(WatchState::Playing);
                        request_key(held, &mut last_key, Why::Subscribed).await;
                    }
                    // 进房时 SDK 会先报一次取消发布再报发布，实测确认。取消发布只回到等待，
                    // 分享真正结束由界面按网关 live_status_changed 停止观看。
                    Some(WatchEvent::Unpublished(reason)) => {
                        log::info!("分享者取消发布屏幕流：{reason}");
                        waiting_key.store(true, std::sync::atomic::Ordering::Relaxed);
                        if joined {
                            state(WatchState::Waiting);
                        }
                    }
                    Some(WatchEvent::TokenWillExpire) => renew(api, req, held).await?,
                    Some(WatchEvent::Stats { kbps, width, height, stalls }) => {
                        state(WatchState::Stats { kbps, width, height, stalls });
                    }
                    None => return Err(AppError::Other("观看已结束".into())),
                },
            }
        }
    }
    .await;
    // 转发线程在桥接回调释放后自行退出，见 Session::start。
    drop(forward);
    result
}

/// 需要关键帧的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// 刚订阅或分享者重新发布。
    Subscribed,
    /// 前端解码器出错后重建。
    Decoder,
    /// 发往前端的队列满，丢了帧。
    Dropped,
    /// 画面改发到新的承载窗口。
    Redirected,
}

/// 请求关键帧，按厂商的最短间隔限流。失败只记日志。
/// 火山刚订阅时立即请求。Agora 以重新进房代替，新进房时服务端已下发关键帧，刚订阅时不必再请求；
/// 换承载窗口由用户触发，Agora 也立即重新进房，否则新窗口要等很久才有关键帧。
async fn request_key(held: &mut Option<Watch>, last: &mut Option<tokio::time::Instant>, why: Why) {
    let vendor = held.as_ref().map(Watch::vendor);
    let now = tokio::time::Instant::now();
    let every = match (vendor, why) {
        (Some(Vendor::Agora), Why::Subscribed) => return,
        (_, Why::Subscribed | Why::Redirected) => Duration::ZERO,
        (Some(Vendor::Agora), _) => AGORA_REJOIN_EVERY,
        _ => KEY_FRAME_EVERY,
    };
    if last.is_some_and(|t| now - t < every) {
        return;
    }
    *last = Some(now);
    if vendor == Some(Vendor::Agora) {
        log::info!("Agora 画面需要关键帧（{why:?}），重新进入分享房间");
    }
    if let Err(e) = call(held, super::bridge::Watch::key_frame).await {
        log::debug!("请求关键帧失败：{e}");
    }
}

/// 取新的观众令牌并交给 SDK。
async fn renew(api: &Api, req: &Request, held: &mut Option<Watch>) -> AppResult<()> {
    let token = api
        .go_live_watch(&req.channel_id, &req.liver)
        .await?
        .options
        .token;
    call(held, move |w| w.token(&token)).await?;
    log::info!("观看令牌已续期");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_frame_header() {
        let data = [0u8, 0, 0, 1, 0x67];
        let out = pack(&Frame {
            data: &data,
            codec: 1,
            key: true,
            width: 1920,
            height: 1080,
            ts_us: 0x0102_0304_0506_0708,
        });
        assert_eq!(out.len(), 14 + data.len());
        assert_eq!(&out[..2], &[1, 1]);
        assert_eq!(u16::from_le_bytes([out[2], out[3]]), 1920);
        assert_eq!(u16::from_le_bytes([out[4], out[5]]), 1080);
        assert_eq!(
            i64::from_le_bytes(out[6..14].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
        assert_eq!(&out[14..], &data);
    }

    #[test]
    fn states_use_camel_case() {
        let v = serde_json::to_value(WatchState::Ended { message: None }).unwrap();
        assert_eq!(v["kind"], "ended");
        let v = serde_json::to_value(WatchState::Stats {
            kbps: 1,
            width: 2,
            height: 3,
            stalls: 0,
        })
        .unwrap();
        assert_eq!(v["kind"], "stats");
        assert_eq!(v["kbps"], 1);
    }
}
