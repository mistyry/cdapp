use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use super::{AppState, VoiceState, lock};
use crate::error::{AppError, AppResult};
use crate::kook::api::Api;
use crate::kook::auth::{self, QrStatus};

/// 恢复结果。只有服务端确认凭据失效时才清除凭据文件，网络等错误以 `Err` 返回并保留凭据。
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Restore {
    /// 没有保存的凭据。
    None,
    Ready {
        me: Value,
    },
    Expired {
        message: String,
    },
}

/// 尝试使用凭据文件中的 token 恢复登录。
#[tauri::command]
pub async fn restore_session(state: State<'_, Mutex<AppState>>) -> AppResult<Restore> {
    let Some(token) = auth::load_token()? else {
        return Ok(Restore::None);
    };
    let r = Api::new(Some(token.clone()))?.me().await;
    // 等待期间可能已扫码登录了新凭据：此时不写回内存，也不删除凭据文件。凭据文件读写都在状态锁内进行。
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

/// 启动页一言。不需要登录，不使用凭据。
#[tauri::command]
pub async fn sayings() -> AppResult<Vec<crate::kook::saying::Saying>> {
    crate::kook::saying::list(Api::new(None)?).await
}

#[tauri::command]
pub async fn login_qr_code() -> AppResult<String> {
    auth::qr_code(&Api::new(None)?).await
}

/// 查询一次扫码状态。确认后保存 token。
#[tauri::command]
pub async fn login_qr_poll(
    content: String,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<QrStatus> {
    let (status, token) = auth::qr_poll(&Api::new(None)?, &content).await?;
    if let (QrStatus::Confirmed, Some(token)) = (&status, token) {
        // 与退出、失效的凭据文件删除互斥。
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

/// 退出登录：在状态锁内停止网关、清空内存并删除保存的凭据，再退出语音。
/// 删除在任何 await 之前完成，退出期间并发完成的新登录不会被删掉。删除失败时内存状态仍已清空。
#[tauri::command]
pub async fn logout(
    state: State<'_, Mutex<AppState>>,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let cleared = {
        let mut s = lock(&state)?;
        if let Some(stop) = s.gateway_stop.take() {
            let _ = stop.send(true);
        }
        *s = AppState {
            gateway_gen: s.gateway_gen,
            ..AppState::default()
        };
        auth::clear_token()
    };
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

/// 检查音效索引并下载缺少的音效，进度经 `progress` 推送。不需要登录。
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
