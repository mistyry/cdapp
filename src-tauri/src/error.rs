use serde::{Serialize, Serializer};

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
    #[error("请求过于频繁，请稍后再试")]
    RateLimited,
    #[error("响应格式错误：{0}")]
    Format(String),
    #[error("未登录")]
    NotLoggedIn,
    #[error("登录已失效：{0}")]
    Unauthorized(String),
    #[error("已取消")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotSent,
    Rejected,
    Uncertain,
    Local,
    Unauthorized,
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
