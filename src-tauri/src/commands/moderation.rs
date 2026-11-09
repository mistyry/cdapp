//! 服务器与语音频道的成员管理。权限由前端按频道权限预先判断，服务端仍会校验。

use std::sync::Mutex;

use tauri::State;

use super::{AppState, with_api};
use crate::error::AppResult;

/// 服务器闭麦或静音。`kind` 为 `mute` 或 `deafen`。
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

/// 服务器中被闭麦与静音的用户。
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
