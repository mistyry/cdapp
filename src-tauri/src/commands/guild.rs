use std::sync::Mutex;

use serde_json::Value;
use tauri::State;

use super::{AppState, lock, note, token_of, with_api};
use crate::error::{AppError, AppResult};

#[tauri::command]
pub async fn me(state: State<'_, Mutex<AppState>>) -> AppResult<Value> {
    let token = token_of(&state)?;
    let me = with_api(&state, move |a| async move { a.me().await }).await?;
    let id = crate::kook::model::id_str(me.get("id"));
    note(&state, &token, |g| g.me = id);
    Ok(me)
}

#[tauri::command]
pub async fn guilds(state: State<'_, Mutex<AppState>>) -> AppResult<Value> {
    let token = token_of(&state)?;
    let list = with_api(&state, move |a| async move { a.guilds().await }).await?;
    // 已退出的服务器不再保留权限依据。
    if let Some(items) = list.as_array() {
        let ids = items
            .iter()
            .filter_map(|g| crate::kook::model::id_str(g.get("id")))
            .collect();
        note(&state, &token, |g| g.retain_guilds(&ids));
    }
    Ok(list)
}

#[tauri::command]
pub async fn guild_folders(state: State<'_, Mutex<AppState>>) -> AppResult<Value> {
    with_api(&state, move |a| async move { a.guild_folders().await }).await
}

#[tauri::command]
pub async fn channels(
    guild_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<Vec<crate::kook::model::Channel>> {
    let token = token_of(&state)?;
    let ticket = lock(&state)?.gate.ticket();
    let gid = guild_id.clone();
    let list = with_api(&state, move |a| async move { a.channels(&gid).await }).await?;
    note(&state, &token, |g| g.set_channels(&guild_id, &list, ticket));
    Ok(list)
}

/// 当前账号在服务器中的角色与权限依据。
#[tauri::command]
pub async fn guild_access(
    guild_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<crate::kook::model::GuildAccess> {
    let token = token_of(&state)?;
    let ticket = lock(&state)?.gate.ticket();
    let gid = guild_id.clone();
    let access = with_api(&state, move |a| async move { a.guild_access(&gid).await }).await?;
    let copy = access.clone();
    note(&state, &token, |g| g.set_access(&guild_id, copy, ticket));
    Ok(access)
}

/// 一页服务器成员，可按频道过滤。
#[tauri::command]
pub async fn members(
    guild_id: String,
    channel_id: Option<String>,
    page: u32,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<crate::kook::model::MemberPage> {
    with_api(&state, move |a| async move {
        a.members(&guild_id, channel_id.as_deref(), page).await
    })
    .await
}

/// 订阅当前频道成员区间的在线状态推送；`start`、`end` 为 -1 时取消。须在网关就绪后调用。
#[tauri::command]
pub async fn subscribe_members(
    channel_id: String,
    start: i64,
    end: i64,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<()> {
    let session = lock(&state)?.session_id.clone();
    let session = session.ok_or_else(|| AppError::Other("网关未就绪".into()))?;
    with_api(&state, move |a| async move {
        a.subscribe_members(&channel_id, start, end, &session).await
    })
    .await
    .map(|_| ())
}

/// 激活服务器以接收其频道消息推送。须在网关就绪后调用。
#[tauri::command]
pub async fn activate_guild(
    guild_id: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<Value> {
    let session = state
        .lock()
        .map_err(|_| AppError::Other("状态锁损坏".into()))?
        .session_id
        .clone();
    let session = session.ok_or_else(|| AppError::Other("网关未就绪".into()))?;
    with_api(&state, move |a| async move {
        a.activate_guild(&guild_id, &session).await
    })
    .await
}

/// 当前账号可见的服务器表情与个人表情。
#[tauri::command]
pub async fn emojis(state: State<'_, Mutex<AppState>>) -> AppResult<crate::kook::emoji::EmojiSet> {
    with_api(&state, move |a| async move { a.emojis().await }).await
}
