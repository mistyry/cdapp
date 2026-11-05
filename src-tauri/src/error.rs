use serde::{Serialize, Serializer};

/// 命令层统一错误。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("网络请求失败：{0}")]
    Http(#[from] reqwest::Error),
    #[error("连接失败：{0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("数据解析失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("凭据存取失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("接口返回错误 {code}：{message}")]
    Api { code: i64, message: String },
    /// 服务端限流（HTTP 429），请求未被处理。
    #[error("请求过于频繁，请稍后再试")]
    RateLimited,
    /// 响应结构与预期不符。
    #[error("响应格式错误：{0}")]
    Format(String),
    #[error("未登录")]
    NotLoggedIn,
    /// 服务端确认凭据无效（HTTP 401 或接口 code 401）。
    #[error("登录已失效：{0}")]
    Unauthorized(String),
    /// 用户取消。
    #[error("已取消")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

/// 写请求失败后的结果确定性。KOOK 写接口不按 nonce 去重，结果不确定时不得自动重发。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 请求未到达服务端，例如连接失败或未登录，可安全重试。
    NotSent,
    /// 服务端明确拒绝，重试前需要处理原因。
    Rejected,
    /// 请求可能已被服务端处理，例如超时或响应中断。
    Uncertain,
    /// 本地错误。
    Local,
    /// 当前凭据已失效，请求未被处理。前端据此结束登录会话。
    Unauthorized,
    /// 被限流，请求未被处理，稍后可重试。
    RateLimited,
}

impl AppError {
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Http(e) => http_kind(
                e.is_connect(),
                e.is_builder() || e.is_request() && !e.is_timeout(),
                e.status().map(|s| s.as_u16()),
            ),
            Self::Api { .. } => ErrorKind::Rejected,
            Self::NotLoggedIn => ErrorKind::NotSent,
            Self::Unauthorized(_) => ErrorKind::Unauthorized,
            Self::RateLimited => ErrorKind::RateLimited,
            Self::Json(_) | Self::Format(_) => ErrorKind::Uncertain,
            Self::WebSocket(_) | Self::Io(_) | Self::Other(_) | Self::Cancelled => ErrorKind::Local,
        }
    }
}

/// 连接失败或请求未构造成功视为未发出；4xx 为服务端拒绝；其余（超时、5xx、读取中断）视为不确定。
fn http_kind(connect: bool, unsent: bool, status: Option<u16>) -> ErrorKind {
    match status {
        Some(s) if (400..500).contains(&s) => ErrorKind::Rejected,
        None if connect || unsent => ErrorKind::NotSent,
        _ => ErrorKind::Uncertain,
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", &self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_http_failures() {
        assert_eq!(http_kind(true, false, None), ErrorKind::NotSent);
        assert_eq!(http_kind(false, true, None), ErrorKind::NotSent);
        assert_eq!(http_kind(false, false, None), ErrorKind::Uncertain);
        assert_eq!(http_kind(false, false, Some(403)), ErrorKind::Rejected);
        assert_eq!(http_kind(false, false, Some(502)), ErrorKind::Uncertain);
    }

    #[test]
    fn serializes_kind_and_message() {
        let v = serde_json::to_value(AppError::Api {
            code: 40000,
            message: "x".into(),
        })
        .unwrap();
        assert_eq!(v["kind"], "rejected");
        assert_eq!(v["message"], "接口返回错误 40000：x");
        assert_eq!(
            serde_json::to_value(AppError::NotLoggedIn).unwrap()["kind"],
            "not_sent"
        );
        assert_eq!(
            serde_json::to_value(AppError::Unauthorized("x".into())).unwrap()["kind"],
            "unauthorized"
        );
        assert_eq!(
            serde_json::to_value(AppError::RateLimited).unwrap()["kind"],
            "rate_limited"
        );
    }
}
