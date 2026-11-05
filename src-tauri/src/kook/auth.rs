use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::api::{Api, str_field};
use crate::error::{AppError, AppResult};

/// 凭据文件位置，在应用启动时设置为应用数据目录下的文件。
static PATH: OnceLock<PathBuf> = OnceLock::new();
const FILE: &str = "credentials.json";

#[derive(Serialize, Deserialize)]
struct Stored {
    token: String,
}

/// 设置凭据所在目录。只在启动时调用一次。
pub fn init(dir: &Path) {
    let _ = PATH.set(dir.join(FILE));
}

fn path() -> AppResult<&'static Path> {
    PATH.get()
        .map(PathBuf::as_path)
        .ok_or_else(|| AppError::Other("凭据存储未初始化".into()))
}

/// 读取保存的 token，不存在时返回 None。文件损坏视为没有凭据。
///
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

/// 写入临时文件后原子替换，Unix 下权限为仅本人可读写。
///
/// # Errors
///
/// 目录创建或文件写入失败时返回错误。
pub fn save_token(token: &str) -> AppResult<()> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(&serde_json::to_vec(&Stored {
        token: token.to_owned(),
    })?)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
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

/// 扫码状态。
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

/// 查询一次扫码状态。确认后返回 token。
///
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

    #[test]
    fn save_load_clear() {
        let dir = std::env::temp_dir().join(format!("cdapp-cred-{}", std::process::id()));
        init(&dir);
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
