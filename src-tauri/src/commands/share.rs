use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ShareSdk {
    Ready { version: String },
    Unavailable { message: String },
}

#[tauri::command]
pub async fn share_sdk_sync() -> ShareSdk {
    #[cfg(windows)]
    {
        let s = crate::share::volc_sdk::sync().await;
        if matches!(s, ShareSdk::Ready { .. }) {
            tauri::async_runtime::spawn(crate::share::agora_sdk::sync());
        }
        s
    }
    #[cfg(not(windows))]
    ShareSdk::Unavailable {
        message: "当前系统暂不支持屏幕共享".into(),
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::sync::{Arc, Mutex};

    use serde::{Deserialize, Serialize};
    use tauri::{AppHandle, Emitter, State};

    use super::super::{AppState, VoiceState, api, lock};
    use crate::error::{AppError, AppResult};
    use crate::kook::golive::Tier;
    use crate::share::bridge::{self, Source, SourceKind, volc};
    use crate::share::push::{Request, Session, ShareEvent, Sink};

    #[derive(Default)]
    pub struct ShareSlot {
        pub session: Option<Session>,
        pub generation: u64,
    }

    #[derive(Default)]
    pub struct ShareState(pub tokio::sync::Mutex<ShareSlot>);

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct SharePayload {
        generation: u64,
        event: ShareEvent,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ShareStart {
        channel_id: String,
        source_kind: SourceKind,
        source_id: String,
        #[serde(default)]
        source_pid: u32,
        resolution: u32,
        frame_rate: u32,
        #[serde(default)]
        audio: bool,
    }

    fn ready() -> AppResult<()> {
        match crate::share::volc_sdk::status() {
            Some(crate::share::volc_sdk::Status::Ready { .. }) => Ok(()),
            _ => Err(AppError::Other("屏幕共享组件不可用".into())),
        }
    }

    #[tauri::command]
    pub async fn share_preview_open() -> AppResult<()> {
        ready()?;
        tauri::async_runtime::spawn_blocking(volc::preview_open)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?
            .map_err(|e| AppError::Other(e.to_string()))
    }

    #[tauri::command]
    pub async fn share_preview_close() -> AppResult<()> {
        ready()?;
        tauri::async_runtime::spawn_blocking(volc::preview_close)
            .await
            .map_err(|e| AppError::Other(e.to_string()))
    }

    #[tauri::command]
    pub async fn share_sources() -> AppResult<Vec<Source>> {
        ready()?;
        tauri::async_runtime::spawn_blocking(volc::sources)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?
            .map_err(|e| AppError::Other(e.to_string()))
    }

    #[tauri::command]
    pub async fn share_thumbnail(kind: SourceKind, id: String) -> AppResult<Option<String>> {
        ready()?;
        let id = bridge::parse_id(&id).map_err(|e| AppError::Other(e.to_string()))?;
        tauri::async_runtime::spawn_blocking(move || crate::share::source::thumbnail(kind, id))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
    }

    #[tauri::command]
    pub async fn share_start(
        params: ShareStart,
        app: AppHandle,
        state: State<'_, Mutex<AppState>>,
        voice: State<'_, VoiceState>,
        share: State<'_, ShareState>,
    ) -> AppResult<u64> {
        share_start_inner(params, app, state, voice, share)
            .await
            .inspect_err(|e| log::warn!("屏幕共享开始失败：{e}"))
    }

    async fn share_start_inner(
        params: ShareStart,
        app: AppHandle,
        state: State<'_, Mutex<AppState>>,
        voice: State<'_, VoiceState>,
        share: State<'_, ShareState>,
    ) -> AppResult<u64> {
        ready()?;
        let source_id =
            bridge::parse_id(&params.source_id).map_err(|e| AppError::Other(e.to_string()))?;
        let api = api(&state)?;
        super::super::check_loaded(
            &state,
            &api,
            &params.channel_id,
            crate::kook::gate::Need::Connect,
        )
        .await?;
        let (me, guild) = {
            let s = lock(&state)?;
            let me = s.gate.me.clone().ok_or(AppError::NotLoggedIn)?;
            let guild = s
                .gate
                .guild_of(&params.channel_id)
                .ok_or_else(|| AppError::Other("尚未取得频道信息".into()))?
                .to_owned();
            (me, guild)
        };
        let (voice_ended, rebuilt) = {
            let slot = voice.0.lock().await;
            match slot
                .session
                .as_ref()
                .filter(|s| s.channel_id == params.channel_id && !s.is_finished())
            {
                Some(s) => (s.ended(), s.rebuilt()),
                None => return Err(AppError::Other("请先加入该语音频道".into())),
            }
        };
        let vendor = api.golive_vendor(&guild, &params.channel_id, &me).await?;
        let vendor = crate::kook::golive::Vendor::parse(&vendor)
            .ok_or_else(|| AppError::Other("当前账号分配的分享服务暂不支持".into()))?;
        let mut slot = share.0.lock().await;
        if let Some(old) = slot.session.take() {
            old.stop().await;
        }
        slot.generation += 1;
        let generation = slot.generation;
        let sink: Sink = Arc::new(move |event| {
            if let Err(e) = app.emit("share://event", SharePayload { generation, event }) {
                log::warn!("事件广播失败：{e}");
            }
        });
        slot.session = Some(Session::start(
            api,
            Request {
                channel_id: params.channel_id,
                user_id: me,
                source_kind: params.source_kind,
                source_id,
                source_pid: params.source_pid,
                tier: Tier {
                    resolution: params.resolution,
                    frame_rate: params.frame_rate,
                    mode: 0,
                },
                audio: params.audio,
                vendor,
            },
            generation,
            sink,
            Box::pin(voice_ended.wait()),
            rebuilt,
        ));
        Ok(generation)
    }

    #[tauri::command]
    pub async fn share_stop(share: State<'_, ShareState>) -> AppResult<()> {
        let old = share.0.lock().await.session.take();
        if let Some(s) = old {
            s.stop().await;
        }
        Ok(())
    }

    #[tauri::command]
    pub async fn share_update(
        generation: u64,
        resolution: u32,
        frame_rate: u32,
        share: State<'_, ShareState>,
    ) -> AppResult<()> {
        let slot = share.0.lock().await;
        let session = slot
            .session
            .as_ref()
            .filter(|s| s.generation == generation && !s.is_finished())
            .ok_or_else(|| AppError::Other("屏幕共享已结束".into()))
            .inspect_err(|e| log::warn!("屏幕共享改档失败：{e}"))?;
        session
            .update(Tier {
                resolution,
                frame_rate,
                mode: 0,
            })
            .await
            .inspect_err(|e| log::warn!("屏幕共享改档失败：{e}"))
    }

    #[derive(Default)]
    pub struct WatchSlot {
        pub session: Option<crate::share::watch::Session>,
        pub generation: u64,
    }

    #[derive(Default)]
    pub struct WatchState(pub tokio::sync::Mutex<WatchSlot>);

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct WatchPayload {
        generation: u64,
        state: crate::share::watch::WatchState,
    }

    #[tauri::command]
    #[allow(clippy::too_many_arguments)]
    pub async fn watch_start(
        channel_id: String,
        liver: String,
        volume: u16,
        frames: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
        app: AppHandle,
        state: State<'_, Mutex<AppState>>,
        voice: State<'_, VoiceState>,
        watch: State<'_, WatchState>,
    ) -> AppResult<u64> {
        watch_start_inner(channel_id, liver, volume, frames, app, state, voice, watch)
            .await
            .inspect_err(|e| log::warn!("观看屏幕共享失败：{e}"))
    }

    #[allow(clippy::too_many_arguments)]
    async fn watch_start_inner(
        channel_id: String,
        liver: String,
        volume: u16,
        frames: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
        app: AppHandle,
        state: State<'_, Mutex<AppState>>,
        voice: State<'_, VoiceState>,
        watch: State<'_, WatchState>,
    ) -> AppResult<u64> {
        use crate::share::watch::{Request, Session, StateSink};
        ready()?;
        let api = api(&state)?;
        super::super::check_loaded(&state, &api, &channel_id, crate::kook::gate::Need::Connect)
            .await?;
        let me = lock(&state)?.gate.me.clone().ok_or(AppError::NotLoggedIn)?;
        let (voice_ended, rebuilt) = {
            let slot = voice.0.lock().await;
            match slot
                .session
                .as_ref()
                .filter(|s| s.channel_id == channel_id && !s.is_finished())
            {
                Some(s) => (s.ended(), s.rebuilt()),
                None => return Err(AppError::Other("请先加入该语音频道".into())),
            }
        };
        let mut slot = watch.0.lock().await;
        if let Some(old) = slot.session.take() {
            old.stop().await;
        }
        slot.generation += 1;
        let generation = slot.generation;
        let sink: StateSink = Arc::new(move |s| {
            if let Err(e) = app.emit(
                "watch://event",
                WatchPayload {
                    generation,
                    state: s,
                },
            ) {
                log::warn!("事件广播失败：{e}");
            }
        });
        let frame_sink = Box::new(move |msg: Vec<u8>| {
            frames
                .send(tauri::ipc::InvokeResponseBody::Raw(msg))
                .map_err(|e| e.to_string())
        });
        log::info!("开始观看屏幕共享");
        slot.session = Some(Session::start(
            api,
            Request {
                channel_id,
                user_id: me,
                liver,
                volume: volume.min(crate::audio::mixer::MAX_VOLUME),
            },
            generation,
            sink,
            frame_sink,
            Box::pin(voice_ended.wait()),
            rebuilt,
        ));
        Ok(generation)
    }

    #[tauri::command]
    pub async fn watch_stop(watch: State<'_, WatchState>) -> AppResult<()> {
        let old = watch.0.lock().await.session.take();
        if let Some(s) = old {
            s.stop().await;
        }
        Ok(())
    }

    #[tauri::command]
    pub async fn watch_key_frame(generation: u64, watch: State<'_, WatchState>) -> AppResult<()> {
        let slot = watch.0.lock().await;
        if let Some(s) = slot
            .session
            .as_ref()
            .filter(|s| s.generation == generation && !s.is_finished())
        {
            s.key_frame();
        }
        Ok(())
    }

    #[tauri::command]
    pub async fn watch_volume(
        generation: u64,
        percent: u16,
        watch: State<'_, WatchState>,
    ) -> AppResult<()> {
        let slot = watch.0.lock().await;
        let s = slot
            .session
            .as_ref()
            .filter(|s| s.generation == generation && !s.is_finished())
            .ok_or_else(|| AppError::Other("观看已结束".into()))?;
        s.set_volume(percent.min(crate::audio::mixer::MAX_VOLUME));
        Ok(())
    }

    #[tauri::command]
    pub async fn watch_redirect(
        generation: u64,
        frames: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
        watch: State<'_, WatchState>,
    ) -> AppResult<()> {
        let slot = watch.0.lock().await;
        let s = slot
            .session
            .as_ref()
            .filter(|s| s.generation == generation && !s.is_finished())
            .ok_or_else(|| AppError::Other("观看已结束".into()))?;
        s.redirect(Box::new(move |msg: Vec<u8>| {
            frames
                .send(tauri::ipc::InvokeResponseBody::Raw(msg))
                .map_err(|e| e.to_string())
        }));
        log::info!("屏幕共享画面改发到新窗口");
        Ok(())
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
    pub struct PipId {
        generation: u64,
        seq: u64,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    enum PipSlot {
        #[default]
        None,
        Open(PipId),
        Closing,
    }

    #[derive(Default)]
    pub struct PipState {
        slot: Mutex<PipSlot>,
        closed: tokio::sync::Notify,
    }

    impl PipState {
        fn slot(&self) -> std::sync::MutexGuard<'_, PipSlot> {
            self.slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn current(&self) -> Option<PipId> {
            match *self.slot() {
                PipSlot::Open(id) => Some(id),
                _ => None,
            }
        }

        fn begin_close(&self, destroy: impl FnOnce() -> AppResult<bool>) -> AppResult<()> {
            let prev = {
                let mut slot = self.slot();
                let PipSlot::Open(prev) = *slot else {
                    return Ok(());
                };
                *slot = PipSlot::Closing;
                prev
            };
            let reset = match destroy() {
                Ok(alive) => !alive,
                Err(e) => {
                    {
                        let mut slot = self.slot();
                        if *slot == PipSlot::Closing {
                            *slot = PipSlot::Open(prev);
                        }
                    }
                    self.closed.notify_waiters();
                    return Err(e);
                }
            };
            if reset {
                *self.slot() = PipSlot::None;
                self.closed.notify_waiters();
            }
            Ok(())
        }

        fn reset_if_gone(&self, exists: impl Fn() -> bool) -> bool {
            if exists() {
                return false;
            }
            let mut slot = self.slot();
            if *slot == PipSlot::Closing {
                *slot = PipSlot::None;
            }
            true
        }

        async fn claim(
            &self,
            id: PipId,
            wait: std::time::Duration,
            exists: impl Fn() -> bool,
            destroy: impl Fn() -> AppResult<bool>,
        ) -> AppResult<()> {
            let mut retried = false;
            loop {
                let notified = {
                    let mut slot = self.slot();
                    match *slot {
                        PipSlot::None => {
                            *slot = PipSlot::Open(id);
                            return Ok(());
                        }
                        PipSlot::Open(_) => None,
                        PipSlot::Closing => Some(self.closed.notified()),
                    }
                };
                let Some(n) = notified else {
                    self.begin_close(&destroy)?;
                    continue;
                };
                if self.reset_if_gone(&exists) {
                    continue;
                }
                if tokio::time::timeout(wait, n).await.is_err() {
                    if retried || !self.reset_if_gone(&exists) {
                        return Err(AppError::Other("悬浮窗未能关闭".into()));
                    }
                    retried = true;
                }
            }
        }

        fn destroyed(&self, id: PipId) {
            {
                let mut slot = self.slot();
                if matches!(*slot, PipSlot::Closing) || *slot == PipSlot::Open(id) {
                    *slot = PipSlot::None;
                }
            }
            self.closed.notify_waiters();
        }

        fn failed(&self, id: PipId) {
            let mut slot = self.slot();
            if *slot == PipSlot::Open(id) {
                *slot = PipSlot::None;
            }
        }
    }

    fn destroy_pip(app: &AppHandle) -> AppResult<bool> {
        use tauri::Manager;
        let Some(w) = app.get_webview_window(PIP_LABEL) else {
            return Ok(false);
        };
        w.destroy().map_err(|e| AppError::Other(e.to_string()))?;
        Ok(true)
    }

    #[tauri::command]
    pub async fn watch_pip_open(
        generation: u64,
        seq: u64,
        app: AppHandle,
        pips: State<'_, PipState>,
    ) -> AppResult<()> {
        use tauri::Manager;
        let id = PipId { generation, seq };
        pips.claim(
            id,
            PIP_CLOSE_WAIT,
            || app.get_webview_window(PIP_LABEL).is_some(),
            || destroy_pip(&app),
        )
        .await?;
        let pip = build_pip(&app).inspect_err(|_| pips.failed(id))?;
        crate::pipsize::set_ratio(16, 9);
        let target = pip.clone();
        if let Err(e) = app.run_on_main_thread(move || crate::pipsize::init(&target)) {
            log::warn!("无法安装悬浮窗子类: {e}");
        }
        let handle = app.clone();
        pip.on_window_event(move |e| {
            if matches!(e, tauri::WindowEvent::Destroyed) {
                use tauri::Emitter;
                handle.state::<PipState>().destroyed(id);
                let _ = handle.emit_to("main", PIP_CLOSED_EVENT, id);
            }
        });
        Ok(())
    }

    fn build_pip(app: &AppHandle) -> AppResult<tauri::WebviewWindow> {
        use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
        let (x, y) = app
            .get_webview_window("main")
            .and_then(|m| m.current_monitor().ok().flatten())
            .map_or((100.0, 100.0), |mon| {
                let s = mon.scale_factor();
                let (p, z) = (mon.position(), mon.size());
                (
                    f64::from(p.x) / s + f64::from(z.width) / s - PIP_W - 24.0,
                    f64::from(p.y) / s + f64::from(z.height) / s - PIP_H - 72.0,
                )
            });
        WebviewWindowBuilder::new(app, PIP_LABEL, WebviewUrl::App("index.html?pip".into()))
            .title("屏幕共享")
            .inner_size(PIP_W, PIP_H)
            .min_inner_size(crate::pipsize::MIN_W, crate::pipsize::MIN_H)
            .position(x, y)
            .visible(false)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(true)
            .focused(false)
            .build()
            .map_err(|e| AppError::Other(e.to_string()))
    }

    #[tauri::command]
    pub async fn watch_pip_current(pips: State<'_, PipState>) -> AppResult<Option<PipId>> {
        Ok(pips.current())
    }

    #[tauri::command]
    pub async fn watch_pip_ratio(
        width: u32,
        height: u32,
        app: AppHandle,
        pips: State<'_, PipState>,
    ) -> AppResult<()> {
        use tauri::Manager;
        if width == 0 || height == 0 || pips.current().is_none() {
            return Ok(());
        }
        let Some(w) = app.get_webview_window(PIP_LABEL) else {
            return Ok(());
        };
        crate::pipsize::set_ratio(width, height);
        let size = w.inner_size().map_err(|e| AppError::Other(e.to_string()))?;
        let scale = w.scale_factor().unwrap_or(1.0);
        #[allow(clippy::cast_possible_truncation)]
        let min = |v: f64| (v * scale).round() as i64;
        let ratio = (i64::from(width), i64::from(height));
        let mut cw = i64::from(size.width).max(min(crate::pipsize::MIN_W));
        let mut ch = crate::pipsize::height_for(cw, ratio);
        if ch < min(crate::pipsize::MIN_H) {
            ch = min(crate::pipsize::MIN_H);
            cw = crate::pipsize::width_for(ch, ratio);
        }
        let cw = u32::try_from(cw).unwrap_or(size.width).max(1);
        let ch = u32::try_from(ch).unwrap_or(size.height).max(1);
        if (cw, ch) != (size.width, size.height) {
            w.set_size(tauri::PhysicalSize::new(cw, ch))
                .map_err(|e| AppError::Other(e.to_string()))?;
        }
        Ok(())
    }

    #[tauri::command]
    pub async fn watch_pip_close(app: AppHandle, pips: State<'_, PipState>) -> AppResult<()> {
        pips.begin_close(|| destroy_pip(&app))
    }

    const PIP_LABEL: &str = "pip";
    const PIP_CLOSED_EVENT: &str = "watch://pip-closed";
    const PIP_CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
    const PIP_W: f64 = 480.0;
    const PIP_H: f64 = 270.0;

    #[cfg(test)]
    mod pip_tests {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        use super::*;

        const A: PipId = PipId {
            generation: 7,
            seq: 1,
        };
        const B: PipId = PipId {
            generation: 7,
            seq: 2,
        };
        const WAIT: Duration = Duration::from_secs(3);

        fn slot(p: &PipState) -> PipSlot {
            *p.slot()
        }

        #[tokio::test]
        async fn opens_from_none() {
            let p = PipState::default();
            p.claim(A, WAIT, || false, || panic!("不应关闭"))
                .await
                .unwrap();
            assert_eq!(slot(&p), PipSlot::Open(A));
            assert_eq!(p.current(), Some(A));
        }

        #[test]
        fn close_waits_for_destroyed() {
            let p = PipState::default();
            *p.slot() = PipSlot::Open(A);
            p.begin_close(|| Ok(true)).unwrap();
            assert_eq!(slot(&p), PipSlot::Closing);
            assert_eq!(p.current(), None);
            p.begin_close(|| panic!("不应重复关闭")).unwrap();
            p.destroyed(A);
            assert_eq!(slot(&p), PipSlot::None);
        }

        #[test]
        fn close_without_window_resets() {
            let p = PipState::default();
            *p.slot() = PipSlot::Open(A);
            p.begin_close(|| Ok(false)).unwrap();
            assert_eq!(slot(&p), PipSlot::None);
        }

        #[tokio::test(start_paused = true)]
        async fn reopen_waits_until_destroyed() {
            let p = std::sync::Arc::new(PipState::default());
            *p.slot() = PipSlot::Open(A);
            let destroys = std::sync::Arc::new(AtomicUsize::new(0));
            let task = {
                let (p, d) = (p.clone(), destroys.clone());
                tokio::spawn(async move {
                    p.claim(
                        B,
                        WAIT,
                        || true,
                        || {
                            d.fetch_add(1, Ordering::SeqCst);
                            Ok(true)
                        },
                    )
                    .await
                })
            };
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert_eq!(destroys.load(Ordering::SeqCst), 1);
            assert_eq!(slot(&p), PipSlot::Closing);
            p.destroyed(A);
            task.await.unwrap().unwrap();
            assert_eq!(slot(&p), PipSlot::Open(B));
        }

        #[tokio::test(start_paused = true)]
        async fn reopen_times_out() {
            let p = PipState::default();
            *p.slot() = PipSlot::Closing;
            assert!(p.claim(B, WAIT, || true, || Ok(true)).await.is_err());
            assert_eq!(slot(&p), PipSlot::Closing);
        }

        #[tokio::test(start_paused = true)]
        async fn closing_without_window_opens_immediately() {
            let p = PipState::default();
            *p.slot() = PipSlot::Closing;
            let start = tokio::time::Instant::now();
            p.claim(B, WAIT, || false, || panic!("不应关闭"))
                .await
                .unwrap();
            assert_eq!(start.elapsed(), Duration::ZERO);
            assert_eq!(slot(&p), PipSlot::Open(B));
        }

        #[tokio::test(start_paused = true)]
        async fn timeout_retries_once_window_gone() {
            let p = PipState::default();
            *p.slot() = PipSlot::Closing;
            let checks = AtomicUsize::new(0);
            p.claim(
                B,
                WAIT,
                || checks.fetch_add(1, Ordering::SeqCst) == 0,
                || panic!("不应关闭"),
            )
            .await
            .unwrap();
            assert_eq!(slot(&p), PipSlot::Open(B));
            p.destroyed(A);
            assert_eq!(slot(&p), PipSlot::Open(B));
        }

        #[test]
        fn destroy_error_restores_open() {
            let p = PipState::default();
            *p.slot() = PipSlot::Open(A);
            assert!(p.begin_close(|| Err(AppError::Other("x".into()))).is_err());
            assert_eq!(slot(&p), PipSlot::Open(A));
        }

        #[tokio::test(start_paused = true)]
        async fn claim_reports_destroy_error() {
            let p = PipState::default();
            *p.slot() = PipSlot::Open(A);
            let r = p
                .claim(B, WAIT, || true, || Err(AppError::Other("x".into())))
                .await;
            assert!(r.is_err());
            assert_eq!(slot(&p), PipSlot::Open(A));
        }

        #[test]
        fn stale_destroyed_keeps_newer_window() {
            let p = PipState::default();
            *p.slot() = PipSlot::Open(B);
            p.destroyed(A);
            assert_eq!(slot(&p), PipSlot::Open(B));
            p.failed(A);
            assert_eq!(slot(&p), PipSlot::Open(B));
            p.failed(B);
            assert_eq!(slot(&p), PipSlot::None);
        }

        #[test]
        fn pip_closed_carries_seq() {
            let v = serde_json::to_value(A).unwrap();
            assert_eq!(v, serde_json::json!({ "generation": 7, "seq": 1 }));
        }
    }
}
