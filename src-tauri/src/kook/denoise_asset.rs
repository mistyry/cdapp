//! AI 降噪资源：模块文件缓存到数据根 `cache/denoise/`，许可证书只保存在内存中。

use sha2::{Digest, Sha256};

use super::api::Api;
use crate::error::{AppError, AppResult};

const URL: &str = "https://static.kookapp.cn/app/assets/external/ai_denoiser_module_simd.wasm";
const FILE: &str = "denoise/ai_denoiser_module_simd.wasm";
/// 模块文件的 SHA-256。地址上的文件更换后不再加载，避免运行未知版本。
const SHA256: &str = "8f5ef43d31dce7f4be128adcd6ee5d5d4e96d33ec66f456a4dfb3fea5476416f";

fn sha_ok(bytes: &[u8]) -> bool {
    let d = Sha256::digest(bytes);
    let hex = d.iter().fold(String::new(), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    hex == SHA256
}

/// 读取缓存的模块，缺失或校验失败时下载。成功后在后台编译。
///
/// # Errors
///
/// 数据目录不可用、下载失败或校验不符时返回错误。
pub async fn sync(api: &Api) -> AppResult<()> {
    let path =
        crate::storage::cache_file(FILE).ok_or_else(|| AppError::Other("数据目录不可用".into()))?;
    let cached = tokio::fs::read(&path).await.ok().filter(|b| sha_ok(b));
    let bytes = if let Some(b) = cached {
        b
    } else {
        let b = api.get_bytes(URL).await?.to_vec();
        if !sha_ok(&b) {
            return Err(AppError::Other("降噪模块校验失败".into()));
        }
        let (p, data) = (path.clone(), b.clone());
        tauri::async_runtime::spawn_blocking(move || crate::storage::write(&p, &data))
            .await
            .map_err(|e| AppError::Other(e.to_string()))??;
        b
    };
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = crate::audio::denoise::prepare(&bytes) {
            log::warn!("降噪模块编译失败：{e:#}");
        }
    });
    Ok(())
}

/// 获取许可证书并保存在内存中。已有证书时不再请求。
///
/// # Errors
///
/// 请求失败或响应中没有证书时返回错误。
pub async fn ensure_license(api: &Api) -> AppResult<()> {
    if crate::audio::denoise::has_license() {
        return Ok(());
    }
    let v = api.get("/api/v3/gateway/web-agora-licenses", &()).await?;
    let cert = v
        .get("cert")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Other("没有降噪许可".into()))?;
    crate::audio::denoise::set_license(Some(cert.to_owned()));
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_wrong_hash() {
        assert!(!super::sha_ok(b"x"));
    }
}
