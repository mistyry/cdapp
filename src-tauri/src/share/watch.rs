use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::bridge::{Frame, Vendor, Watch, WatchEvent, WatchParams, WatchSink};
use crate::error::{AppError, AppResult};
use crate::kook::api::Api;

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
const QUEUE: usize = 60;
const KEY_FRAME_EVERY: Duration = Duration::from_secs(1);
const AGORA_REJOIN_EVERY: Duration = Duration::from_secs(10);
const REFRESH_EVERY: Duration = Duration::from_secs(12 * 3600);
const MAX_RESUMES: u32 = 3;

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

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WatchState {
    Waiting,
    Playing,
    Stats {
        kbps: i32,
        width: i32,
        height: i32,
        stalls: i32,
    },
    Ended {
        message: Option<String>,
    },
}

pub type StateSink = std::sync::Arc<dyn Fn(WatchState) + Send + Sync>;
pub type FrameSink = Box<dyn Fn(Vec<u8>) -> Result<(), String> + Send>;

#[derive(Clone)]
pub struct Frames(std::sync::Arc<std::sync::Mutex<FrameSink>>);

impl Frames {
    #[must_use]
    pub fn new(sink: FrameSink) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(sink)))
    }

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

#[derive(Debug, Clone)]
pub struct Request {
    pub channel_id: String,
    pub user_id: String,
    pub liver: String,
}

enum Control {
    KeyFrame,
    Redirected,
}

pub struct Session {
    stop: Option<oneshot::Sender<()>>,
    control: mpsc::UnboundedSender<Control>,
    done: tauri::async_runtime::JoinHandle<()>,
    frames: Frames,
    pub liver: String,
    pub generation: u64,
}

impl Session {
    #[must_use]
    pub fn start(
        api: Api,
        req: Request,
        generation: u64,
        state: StateSink,
        frames: FrameSink,
        voice: super::session::VoiceEnded,
        rebuilt: super::session::Rebuilt,
    ) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let (control, control_rx) = mpsc::unbounded_channel();
        let liver = req.liver.clone();
        let frames = Frames::new(frames);
        let out = frames.clone();
        let done = tauri::async_runtime::spawn(async move {
            let r = run(&api, &req, stop_rx, control_rx, &state, out, voice, rebuilt).await;
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

    pub fn redirect(&self, sink: FrameSink) {
        self.frames.replace(sink);
        let _ = self.control.send(Control::Redirected);
    }

    pub fn key_frame(&self) {
        let _ = self.control.send(Control::KeyFrame);
    }

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

#[allow(clippy::too_many_arguments)]
async fn run(
    api: &Api,
    req: &Request,
    mut stop: oneshot::Receiver<()>,
    mut control: mpsc::UnboundedReceiver<Control>,
    state: &StateSink,
    frames: Frames,
    voice: super::session::VoiceEnded,
    mut rebuilt: super::session::Rebuilt,
) -> AppResult<()> {
    use futures_util::FutureExt;
    let mut voice = voice.fuse();
    let mut resumes = super::session::Resumes::new(&mut rebuilt);
    loop {
        resumes.sync(&mut rebuilt);
        let mut held = None;
        let r = watch_once(
            api,
            req,
            &mut stop,
            &mut control,
            state,
            frames.clone(),
            &mut voice,
            &mut held,
        )
        .await;
        let _ = tauri::async_runtime::spawn_blocking(move || drop(held)).await;
        let Err(e) = r else { return Ok(()) };
        let go = resumes
            .recover(e, &mut rebuilt, &mut voice, &mut stop, |e, n| {
                log::warn!("语音重建期间观看中断：{e}；等待语音恢复后重新观看 {n}/{MAX_RESUMES}");
                state(WatchState::Waiting);
            })
            .await?;
        if !go {
            return Ok(());
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn watch_once(
    api: &Api,
    req: &Request,
    stop: &mut oneshot::Receiver<()>,
    control: &mut mpsc::UnboundedReceiver<Control>,
    state: &StateSink,
    frames: Frames,
    voice: &mut super::session::Voice,
    held: &mut Option<Watch>,
) -> AppResult<()> {
    let cred =
        super::session::until_voice_ends(voice, api.go_live_watch(&req.channel_id, &req.liver))
            .await?;
    let vendor = cred.vendor;
    if vendor == Vendor::Agora {
        super::session::until_voice_ends(voice, async {
            super::agora_sdk::sync().await;
            Ok(())
        })
        .await?;
        if !super::agora_sdk::ready() {
            return Err(AppError::Other("Agora 观看组件不可用".into()));
        }
    }
    log::info!("观看厂商：{vendor:?}");
    let volc = cred.options;
    let (need_key_tx, mut need_key) = mpsc::unbounded_channel::<()>();
    let (queue, forward) = forwarder(frames, need_key_tx.clone());
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
        if let Err(TrySendError::Full(_)) = queue.try_send(pack(f)) {
            wk.store(true, Ordering::Relaxed);
            let _ = need_key_tx.send(());
        }
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let (r, v) = (req.clone(), volc.clone());
    let mut start = tauri::async_runtime::spawn_blocking(move || {
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
    });
    let started = tokio::select! {
        r = &mut start => r,
        () = &mut *voice => {
            if let Ok(Ok(w)) = start.await {
                *held = Some(w);
            }
            return Err(AppError::Other("已退出语音频道".into()));
        }
    };
    let watch = started
        .map_err(|e| AppError::Other(e.to_string()))?
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
                _ = &mut *stop => return Ok(()),
                () = &mut *voice => return Err(AppError::Other("已退出语音频道".into())),
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
    drop(forward);
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    Subscribed,
    Decoder,
    Dropped,
    Redirected,
}

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
