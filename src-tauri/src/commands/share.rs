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
        let s = crate::share::sdk::sync().await;
        if matches!(s, ShareSdk::Ready { .. }) {
            tauri::async_runtime::spawn(crate::share::agora_sdk::sync());
        }
        s
    }
    #[cfg(not(windows))]
    ShareSdk::Unavailable {
        message: "当前系统暂不支持屏幕分享".into(),
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
    use crate::share::bridge::{self, Source, SourceKind};
    use crate::share::session::{Request, Session, ShareEvent, Sink};

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
        match crate::share::sdk::status() {
            Some(crate::share::sdk::Status::Ready { .. }) => Ok(()),
            _ => Err(AppError::Other("屏幕分享组件不可用".into())),
        }
    }

    #[tauri::command]
    pub async fn share_preview_open() -> AppResult<()> {
        ready()?;
        tauri::async_runtime::spawn_blocking(bridge::preview_open)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?
            .map_err(|e| AppError::Other(e.to_string()))
    }

    #[tauri::command]
    pub async fn share_preview_close() -> AppResult<()> {
        ready()?;
        tauri::async_runtime::spawn_blocking(bridge::preview_close)
            .await
            .map_err(|e| AppError::Other(e.to_string()))
    }

    #[tauri::command]
    pub async fn share_sources() -> AppResult<Vec<Source>> {
        ready()?;
        tauri::async_runtime::spawn_blocking(bridge::sources)
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
            .inspect_err(|e| log::warn!("屏幕分享开始失败：{e}"))
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
        let voice_ended = {
            let slot = voice.0.lock().await;
            match slot
                .session
                .as_ref()
                .filter(|s| s.channel_id == params.channel_id && !s.is_finished())
            {
                Some(s) => s.ended(),
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
            .ok_or_else(|| AppError::Other("屏幕分享已结束".into()))
            .inspect_err(|e| log::warn!("屏幕分享改档失败：{e}"))?;
        session
            .update(Tier {
                resolution,
                frame_rate,
                mode: 0,
            })
            .await
            .inspect_err(|e| log::warn!("屏幕分享改档失败：{e}"))
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
    pub async fn watch_start(
        channel_id: String,
        liver: String,
        frames: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
        app: AppHandle,
        state: State<'_, Mutex<AppState>>,
        voice: State<'_, VoiceState>,
        watch: State<'_, WatchState>,
    ) -> AppResult<u64> {
        watch_start_inner(channel_id, liver, frames, app, state, voice, watch)
            .await
            .inspect_err(|e| log::warn!("观看屏幕分享失败：{e}"))
    }

    async fn watch_start_inner(
        channel_id: String,
        liver: String,
        frames: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
        app: AppHandle,
        state: State<'_, Mutex<AppState>>,
        voice: State<'_, VoiceState>,
        watch: State<'_, WatchState>,
    ) -> AppResult<u64> {
        use crate::share::watch::{Request, Session, StateSink};
        ready()?;
        let api = api(&state)?;
        let me = lock(&state)?.gate.me.clone().ok_or(AppError::NotLoggedIn)?;
        if me == liver {
            return Err(AppError::Other("不能观看自己的屏幕分享".into()));
        }
        let voice_ended = {
            let slot = voice.0.lock().await;
            match slot
                .session
                .as_ref()
                .filter(|s| s.channel_id == channel_id && !s.is_finished())
            {
                Some(s) => s.ended(),
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
        log::info!("开始观看屏幕分享");
        slot.session = Some(Session::start(
            api,
            Request {
                channel_id,
                user_id: me,
                liver,
            },
            generation,
            sink,
            frame_sink,
            Box::pin(voice_ended.wait()),
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
        log::info!("屏幕分享画面改发到新窗口");
        Ok(())
    }

    #[tauri::command]
    pub async fn watch_pip_open(generation: u64, app: AppHandle) -> AppResult<()> {
        use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
        if let Some(w) = app.get_webview_window(PIP_LABEL) {
            let _ = w.show();
            let _ = w.set_focus();
            return Ok(());
        }
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
        WebviewWindowBuilder::new(
            &app,
            PIP_LABEL,
            WebviewUrl::App(format!("index.html?pip={generation}").into()),
        )
        .title("屏幕共享")
        .inner_size(PIP_W, PIP_H)
        .min_inner_size(240.0, 135.0)
        .position(x, y)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .focused(false)
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }

    #[tauri::command]
    pub async fn watch_pip_close(app: AppHandle) -> AppResult<()> {
        use tauri::Manager;
        if let Some(w) = app.get_webview_window(PIP_LABEL) {
            w.destroy().map_err(|e| AppError::Other(e.to_string()))?;
        }
        Ok(())
    }

    pub const PIP_LABEL: &str = "pip";
    const PIP_W: f64 = 480.0;
    const PIP_H: f64 = 270.0;
}
