use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};

use super::VoiceState;
use crate::error::AppResult;
use crate::storage::{self, Info};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    #[serde(flatten)]
    info: Info,
    cache_bytes: u64,
}

#[tauri::command]
pub async fn storage_info() -> AppResult<StorageInfo> {
    tauri::async_runtime::spawn_blocking(|| StorageInfo {
        info: storage::info(),
        cache_bytes: storage::cache_size(),
    })
    .await
    .map_err(|e| crate::error::AppError::Other(e.to_string()))
}

#[tauri::command]
pub async fn storage_relocate(
    path: PathBuf,
    app: tauri::AppHandle,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    super::stop_share(&app).await;
    let old = voice.0.lock().await.session.take();
    if let Some(s) = old {
        s.leave().await;
    }
    tauri::async_runtime::spawn_blocking(move || storage::relocate(&path))
        .await
        .map_err(|e| crate::error::AppError::Other(e.to_string()))?
}

#[tauri::command]
pub async fn storage_switch(path: Option<PathBuf>) -> AppResult<()> {
    storage::switch(path.as_deref())
}

#[tauri::command]
pub async fn storage_clear_cache() -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(storage::clear_cache)
        .await
        .map_err(|e| crate::error::AppError::Other(e.to_string()))?
}

#[tauri::command]
pub async fn account_state(user: String) -> AppResult<Value> {
    storage::account_get(&user)
}

#[tauri::command]
pub async fn set_account_state(user: String, key: String, value: Value) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || storage::account_set(&user, &key, value))
        .await
        .map_err(|e| crate::error::AppError::Other(e.to_string()))?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn restart(app: AppHandle) {
    app.restart();
}
