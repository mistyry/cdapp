use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use super::{AppState, VoiceState, lock};
use crate::error::{AppError, AppResult};
use crate::kook::api::Api;
use crate::kook::auth::{self, QrStatus};

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Restore {
    None,
    Ready { me: Value },
    Expired { message: String },
}

#[tauri::command]
pub async fn restore_session(state: State<'_, Mutex<AppState>>) -> AppResult<Restore> {
    let Some(token) = auth::load_token()? else {
        return Ok(Restore::None);
    };
    let r = Api::new(Some(token.clone()))?.me().await;
    let mut s = lock(&state)?;
    let superseded = s.token.as_deref().is_some_and(|t| t != token)
        || auth::load_token()?.as_deref() != Some(token.as_str());
    match r {
        _ if superseded => Ok(Restore::None),
        Ok(me) => {
            s.token = Some(token);
            s.gate.me = crate::kook::model::id_str(me.get("id"));
            Ok(Restore::Ready { me })
        }
        Err(AppError::Unauthorized(message)) => {
            if s.token.is_none() {
                auth::clear_token()?;
            }
            Ok(Restore::Expired { message })
        }
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn sayings() -> AppResult<Vec<crate::kook::saying::Saying>> {
    crate::kook::saying::list(Api::new(None)?).await
}

#[tauri::command]
pub async fn login_qr_code() -> AppResult<String> {
    auth::qr_code(&Api::new(None)?).await
}

#[tauri::command]
pub async fn login_qr_poll(
    content: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<QrStatus> {
    let (status, token) = auth::qr_poll(&Api::new(None)?, &content).await?;
    if let (QrStatus::Confirmed, Some(token)) = (&status, token) {
        let mut s = lock(&state)?;
        auth::save_token(&token)?;
        s.token = Some(token);
    }
    Ok(status)
}

#[tauri::command]
pub async fn login_qr_cancel(content: String) -> AppResult<()> {
    auth::qr_cancel(&Api::new(None)?, &content).await
}

#[tauri::command]
pub async fn logout(
    app: tauri::AppHandle,
    state: State<'_, Mutex<AppState>>,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let cleared = {
        let mut s = lock(&state)?;
        super::reset(&mut s);
        auth::clear_token()
    };
    super::stop_share(&app).await;
    let old = {
        let mut slot = voice.0.lock().await;
        slot.reset_account();
        slot.session.take()
    };
    if let Some(v) = old {
        v.leave().await;
    }
    cleared
}

#[tauri::command]
pub async fn sound_sync(
    progress: tauri::ipc::Channel<crate::kook::sound::Progress>,
) -> AppResult<crate::kook::sound::Summary> {
    let api = Api::new(None)?;
    crate::kook::sound::sync(&api, |p| {
        let _ = progress.send(p);
    })
    .await
}

#[tauri::command]
pub async fn denoise_sync() -> AppResult<()> {
    crate::kook::denoise_asset::sync(&Api::new(None)?).await
}
