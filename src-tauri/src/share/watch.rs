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
const OWN_KEY_FRAME_EVERY: Duration = Duration::from_secs(5);
const GOP_LIMIT: usize = 16 << 20;
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
    Paused {
        paused: bool,
    },
}

pub type StateSink = std::sync::Arc<dyn Fn(WatchState) + Send + Sync>;
pub type FrameSink = Box<dyn Fn(Vec<u8>) -> Result<(), String> + Send>;

#[derive(Default)]
struct Gop {
    frames: Vec<Vec<u8>>,
    bytes: usize,
    broken: bool,
}

impl Gop {
    fn push(&mut self, msg: &[u8], limit: usize) {
        let key = msg.first() == Some(&1);
        if key {
            self.clear();
            self.broken = false;
        } else if self.frames.is_empty() {
            return;
        }
        if self.bytes + msg.len() > limit {
            log::warn!(
                "自看画面缓存超过上限 {limit} 字节，已清空 {} 帧",
                self.frames.len()
            );
            self.clear();
            self.broken = true;
            return;
        }
        self.bytes += msg.len();
        self.frames.push(msg.to_vec());
    }

    fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    fn complete(&self) -> bool {
        !self.frames.is_empty() && !self.broken
    }
}

struct Outlet {
    sink: FrameSink,
    gop: Gop,
    cache: bool,
    backlog: Option<Vec<Vec<u8>>>,
}

#[derive(Clone)]
pub struct Frames(
    std::sync::Arc<std::sync::Mutex<Outlet>>,
    std::sync::Arc<std::sync::Mutex<()>>,
);

impl Frames {
    #[must_use]
    pub fn new(sink: FrameSink) -> Self {
        Self(
            std::sync::Arc::new(std::sync::Mutex::new(Outlet {
                sink,
                gop: Gop::default(),
                cache: false,
                backlog: None,
            })),
            std::sync::Arc::default(),
        )
    }

    fn cache(&self, on: bool) {
        if let Ok(mut o) = self.0.lock() {
            o.cache = on;
            o.gop.clear();
            o.gop.broken = false;
        }
    }

    fn replace(&self, sink: FrameSink) -> bool {
        let Ok(_swap) = self.1.lock() else {
            return false;
        };
        let (mut batch, complete) = {
            let Ok(mut o) = self.0.lock() else {
                return false;
            };
            if !o.cache {
                o.sink = sink;
                return false;
            }
            o.backlog = Some(Vec::new());
            (o.gop.frames.clone(), o.gop.complete())
        };
        let begin = std::time::Instant::now();
        let mut sent = 0usize;
        let (delivered, whole) = loop {
            let mut ok = true;
            for m in batch {
                if sink(m).is_err() {
                    ok = false;
                    break;
                }
                sent += 1;
            }
            let Ok(mut o) = self.0.lock() else {
                return false;
            };
            let pending = o.backlog.take().unwrap_or_default();
            if !ok || pending.is_empty() {
                o.sink = sink;
                break (ok, complete && ok);
            }
            o.backlog = Some(Vec::new());
            batch = pending;
        };
        log::info!(
            "已向新窗口重放 {sent} 帧，耗时 {} 毫秒，缓存{}",
            begin.elapsed().as_millis(),
            if complete { "完整" } else { "不完整" }
        );
        if !delivered {
            log::warn!("重放画面未能全部送达新窗口");
        }
        whole
    }

    fn send(&self, msg: Vec<u8>) -> Result<(), String> {
        let mut o = self.0.lock().map_err(|e| e.to_string())?;
        if o.cache {
            o.gop.push(&msg, GOP_LIMIT);
        }
        if let Some(b) = o.backlog.as_mut() {
            b.push(msg);
            return Ok(());
        }
        (o.sink)(msg)
    }
}

struct Gate {
    waiting_key: std::sync::atomic::AtomicBool,
    want_frame: std::sync::atomic::AtomicBool,
}

impl Gate {
    fn new() -> Self {
        Self {
            waiting_key: std::sync::atomic::AtomicBool::new(true),
            want_frame: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn need_key(&self) {
        self.waiting_key
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn want_frame(&self, on: bool) {
        self.want_frame
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }

    fn pass(&self, key: bool) -> (bool, bool) {
        use std::sync::atomic::Ordering;
        if self.waiting_key.load(Ordering::Relaxed) {
            if !key {
                return (false, false);
            }
            self.waiting_key.store(false, Ordering::Relaxed);
        }
        (true, self.want_frame.swap(false, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub channel_id: String,
    pub user_id: String,
    pub liver: String,
    pub volume: u16,
}

impl Request {
    #[must_use]
    pub fn own(&self) -> bool {
        self.user_id == self.liver
    }
}

enum Control {
    KeyFrame,
    Redirected { complete: bool },
    Volume,
}

pub struct Session {
    stop: Option<oneshot::Sender<()>>,
    control: mpsc::UnboundedSender<Control>,
    done: tauri::async_runtime::JoinHandle<()>,
    frames: Frames,
    volume: Volume,
    pub liver: String,
    pub generation: u64,
}

type Volume = std::sync::Arc<std::sync::atomic::AtomicU16>;

impl Session {
    #[must_use]
    pub fn start(
        api: Api,
        req: Request,
        generation: u64,
        state: StateSink,
        frames: FrameSink,
        voice: super::push::VoiceEnded,
        rebuilt: super::push::Rebuilt,
    ) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let (control, control_rx) = mpsc::unbounded_channel();
        let liver = req.liver.clone();
        let frames = Frames::new(frames);
        let out = frames.clone();
        let volume: Volume = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(req.volume));
        let vol = volume.clone();
        let done = tauri::async_runtime::spawn(async move {
            let r = run(
                &api, &req, stop_rx, control_rx, &state, out, &vol, voice, rebuilt,
            )
            .await;
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
            volume,
            liver,
            generation,
        }
    }

    pub fn set_volume(&self, percent: u16) {
        self.volume
            .store(percent, std::sync::atomic::Ordering::Relaxed);
        let _ = self.control.send(Control::Volume);
    }

    pub fn redirect(&self, sink: FrameSink) {
        let complete = self.frames.replace(sink);
        let _ = self.control.send(Control::Redirected { complete });
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
    volume: &Volume,
    voice: super::push::VoiceEnded,
    mut rebuilt: super::push::Rebuilt,
) -> AppResult<()> {
    use futures_util::FutureExt;
    let mut voice = voice.fuse();
    let mut resumes = super::push::Resumes::new(&mut rebuilt);
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
            volume,
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

fn own_vendor(live: Option<Vendor>) -> AppResult<Vendor> {
    live.ok_or_else(|| AppError::Other("没有在屏幕分享".into()))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn watch_once(
    api: &Api,
    req: &Request,
    stop: &mut oneshot::Receiver<()>,
    control: &mut mpsc::UnboundedReceiver<Control>,
    state: &StateSink,
    frames: Frames,
    volume: &Volume,
    voice: &mut super::push::Voice,
    held: &mut Option<Watch>,
) -> AppResult<()> {
    let own = req.own();
    let cred = if own {
        None
    } else {
        let c =
            super::push::until_voice_ends(voice, api.go_live_watch(&req.channel_id, &req.liver))
                .await?;
        if c.vendor == Vendor::Agora {
            super::push::until_voice_ends(voice, async {
                super::agora_sdk::sync().await;
                Ok(())
            })
            .await?;
            if !super::agora_sdk::ready() {
                return Err(AppError::Other("Agora 观看组件不可用".into()));
            }
        }
        Some(c)
    };
    let vendor = match &cred {
        Some(c) => c.vendor,
        None => own_vendor(super::push::live_vendor())?,
    };
    if own {
        log::info!("观看自己的屏幕分享：{vendor:?}");
    } else {
        log::info!("观看厂商：{vendor:?}");
    }
    let (need_key_tx, mut need_key) = mpsc::unbounded_channel::<()>();
    let replay = own && vendor == Vendor::Volc;
    frames.cache(replay);
    let (queue, forward) = forwarder(frames, need_key_tx.clone());
    let gate = std::sync::Arc::new(Gate::new());
    let g = gate.clone();
    let (seen_tx, mut seen) = mpsc::unbounded_channel::<()>();
    let on_frame = move |f: &Frame<'_>| {
        let (pass, first) = g.pass(f.key);
        if !pass {
            return;
        }
        if first {
            let _ = seen_tx.send(());
        }
        if let Err(TrySendError::Full(_)) = queue.try_send(pack(f)) {
            g.need_key();
            let _ = need_key_tx.send(());
        }
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let r = req.clone();
    let level = volume.load(std::sync::atomic::Ordering::Relaxed);
    let mut start = tauri::async_runtime::spawn_blocking(move || {
        let sink = WatchSink {
            events: tx,
            frame: Box::new(on_frame),
        };
        let Some(c) = cred else {
            return Watch::own(vendor, sink);
        };
        let v = c.options;
        Watch::start(
            vendor,
            &WatchParams {
                app_id: &v.app_id,
                room: &v.channel_name,
                uid: &r.user_id,
                token: &v.token,
                liver: &r.liver,
                volume: level,
            },
            sink,
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
        let mut refresh = tokio::time::interval(REFRESH_EVERY);
        refresh.tick().await;
        let mut last_key: Option<tokio::time::Instant> = None;
        let mut paused = false;
        let mut waiting = false;
        let wait = |waiting: &mut bool| {
            *waiting = true;
            gate.want_frame(true);
            state(WatchState::Waiting);
        };
        loop {
            tokio::select! {
                _ = &mut *stop => return Ok(()),
                () = &mut *voice => return Err(AppError::Other("已退出语音频道".into())),
                () = &mut deadline, if !joined => return Err(AppError::Other("进入分享房间超时".into())),
                _ = refresh.tick() => renew(api, req, held).await?,
                Some(()) = seen.recv() => {
                    if waiting && !paused {
                        waiting = false;
                        log::info!("等待中收到画面，恢复播放");
                        state(WatchState::Playing);
                    }
                }
                c = control.recv() => match c {
                    Some(Control::KeyFrame) => {
                        gate.need_key();
                        request_key(held, &mut last_key, Why::Decoder).await;
                    }
                    Some(Control::Redirected { complete }) => {
                        if !replay {
                            gate.need_key();
                        }
                        if paused {
                            state(WatchState::Paused { paused });
                        }
                        let why = if replay && !complete { Why::Incomplete } else { Why::Redirected };
                        request_key(held, &mut last_key, why).await;
                    }
                    Some(Control::Volume) => {
                        let level = volume.load(std::sync::atomic::Ordering::Relaxed);
                        if let Err(e) = call(held, move |w| w.volume(level)).await {
                            log::warn!("调整屏幕共享音量失败：{e}");
                        }
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
                            wait(&mut waiting);
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
                        gate.need_key();
                        call(held, super::bridge::Watch::subscribe).await?;
                        log::info!("分享者发布屏幕流，已订阅");
                        waiting = false;
                        gate.want_frame(false);
                        state(WatchState::Playing);
                        request_key(held, &mut last_key, Why::Subscribed).await;
                    }
                    Some(WatchEvent::Unpublished(reason)) => {
                        log::info!("分享者取消发布屏幕流：{reason}");
                        gate.need_key();
                        if joined && !paused {
                            wait(&mut waiting);
                        }
                    }
                    Some(WatchEvent::Paused(p)) => {
                        if p != paused {
                            paused = p;
                            log::info!("分享者{}屏幕分享", if p { "暂停了" } else { "恢复了" });
                            state(WatchState::Paused { paused: p });
                        }
                        rearm(&gate, paused, waiting);
                        if !p {
                            request_key(held, &mut last_key, Why::Resumed).await;
                        }
                    }
                    Some(WatchEvent::TokenWillExpire) => renew(api, req, held).await?,
                    Some(WatchEvent::Stats { kbps, width, height, stalls }) => {
                        state(WatchState::Stats { kbps, width, height, stalls });
                    }
                    Some(WatchEvent::Volume { raw: 0, volume, why }) => {
                        log::info!("已应用屏幕共享音量 {volume}：{why}");
                    }
                    Some(WatchEvent::Volume { raw, volume, why }) => {
                        log::warn!("应用屏幕共享音量 {volume} 失败：{raw}，{why}");
                    }
                    Some(WatchEvent::Ended) => {
                        log::info!("屏幕分享已停止，结束观看自己");
                        return Ok(());
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
    Incomplete,
    Resumed,
}

fn rearm(gate: &Gate, paused: bool, waiting: bool) {
    if waiting && !paused {
        gate.want_frame(true);
    }
}

async fn request_key(held: &mut Option<Watch>, last: &mut Option<tokio::time::Instant>, why: Why) {
    let Some(target) = held.as_ref().map(|w| (w.is_own(), w.vendor())) else {
        return;
    };
    let now = tokio::time::Instant::now();
    let Some(every) = key_interval(target, why) else {
        return;
    };
    if last.is_some_and(|t| now - t < every) {
        return;
    }
    *last = Some(now);
    match target {
        (false, Vendor::Agora) => log::info!("Agora 画面需要关键帧（{why:?}），重新进入分享房间"),
        (true, Vendor::Volc) => log::info!("自看画面需要关键帧（{why:?}），推流端重建编码"),
        _ => {}
    }
    if let Err(e) = call(held, super::bridge::Watch::key_frame).await {
        log::debug!("请求关键帧失败：{e}");
    }
}

fn key_interval((own, vendor): (bool, Vendor), why: Why) -> Option<Duration> {
    Some(match (own, vendor, why) {
        (true, Vendor::Volc, Why::Decoder | Why::Dropped | Why::Incomplete) => OWN_KEY_FRAME_EVERY,
        (true, Vendor::Volc, _) | (false, Vendor::Agora, Why::Subscribed | Why::Resumed) => {
            return None;
        }
        (_, _, Why::Subscribed | Why::Redirected | Why::Incomplete | Why::Resumed) => {
            Duration::ZERO
        }
        (false, Vendor::Agora, _) => AGORA_REJOIN_EVERY,
        _ => KEY_FRAME_EVERY,
    })
}

async fn renew(api: &Api, req: &Request, held: &mut Option<Watch>) -> AppResult<()> {
    if req.own() {
        return Ok(());
    }
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
    fn own_request_is_same_user() {
        let mut r = Request {
            channel_id: "c".into(),
            user_id: "1".into(),
            liver: "1".into(),
            volume: 100,
        };
        assert!(r.own());
        r.liver = "2".into();
        assert!(!r.own());
    }

    #[test]
    fn own_follows_live_vendor() {
        assert_eq!(own_vendor(Some(Vendor::Agora)).unwrap(), Vendor::Agora);
        assert_eq!(own_vendor(Some(Vendor::Volc)).unwrap(), Vendor::Volc);
        assert_eq!(own_vendor(None).unwrap_err().to_string(), "没有在屏幕分享");
    }

    #[test]
    fn key_frame_intervals() {
        let agora = (false, Vendor::Agora);
        let volc = (false, Vendor::Volc);
        let own_agora = (true, Vendor::Agora);
        assert_eq!(key_interval(agora, Why::Subscribed), None);
        assert_eq!(key_interval(agora, Why::Resumed), None);
        assert_eq!(key_interval(agora, Why::Dropped), Some(AGORA_REJOIN_EVERY));
        assert_eq!(key_interval(volc, Why::Decoder), Some(KEY_FRAME_EVERY));
        assert_eq!(key_interval(volc, Why::Resumed), Some(Duration::ZERO));
        assert_eq!(key_interval(volc, Why::Redirected), Some(Duration::ZERO));
        assert_eq!(key_interval(own_agora, Why::Decoder), Some(KEY_FRAME_EVERY));
        assert_eq!(
            key_interval(own_agora, Why::Subscribed),
            Some(Duration::ZERO)
        );
        assert_eq!(
            key_interval(own_agora, Why::Redirected),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn own_volc_resizes_only_when_replay_cannot_help() {
        let own = (true, Vendor::Volc);
        for why in [Why::Decoder, Why::Dropped, Why::Incomplete] {
            assert_eq!(key_interval(own, why), Some(OWN_KEY_FRAME_EVERY), "{why:?}");
        }
        assert!(OWN_KEY_FRAME_EVERY >= Duration::from_secs(5));
        for why in [Why::Subscribed, Why::Redirected, Why::Resumed] {
            assert_eq!(key_interval(own, why), None, "{why:?}");
        }
    }

    #[test]
    fn redirect_reports_incomplete_cache() {
        let frames = Frames::new(Box::new(|_| Ok(())));
        frames.cache(true);
        assert!(!frames.replace(Box::new(|_| Ok(()))));
        frames.send(msg(true, 4)).unwrap();
        assert!(frames.replace(Box::new(|_| Ok(()))));
        frames.send(msg(false, GOP_LIMIT)).unwrap();
        assert!(!frames.replace(Box::new(|_| Ok(()))));
        frames.send(msg(false, 4)).unwrap();
        assert!(!frames.replace(Box::new(|_| Ok(()))));
        frames.send(msg(true, 4)).unwrap();
        assert!(frames.replace(Box::new(|_| Ok(()))));
        assert!(!frames.replace(Box::new(|_| Err("closed".into()))));
    }

    #[test]
    fn gop_marks_overflow_until_next_key() {
        let mut g = Gop::default();
        g.push(&msg(true, 60), 100);
        assert!(g.complete());
        g.push(&msg(false, 60), 100);
        assert!(g.broken);
        assert!(!g.complete());
        g.push(&msg(true, 10), 100);
        assert!(g.complete());
    }

    #[test]
    fn replay_keeps_order_with_concurrent_frames() {
        use std::sync::{Arc, Mutex, mpsc};
        let frames = Frames::new(Box::new(|_| Ok(())));
        frames.cache(true);
        frames.send(vec![1, 0]).unwrap();
        for i in 1..=3u8 {
            frames.send(vec![0, i]).unwrap();
        }
        let got = Arc::new(Mutex::new(Vec::new()));
        let (entered_tx, entered) = mpsc::channel::<()>();
        let (go, go_rx) = mpsc::channel::<()>();
        let go_rx = Mutex::new(go_rx);
        let g = got.clone();
        let sink: FrameSink = Box::new(move |m| {
            if m[1] == 0 {
                let _ = entered_tx.send(());
                let _ = go_rx.lock().unwrap().recv();
            }
            g.lock().unwrap().push(m[1]);
            Ok(())
        });
        let f = frames.clone();
        let replay = std::thread::spawn(move || f.replace(sink));
        entered.recv().unwrap();
        for i in 4..=6u8 {
            frames.send(vec![0, i]).unwrap();
        }
        go.send(()).unwrap();
        assert!(replay.join().unwrap());
        frames.send(vec![0, 7]).unwrap();
        assert_eq!(*got.lock().unwrap(), (0..=7).collect::<Vec<u8>>());
    }

    #[test]
    fn resume_rearms_first_frame_while_waiting() {
        let g = Gate::new();
        assert_eq!(g.pass(true), (true, false));
        g.want_frame(true);
        assert_eq!(g.pass(false), (true, true));
        rearm(&g, true, true);
        assert_eq!(g.pass(false), (true, false));
        rearm(&g, false, false);
        assert_eq!(g.pass(false), (true, false));
        rearm(&g, false, true);
        assert_eq!(g.pass(false), (true, true));
    }

    fn msg(key: bool, len: usize) -> Vec<u8> {
        let mut m = vec![0u8; len];
        m[0] = u8::from(key);
        m
    }

    #[test]
    fn gop_starts_at_key_frame() {
        let mut g = Gop::default();
        g.push(&msg(false, 10), 100);
        assert_eq!(g.frames.len(), 0);
        g.push(&msg(true, 10), 100);
        g.push(&msg(false, 10), 100);
        assert_eq!(g.frames.len(), 2);
        g.push(&msg(true, 20), 100);
        assert_eq!(g.frames.len(), 1);
        assert_eq!(g.bytes, 20);
    }

    #[test]
    fn gop_drops_when_over_limit() {
        let mut g = Gop::default();
        g.push(&msg(true, 60), 100);
        g.push(&msg(false, 30), 100);
        assert_eq!(g.bytes, 90);
        g.push(&msg(false, 30), 100);
        assert_eq!(g.frames.len(), 0);
        assert_eq!(g.bytes, 0);
        g.push(&msg(false, 10), 100);
        assert_eq!(g.frames.len(), 0);
        g.push(&msg(true, 200), 100);
        assert_eq!(g.frames.len(), 0);
    }

    #[test]
    fn redirect_replays_cached_gop() {
        use std::sync::{Arc, Mutex};
        let frames = Frames::new(Box::new(|_| Ok(())));
        frames.cache(true);
        frames.send(msg(false, 4)).unwrap();
        frames.send(msg(true, 4)).unwrap();
        frames.send(msg(false, 4)).unwrap();
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        frames.replace(Box::new(move |m| {
            g.lock().unwrap().push(m[0]);
            Ok(())
        }));
        assert_eq!(*got.lock().unwrap(), vec![1, 0]);
        frames.send(msg(false, 4)).unwrap();
        assert_eq!(*got.lock().unwrap(), vec![1, 0, 0]);
    }

    #[test]
    fn no_replay_without_cache() {
        use std::sync::{Arc, Mutex};
        let frames = Frames::new(Box::new(|_| Ok(())));
        frames.send(msg(true, 4)).unwrap();
        let got = Arc::new(Mutex::new(0));
        let g = got.clone();
        frames.replace(Box::new(move |_| {
            *g.lock().unwrap() += 1;
            Ok(())
        }));
        assert_eq!(*got.lock().unwrap(), 0);
    }

    #[test]
    fn gate_waits_for_key_and_reports_first_frame() {
        let g = Gate::new();
        assert_eq!(g.pass(false), (false, false));
        assert_eq!(g.pass(true), (true, false));
        g.want_frame(true);
        assert_eq!(g.pass(false), (true, true));
        assert_eq!(g.pass(false), (true, false));
        g.need_key();
        g.want_frame(true);
        assert_eq!(g.pass(false), (false, false));
        assert_eq!(g.pass(true), (true, true));
    }

    #[test]
    fn paused_state_serializes() {
        let v = serde_json::to_value(WatchState::Paused { paused: true }).unwrap();
        assert_eq!(v["kind"], "paused");
        assert_eq!(v["paused"], true);
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
