use std::sync::Mutex;

use serde_json::Value;
use tauri::State;

use super::{AppState, with_api};
use crate::error::AppResult;
use crate::kook::model::{History, MessageLocation};

#[tauri::command]
pub async fn messages(
    channel_id: String,
    before: Option<String>,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<History> {
    with_api(&state, move |a| async move {
        a.messages(&channel_id, before.as_deref()).await
    })
    .await
}

#[tauri::command]
pub async fn messages_after(
    channel_id: String,
    after: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<History> {
    with_api(&state, move |a| async move {
        a.messages_after(&channel_id, &after).await
    })
    .await
}

#[tauri::command]
pub async fn locate_message(
    msg_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<Option<MessageLocation>> {
    with_api(
        &state,
        move |a| async move { a.locate_message(&msg_id).await },
    )
    .await
}

#[tauri::command]
pub async fn send_message(
    channel_id: String,
    kind: u8,
    content: String,
    quote: Option<String>,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<Value> {
    with_api(&state, move |a| async move {
        a.send_message(&channel_id, kind, &content, quote.as_deref())
            .await
    })
    .await
}

#[tauri::command]
pub async fn update_message(
    msg_id: String,
    content: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.update_message(&msg_id, &content).await
    })
    .await
}

#[tauri::command]
pub async fn delete_message(msg_id: String, state: State<'_, Mutex<AppState>>) -> AppResult<()> {
    with_api(
        &state,
        move |a| async move { a.delete_message(&msg_id).await },
    )
    .await
}

#[tauri::command]
pub async fn reaction(
    msg_id: String,
    emoji: String,
    add: bool,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.reaction(&msg_id, &emoji, add).await
    })
    .await
}

#[tauri::command]
pub async fn mark_read(
    channel_id: String,
    msg_id: String,
    time: i64,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<Value> {
    with_api(&state, move |a| async move {
        a.mark_read(&channel_id, &msg_id, time).await
    })
    .await
}

#[tauri::command]
pub async fn unread(
    state: State<'_, Mutex<AppState>>,
) -> AppResult<Vec<crate::kook::model::ChannelUnread>> {
    with_api(&state, move |a| async move { a.unread().await }).await
}
