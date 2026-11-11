use serde::Serialize;

/// 屏幕分享组件状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ShareSdk {
    Ready { version: String },
    Unavailable { message: String },
}

/// 校验并下载屏幕分享组件，就绪后加载。只执行一次，之后返回首次的结果。不需要登录。
#[tauri::command]
pub async fn share_sdk_sync() -> ShareSdk {
    #[cfg(windows)]
    return crate::share::sdk::sync().await;
    #[cfg(not(windows))]
    ShareSdk::Unavailable {
        message: "当前系统暂不支持屏幕分享".into(),
    }
}
