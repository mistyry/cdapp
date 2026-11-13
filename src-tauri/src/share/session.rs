use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::bridge::{self, BridgeError, Event, Live, LiveParams, SourceKind, Vendor};
use super::fit;
use super::loopback::{Loopback, Scope};
use crate::error::{AppError, AppResult};
use crate::kook::api::Api;
use crate::kook::golive::Tier;

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const REFRESH_EVERY: Duration = Duration::from_secs(12 * 3600);
const VIDEO_CAPTURING: i32 = 1;
const VIDEO_FAILED: i32 = 3;
const TOKEN_ERRORS: [i32; 4] = [-1000, -1010, -10008, -10009];

#[derive(Debug, PartialEq, Eq)]
enum RoomAction {
    Ignore,
    Renew,
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

enum Control {
    Update(Tier, oneshot::Sender<AppResult<()>>),
    #[cfg(any(test, feature = "test-media"))]
    Renew(oneshot::Sender<AppResult<()>>),
}

#[derive(Debug, Clone)]
pub struct Request {
    pub channel_id: String,
    pub user_id: String,
    pub source_kind: SourceKind,
    pub source_id: u64,
    pub source_pid: u32,
    pub tier: Tier,
    pub audio: bool,
    pub vendor: Vendor,
}

impl Request {
    fn audio_scope(&self) -> Scope {
        match self.source_kind {
            SourceKind::Screen => Scope::Exclude(std::process::id()),
            SourceKind::Window => Scope::Include(self.source_pid),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ShareEvent {
    Live {
        width: u32,
        height: u32,
    },
    Tier {
        resolution: u32,
        frame_rate: u32,
        width: u32,
        height: u32,
    },
    Stats {
        kbps: i32,
        fps: i32,
        width: i32,
        height: i32,
    },
    Ended {
        message: Option<String>,
    },
}

pub type Sink = std::sync::Arc<dyn Fn(ShareEvent) + Send + Sync>;

pub type VoiceEnded = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

pub type Rebuilt = tokio::sync::watch::Receiver<(u64, bool)>;

const MAX_RESUMES: u32 = 3;
const RESUME_STABLE: Duration = Duration::from_secs(60);
const REBUILD_GRACE: Duration = Duration::from_secs(5);

pub(super) type Voice = futures_util::future::Fuse<VoiceEnded>;

fn voice_gone() -> AppError {
    AppError::Other("已退出语音频道".into())
}

async fn during_rebuild(rebuilt: &mut Rebuilt, seen: u64) -> bool {
    tokio::time::timeout(
        REBUILD_GRACE,
        rebuilt.wait_for(|&(count, rebuilding)| rebuilding || count != seen),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

async fn voice_back(
    rebuilt: &mut Rebuilt,
    voice: &mut Voice,
    stop: &mut oneshot::Receiver<()>,
) -> AppResult<Option<u64>> {
    tokio::select! {
        _ = stop => Ok(None),
        () = voice => Err(voice_gone()),
        r = rebuilt.wait_for(|s| !s.1) => match r {
            Ok(s) => Ok(Some(s.0)),
            Err(_) => Err(voice_gone()),
        },
    }
}

pub(super) struct Resumes {
    seen: u64,
    count: u32,
    since: tokio::time::Instant,
}

impl Resumes {
    pub(super) fn new(rebuilt: &mut Rebuilt) -> Self {
        Self {
            seen: rebuilt.borrow_and_update().0,
            count: 0,
            since: tokio::time::Instant::now(),
        }
    }

    pub(super) fn sync(&mut self, rebuilt: &mut Rebuilt) {
        let (count, rebuilding) = *rebuilt.borrow_and_update();
        if !rebuilding {
            self.seen = count;
        }
    }

    pub(super) async fn recover(
        &mut self,
        e: AppError,
        rebuilt: &mut Rebuilt,
        voice: &mut Voice,
        stop: &mut oneshot::Receiver<()>,
        notify: impl FnOnce(&AppError, u32),
    ) -> AppResult<bool> {
        use futures_util::future::FusedFuture;
        if voice.is_terminated() {
            return Err(voice_gone());
        }
        let rebuilding = tokio::select! {
            _ = &mut *stop => return Ok(false),
            () = &mut *voice => return Err(voice_gone()),
            r = during_rebuild(rebuilt, self.seen) => r,
        };
        if !rebuilding {
            return Err(e);
        }
        if self.since.elapsed() >= RESUME_STABLE {
            self.count = 0;
        }
        if self.count >= MAX_RESUMES {
            return Err(e);
        }
        self.count += 1;
        notify(&e, self.count);
        let Some(n) = voice_back(rebuilt, voice, stop).await? else {
            return Ok(false);
        };
        self.seen = n;
        self.since = tokio::time::Instant::now();
        Ok(true)
    }
}

pub(super) async fn until_voice_ends<T>(
    voice: &mut Voice,
    fut: impl std::future::Future<Output = AppResult<T>>,
) -> AppResult<T> {
    tokio::select! {
        r = fut => r,
        () = voice => Err(voice_gone()),
    }
}

pub struct Session {
    stop: Option<oneshot::Sender<()>>,
    control: mpsc::UnboundedSender<Control>,
    done: tauri::async_runtime::JoinHandle<()>,
    pub channel_id: String,
    pub generation: u64,
}

impl Session {
    #[must_use]
    pub fn start(
        api: Api,
        req: Request,
        generation: u64,
        sink: Sink,
        voice: VoiceEnded,
        rebuilt: Rebuilt,
    ) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let (control, control_rx) = mpsc::unbounded_channel();
        let channel_id = req.channel_id.clone();
        let done = tauri::async_runtime::spawn(async move {
            let mut push = Push::default();
            let r = run(
                &api, req, stop_rx, control_rx, &sink, voice, rebuilt, &mut push,
            )
            .await;
            release(&mut push).await;
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

#[derive(Default)]
struct Push {
    live: Option<Live>,
    sound: Option<Loopback>,
    size: (u32, u32),
    source: (u32, u32),
    kbps: u32,
    vendor: Vendor,
}

impl Push {
    fn read_source(&mut self, req: &Request) -> (u32, u32) {
        if let Some(s) = super::source::size(req.source_kind, req.source_id) {
            self.source = s;
        }
        self.source
    }

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

async fn release(push: &mut Push) {
    let (sound, live) = (push.sound.take(), push.live.take());
    let _ = tauri::async_runtime::spawn_blocking(move || {
        drop(sound);
        drop(live);
    })
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn run(
    api: &Api,
    mut req: Request,
    mut stop: oneshot::Receiver<()>,
    mut control: mpsc::UnboundedReceiver<Control>,
    sink: &Sink,
    voice: VoiceEnded,
    mut rebuilt: Rebuilt,
    push: &mut Push,
) -> AppResult<()> {
    use futures_util::FutureExt;
    let mut voice = voice.fuse();
    let mut resumes = Resumes::new(&mut rebuilt);
    loop {
        resumes.sync(&mut rebuilt);
        let r = live(
            api,
            &mut req,
            &mut stop,
            &mut control,
            sink,
            &mut voice,
            push,
        )
        .await;
        let Err(e) = r else { return Ok(()) };
        release(push).await;
        let go = resumes
            .recover(e, &mut rebuilt, &mut voice, &mut stop, |e, n| {
                log::warn!(
                    "语音重建期间屏幕分享中断：{e}；等待语音恢复后重新开播 {n}/{MAX_RESUMES}"
                );
            })
            .await?;
        if !go {
            return Ok(());
        }
    }
}

async fn live(
    api: &Api,
    req: &mut Request,
    stop: &mut oneshot::Receiver<()>,
    control: &mut mpsc::UnboundedReceiver<Control>,
    sink: &Sink,
    voice: &mut Voice,
    push: &mut Push,
) -> AppResult<()> {
    let Some(mut events) = until_voice_ends(voice, begin(api, req, stop, push)).await? else {
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
            _ = &mut *stop => return Ok(()),
            () = &mut *voice => return Err(voice_gone()),
            _ = refresh.tick() => true,
            c = control.recv() => match c {
                Some(Control::Update(tier, reply)) => {
                    let r = update(api, req, tier, push).await;
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
                    let _ = reply.send(renew(api, req, push).await);
                    false
                }
                None => std::future::pending().await,
            },
            e = events.recv() => on_event(e, req, sink, push).await?,
        };
        if renew {
            self::renew(api, req, push).await?;
        }
    }
}

async fn renew(api: &Api, req: &Request, push: &mut Push) -> AppResult<()> {
    let vendor = push.vendor;
    let token = api.go_live_refresh(&req.channel_id, vendor).await?.token;
    push.call(move |l| l.token(&token)).await?;
    log::info!("屏幕分享令牌已续期");
    Ok(())
}

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

async fn on_event(
    e: Option<Event>,
    req: &Request,
    sink: &Sink,
    push: &mut Push,
) -> AppResult<bool> {
    match e {
        Some(Event::TokenWillExpire) => return Ok(true),
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
        Some(Event::SourceGone) => return Err(AppError::Other("共享的窗口已关闭".into())),
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

async fn begin(
    api: &Api,
    req: &Request,
    stop: &mut oneshot::Receiver<()>,
    push: &mut Push,
) -> AppResult<Option<mpsc::UnboundedReceiver<Event>>> {
    let got = api
        .go_live(&req.channel_id, &req.user_id, req.tier, req.vendor)
        .await?;
    let vendor = Vendor::parse(&got.vendor)
        .ok_or_else(|| AppError::Other("当前账号分配的分享服务暂不支持".into()))?;
    let cred = got
        .options
        .ok_or_else(|| AppError::Other("当前账号分配的分享服务暂不支持".into()))?;
    if vendor == Vendor::Agora {
        super::agora_sdk::sync().await;
        if !super::agora_sdk::ready() {
            return Err(AppError::Other("Agora 分享组件不可用".into()));
        }
    }
    push.vendor = vendor;
    log::info!("推流厂商：{vendor:?}");
    push.kbps = fit::kbps(got.max_bitrate);
    push.size = fit::tier_box(req.tier.resolution);
    let (tx, mut events) = mpsc::unbounded_channel();
    let (r, kbps, (width, height)) = (req.clone(), push.kbps, push.size);
    let live = blocking(move || {
        Live::start(
            vendor,
            &LiveParams {
                app_id: &cred.app_id,
                room: &cred.channel_name,
                uid: &r.user_id,
                token: &cred.token,
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
                Some(Event::VideoState { state: VIDEO_CAPTURING, .. }) if vendor == Vendor::Agora => {
                    frame.get_or_insert((0, 0));
                }
                Some(Event::VideoState { state: VIDEO_FAILED, error }) => {
                    return Err(AppError::Other(format!("屏幕采集失败：{error}")));
                }
                Some(_) => {}
                None => return Err(AppError::Other("推流已结束".into())),
            }
        }
    }
    push.source = frame.unwrap_or_default();
    let source = push.read_source(req);
    push.encode(fit::fit(req.tier.resolution, source), req.tier.frame_rate)
        .await?;
    push.call(Live::publish).await?;
    if req.audio {
        push.sound = Some(Loopback::start(req.audio_scope(), move |pcm| {
            if let Err(e) = bridge::push_audio(vendor, pcm)
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
    fn events_use_camel_case_fields() {
        let v = serde_json::to_value(ShareEvent::Tier {
            resolution: 1080,
            frame_rate: 60,
            width: 1920,
            height: 1080,
        })
        .unwrap();
        assert_eq!(v["kind"], "tier");
        assert_eq!(v["frameRate"], 60);
    }

    fn fused(f: impl std::future::Future<Output = ()> + Send + 'static) -> Voice {
        use futures_util::FutureExt;
        let v: VoiceEnded = Box::pin(f);
        v.fuse()
    }

    #[tokio::test]
    async fn startup_ends_when_voice_ends() {
        let mut voice = fused(tokio::time::sleep(std::time::Duration::from_millis(20)));
        let t = std::time::Instant::now();
        let r = until_voice_ends(&mut voice, std::future::pending::<AppResult<()>>()).await;
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(r.unwrap_err().to_string(), "已退出语音频道");
        let mut voice = fused(std::future::pending());
        assert_eq!(
            until_voice_ends(&mut voice, async { Ok(7) }).await.unwrap(),
            7
        );
    }

    fn broken() -> AppError {
        AppError::Other("分享连接已断开：-10003".into())
    }

    #[tokio::test(start_paused = true)]
    async fn voice_ended_after_rebuild_ends_share() {
        let (tx, mut rebuilt) = tokio::sync::watch::channel((0u64, false));
        let mut resumes = Resumes::new(&mut rebuilt);
        tx.send((1, false)).unwrap();
        let mut voice = fused(async {});
        let (_stop_tx, mut stop) = oneshot::channel();
        let r = until_voice_ends(&mut voice, std::future::pending::<AppResult<()>>()).await;
        let e = r.unwrap_err();
        let r = resumes
            .recover(e, &mut rebuilt, &mut voice, &mut stop, |_, _| {
                panic!("不应恢复")
            })
            .await;
        assert_eq!(r.unwrap_err().to_string(), "已退出语音频道");
    }

    #[tokio::test(start_paused = true)]
    async fn voice_ending_during_grace_ends_share() {
        let (_tx, mut rebuilt) = tokio::sync::watch::channel((0u64, false));
        let mut resumes = Resumes::new(&mut rebuilt);
        let mut voice = fused(tokio::time::sleep(Duration::from_secs(1)));
        let (_stop_tx, mut stop) = oneshot::channel();
        let r = resumes
            .recover(broken(), &mut rebuilt, &mut voice, &mut stop, |_, _| {
                panic!("不应恢复")
            })
            .await;
        assert_eq!(r.unwrap_err().to_string(), "已退出语音频道");
    }

    #[tokio::test(start_paused = true)]
    async fn stop_during_grace_returns_at_once() {
        let (_tx, mut rebuilt) = tokio::sync::watch::channel((0u64, false));
        let mut resumes = Resumes::new(&mut rebuilt);
        let mut voice = fused(std::future::pending());
        let (stop_tx, mut stop) = oneshot::channel();
        let t = tokio::time::Instant::now();
        let r = tokio::join!(
            resumes.recover(broken(), &mut rebuilt, &mut voice, &mut stop, |_, _| {
                panic!("不应恢复")
            }),
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                stop_tx.send(()).unwrap();
            }
        )
        .0;
        assert!(!r.unwrap());
        assert!(t.elapsed() < REBUILD_GRACE);
    }

    #[tokio::test(start_paused = true)]
    async fn plain_error_ends_after_grace() {
        let (_tx, mut rebuilt) = tokio::sync::watch::channel((0u64, false));
        let mut resumes = Resumes::new(&mut rebuilt);
        let mut voice = fused(std::future::pending());
        let (_stop_tx, mut stop) = oneshot::channel();
        let t = tokio::time::Instant::now();
        let r = resumes
            .recover(broken(), &mut rebuilt, &mut voice, &mut stop, |_, _| {
                panic!("不应恢复")
            })
            .await;
        assert_eq!(r.unwrap_err().to_string(), broken().to_string());
        assert!(t.elapsed() >= REBUILD_GRACE);
    }

    #[tokio::test(start_paused = true)]
    async fn plain_error_after_quiet_rebuild_is_not_resumed() {
        let (tx, mut rebuilt) = tokio::sync::watch::channel((0u64, false));
        let mut resumes = Resumes::new(&mut rebuilt);
        tx.send((0, true)).unwrap();
        resumes.sync(&mut rebuilt);
        assert_eq!(resumes.seen, 0);
        tx.send((1, false)).unwrap();
        resumes.sync(&mut rebuilt);
        assert_eq!(resumes.seen, 1);
        let mut voice = fused(std::future::pending());
        let (_stop_tx, mut stop) = oneshot::channel();
        let t = tokio::time::Instant::now();
        let r = resumes
            .recover(broken(), &mut rebuilt, &mut voice, &mut stop, |_, _| {
                panic!("不应恢复")
            })
            .await;
        assert_eq!(r.unwrap_err().to_string(), broken().to_string());
        assert!(t.elapsed() >= REBUILD_GRACE);
    }

    #[tokio::test(start_paused = true)]
    async fn resumes_after_rebuild() {
        let (tx, mut rebuilt) = tokio::sync::watch::channel((0u64, false));
        let mut resumes = Resumes::new(&mut rebuilt);
        let mut voice = fused(std::future::pending());
        let (_stop_tx, mut stop) = oneshot::channel();
        let mut notified = 0;
        tx.send((0, true)).unwrap();
        let r = tokio::join!(
            resumes.recover(broken(), &mut rebuilt, &mut voice, &mut stop, |_, n| {
                notified = n;
            }),
            async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                tx.send((1, false)).unwrap();
            }
        )
        .0;
        assert!(r.unwrap());
        assert_eq!(notified, 1);
        assert_eq!(resumes.seen, 1);
        for n in 2..=MAX_RESUMES {
            tx.send((u64::from(n), false)).unwrap();
            let r = resumes
                .recover(broken(), &mut rebuilt, &mut voice, &mut stop, |_, _| {})
                .await;
            assert!(r.unwrap());
            assert_eq!(resumes.count, n);
        }
        tx.send((9, false)).unwrap();
        let r = resumes
            .recover(broken(), &mut rebuilt, &mut voice, &mut stop, |_, _| {})
            .await;
        assert_eq!(r.unwrap_err().to_string(), broken().to_string());
    }

    #[test]
    fn classifies_room_codes() {
        assert_eq!(room_action(0), RoomAction::Ignore);
        assert_eq!(room_action(1), RoomAction::Ignore);
        assert_eq!(room_action(-1000), RoomAction::Renew);
        assert_eq!(room_action(-1010), RoomAction::Renew);
        assert_eq!(room_action(-10008), RoomAction::Renew);
        assert_eq!(room_action(-10009), RoomAction::Renew);
        for code in [-1004, -1006, -1009, -1011, -1084, -10003] {
            assert_eq!(room_action(code), RoomAction::End, "{code}");
        }
    }
}
