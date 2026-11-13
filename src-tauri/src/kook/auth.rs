use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::api::{Api, str_field};
use crate::error::{AppError, AppResult};

#[derive(Serialize, Deserialize)]
struct Stored {
    token: String,
}

fn path() -> AppResult<PathBuf> {
    #[cfg(test)]
    if let Some(p) = tests::PATH.get() {
        return Ok(p.clone());
    }
    crate::storage::credentials().ok_or_else(|| AppError::Other("凭据存储未初始化".into()))
}

/// # Errors
///
/// 文件读取失败时返回错误。
pub fn load_token() -> AppResult<Option<String>> {
    match fs::read(path()?) {
        Ok(b) => Ok(serde_json::from_slice::<Stored>(&b)
            .ok()
            .map(|s| s.token)
            .filter(|t| !t.is_empty())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// # Errors
///
/// 目录创建或文件写入失败时返回错误。
pub fn save_token(token: &str) -> AppResult<()> {
    let bytes = serde_json::to_vec(&Stored {
        token: token.to_owned(),
    })?;
    crate::storage::write_raw(&path()?, &bytes, true)?;
    Ok(())
}

/// # Errors
///
/// 文件删除失败时返回错误。
pub fn clear_token() -> AppResult<()> {
    match fs::remove_file(path()?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QrStatus {
    Waiting,
    Scanned,
    Confirmed,
    Unknown,
}

/// # Errors
///
/// 请求失败时返回错误。
pub async fn qr_code(api: &Api) -> AppResult<String> {
    let data = api
        .get("/api/v3/user/login-qr-code", &[("remember", "true")])
        .await?;
    data.as_str()
        .map(str::to_owned)
        .map_or_else(|| str_field(&data, "content"), Ok)
}

/// # Errors
///
/// 请求失败时返回错误。
pub async fn qr_poll(api: &Api, content: &str) -> AppResult<(QrStatus, Option<String>)> {
    let data = api
        .get("/api/v3/user/qr-code-info", &[("content", content)])
        .await?;
    let info = data.get("info").cloned().unwrap_or(Value::Null);
    let status = match info.get("login_status").and_then(Value::as_i64) {
        Some(0) => QrStatus::Waiting,
        Some(1) => QrStatus::Scanned,
        Some(2) => QrStatus::Confirmed,
        _ => QrStatus::Unknown,
    };
    let token = info.get("token").and_then(Value::as_str).map(str::to_owned);
    Ok((status, token))
}

/// # Errors
///
/// 请求失败时返回错误。
pub async fn qr_cancel(api: &Api, content: &str) -> AppResult<()> {
    api.post("/api/v3/user/qr-code-cancel", &json!({ "code": content }))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    const FILE: &str = "credentials.json";
    pub(super) static PATH: OnceLock<PathBuf> = OnceLock::new();

    #[test]
    fn save_load_clear() {
        let dir = std::env::temp_dir().join(format!("cdapp-cred-{}", std::process::id()));
        let _ = PATH.set(dir.join(FILE));
        assert_eq!(load_token().unwrap(), None);
        save_token("t1").unwrap();
        save_token("t2").unwrap();
        assert_eq!(load_token().unwrap().as_deref(), Some("t2"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::write(dir.join(FILE), b"broken").unwrap();
        assert_eq!(load_token().unwrap(), None);
        clear_token().unwrap();
        clear_token().unwrap();
        assert_eq!(load_token().unwrap(), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
