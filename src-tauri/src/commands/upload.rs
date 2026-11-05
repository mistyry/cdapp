use std::sync::Mutex;

use tauri::{AppHandle, Emitter, State};

use super::{AppState, lock, with_api};
use crate::error::{AppError, AppResult};
use crate::kook::api::check_upload;
use crate::kook::model::Asset;

/// 上传资源。原始字节作为请求体传入，不经 JSON 数组编码。上传 ID 与经 URI 编码的文件名放在请求头中。
/// 进度以 `upload://progress` 事件发出 `{id, sent, total}`；`upload_cancel` 中止进行中的上传。
/// 取消后服务端可能已收到完整内容，但上传不产生消息，重新上传同一内容得到相同地址。
#[tauri::command]
pub async fn upload(
    request: tauri::ipc::Request<'_>,
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> AppResult<Asset> {
    let header = |k: &str| {
        request
            .headers()
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let id = header("x-upload-id").ok_or_else(|| AppError::Other("缺少上传 ID".into()))?;
    let name = header("x-file-name")
        .and_then(|n| {
            url::form_urlencoded::parse(format!("n={n}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "file".into());
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::Other("上传内容格式错误".into()));
    };
    let bytes = bytes.clone();
    check_upload(bytes.len() as u64)?;
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    lock(&state)?.uploads.insert(id.clone(), cancel.clone());
    let pid = id.clone();
    let progress = move |sent: u64, total: u64| {
        let _ = app.emit(
            "upload://progress",
            serde_json::json!({ "id": pid, "sent": sent, "total": total }),
        );
    };
    let r = tokio::select! {
        r = with_api(&state, move |a| async move { a.upload(name, bytes, progress).await }) => r,
        () = cancel.notified() => Err(AppError::Cancelled),
    };
    if let Ok(mut s) = state.lock() {
        s.uploads.remove(&id);
    }
    r
}

/// 取消上传。请求仍在途中、上传尚未登记时不生效，前端以本地状态丢弃迟到结果。
#[tauri::command]
pub async fn upload_cancel(id: String, state: State<'_, Mutex<AppState>>) -> AppResult<()> {
    if let Some(n) = lock(&state)?.uploads.remove(&id) {
        n.notify_one();
    }
    Ok(())
}
