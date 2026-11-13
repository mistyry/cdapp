use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::watch;

use super::{AppState, VoiceState, api, expire, lock, stop_voice};
use crate::error::AppResult;
use crate::kook::gateway::{self, GatewayEvent, ReadyKind};

fn set_session(app: &AppHandle, token: &str, generation: u64, id: Option<String>) -> bool {
    use tauri::Manager;
    let state = app.state::<Mutex<AppState>>();
    let Ok(mut s) = state.lock() else {
        return false;
    };
    if token.is_empty() || s.token.as_deref() != Some(token) || s.gateway_gen != generation {
        return false;
    }
    s.session_id = id;
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GatewayStatus {
    Ready { kind: ReadyKind },
    Disconnected { reason: String, retry_in_ms: u64 },
}

#[tauri::command]
pub async fn gateway_start(app: AppHandle, state: State<'_, Mutex<AppState>>) -> AppResult<()> {
    let api = api(&state)?;
    let token = api.token().unwrap_or_default().to_owned();
    let (stop_tx, stop_rx) = watch::channel(false);
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    let generation = {
        let mut s = lock(&state)?;
        if let Some(old) = s.gateway_stop.replace(stop_tx) {
            let _ = old.send(true);
        }
        s.gateway_wake = Some(wake.clone());
        s.session_id = None;
        s.gateway_gen += 1;
        s.gateway_gen
    };
    let fetch = move || {
        let api = api.clone();
        async move { api.gateway_url().await }
    };
    let current = {
        let app = app.clone();
        let token = token.clone();
        move || {
            use tauri::Manager;
            app.state::<Mutex<AppState>>().lock().is_ok_and(|s| {
                s.token.as_deref() == Some(token.as_str()) && s.gateway_gen == generation
            })
        }
    };
    let cfg = gateway::Config::default();
    tauri::async_runtime::spawn(gateway::run(fetch, cfg, stop_rx, wake, move |ev| {
        use tauri::Manager;
        let r = match ev {
            GatewayEvent::Ready { session_id, kind } => {
                if !set_session(&app, &token, generation, Some(session_id)) {
                    return;
                }
                if kind != ReadyKind::New {
                    wake_voice(&app);
                }
                app.emit("gateway://status", GatewayStatus::Ready { kind })
            }
            GatewayEvent::Event(v) => {
                if !current() {
                    return;
                }
                app.emit("gateway://event", v)
            }
            GatewayEvent::Disconnected { reason, retry_in } => {
                if !set_session(&app, &token, generation, None) {
                    return;
                }
                let retry_in_ms = u64::try_from(retry_in.as_millis()).unwrap_or(u64::MAX);
                app.emit(
                    "gateway://status",
                    GatewayStatus::Disconnected {
                        reason,
                        retry_in_ms,
                    },
                )
            }
            GatewayEvent::Expired(message) => {
                if !expire(&app.state::<Mutex<AppState>>(), &token) {
                    return;
                }
                stop_voice(&app);
                app.emit("session://expired", message)
            }
        };
        if let Err(e) = r {
            log::warn!("事件广播失败：{e}");
        }
    }));
    Ok(())
}

#[tauri::command]
pub async fn gateway_reconnect(state: State<'_, Mutex<AppState>>) -> AppResult<()> {
    if let Some(w) = lock(&state)?.gateway_wake.as_ref() {
        w.notify_one();
    }
    Ok(())
}

#[tauri::command]
pub async fn gateway_stop(state: State<'_, Mutex<AppState>>) -> AppResult<()> {
    let mut s = lock(&state)?;
    if let Some(stop) = s.gateway_stop.take() {
        let _ = stop.send(true);
    }
    s.gateway_wake = None;
    s.session_id = None;
    Ok(())
}

fn wake_voice(app: &AppHandle) {
    use tauri::Manager;
    if let Ok(slot) = app.state::<VoiceState>().0.try_lock()
        && let Some(s) = slot.session.as_ref()
    {
        s.control().wake();
    }
}

#[tauri::command]
pub async fn client_log(level: String, message: String) -> AppResult<()> {
    let level = match level.as_str() {
        "warn" => log::Level::Warn,
        "debug" => log::Level::Debug,
        _ => log::Level::Info,
    };
    let message: String = message.chars().take(500).collect();
    log::log!(target: "app", level, "{message}");
    Ok(())
}
