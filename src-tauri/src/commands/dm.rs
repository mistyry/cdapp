use std::sync::Mutex;

use serde_json::Value;
use tauri::State;

use super::{AppState, with_api};
use crate::error::AppResult;
use crate::kook::model::{DmChat, DmPage, DmSent, History};

#[tauri::command]
pub async fn conversations(state: State<'_, Mutex<AppState>>) -> AppResult<Value> {
    with_api(&state, move |a| async move { a.conversations().await }).await
}

#[tauri::command]
pub async fn dm_chats(page: u32, state: State<'_, Mutex<AppState>>) -> AppResult<DmPage> {
    with_api(&state, move |a| async move { a.dm_chats(page).await }).await
}

#[tauri::command]
pub async fn open_dm(user_id: String, state: State<'_, Mutex<AppState>>) -> AppResult<DmChat> {
    with_api(&state, move |a| async move { a.open_dm(&user_id).await }).await
}

#[tauri::command]
pub async fn dm_messages(
    chat_code: String,
    before: Option<String>,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<History> {
    with_api(&state, move |a| async move {
        a.dm_messages(&chat_code, before.as_deref()).await
    })
    .await
}

#[tauri::command]
pub async fn mark_dm_read(
    chat_code: String,
    msg_id: String,
    time: i64,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.mark_dm_read(&chat_code, &msg_id, time).await
    })
    .await
}

#[tauri::command]
pub async fn send_direct(
    user_id: String,
    kind: u8,
    content: String,
    quote: Option<String>,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<DmSent> {
    with_api(&state, move |a| async move {
        a.send_direct(&user_id, kind, &content, quote.as_deref())
            .await
    })
    .await
}

#[tauri::command]
pub async fn update_dm_message(
    msg_id: String,
    content: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.update_dm_message(&msg_id, &content).await
    })
    .await
}

#[tauri::command]
pub async fn delete_dm_message(msg_id: String, state: State<'_, Mutex<AppState>>) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.delete_dm_message(&msg_id).await
    })
    .await
}

#[tauri::command]
pub async fn dm_reaction(
    msg_id: String,
    emoji: String,
    add: bool,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.dm_reaction(&msg_id, &emoji, add).await
    })
    .await
}
