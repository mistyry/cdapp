use std::sync::Mutex;

use tauri::State;

use super::{AppState, with_api};
use crate::error::AppResult;

#[tauri::command]
pub async fn guild_mute(
    guild_id: String,
    user_id: String,
    kind: String,
    on: bool,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    let k = match kind.as_str() {
        "mute" => 1,
        "deafen" => 2,
        _ => return Err(crate::error::AppError::Other("未知的类型".into())),
    };
    with_api(&state, move |a| async move {
        a.guild_mute(&guild_id, &user_id, k, on).await
    })
    .await
}

#[derive(serde::Serialize)]
pub struct GuildMutes {
    pub mute: Vec<String>,
    pub deafen: Vec<String>,
}

#[tauri::command]
pub async fn guild_mutes(
    guild_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<GuildMutes> {
    with_api(&state, move |a| async move {
        let (mute, deafen) = a.guild_mutes(&guild_id).await?;
        Ok(GuildMutes { mute, deafen })
    })
    .await
}

#[tauri::command]
pub async fn voice_kick(
    channel_id: String,
    user_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.voice_kick(&channel_id, &user_id).await
    })
    .await
}

#[tauri::command]
pub async fn voice_move(
    channel_id: String,
    user_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.voice_move(&channel_id, &user_id).await
    })
    .await
}

#[tauri::command]
pub async fn guild_kick(
    guild_id: String,
    user_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.guild_kick(&guild_id, &user_id).await
    })
    .await
}

#[tauri::command]
pub async fn guild_ban(
    guild_id: String,
    user_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    with_api(&state, move |a| async move {
        a.guild_ban(&guild_id, &user_id).await
    })
    .await
}
