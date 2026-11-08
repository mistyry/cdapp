//! 启动页一言。来源为 `/api/file/config` 的 `saying` 字段，按 etag 缓存到应用数据目录。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::api::Api;
use crate::error::{AppError, AppResult};

const FILE: &str = "saying.json";
const HOST: &str = "https://img.kookapp.cn/";
static PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Saying {
    pub game: String,
    pub role_name: String,
    pub words: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    etag: String,
    data: Vec<Saying>,
}

/// 设置缓存所在目录。只在启动时调用一次。
pub fn init(dir: &Path) {
    let _ = PATH.set(dir.join(FILE));
}

fn load() -> Cache {
    PATH.get()
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(cache: &Cache) {
    let Some(path) = PATH.get() else { return };
    let r = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec(cache)?)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = r {
        log::warn!("一言缓存保存失败：{e}");
    }
}

/// 解析一言列表，跳过缺字段或内容为空的条目。
#[must_use]
pub fn parse(v: &Value) -> Vec<Saying> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| serde_json::from_value::<Saying>(x.clone()).ok())
                .filter(|s| !s.words.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 读取一言列表。网络失败时返回本地缓存；无缓存时返回错误。
///
/// # Errors
///
/// 网络失败且没有缓存时返回错误。
pub async fn list(api: &Api) -> AppResult<Vec<Saying>> {
    let cache = load();
    match fetch(api, &cache).await {
        Ok(Some(fresh)) => {
            save(&fresh);
            Ok(fresh.data)
        }
        Ok(None) => Ok(cache.data),
        Err(e) if !cache.data.is_empty() => {
            log::info!("一言更新失败，使用缓存：{e}");
            Ok(cache.data)
        }
        Err(e) => Err(e),
    }
}

/// etag 未变时返回 None。
async fn fetch(api: &Api, cache: &Cache) -> AppResult<Option<Cache>> {
    let cfg = api.get("/api/file/config", &()).await?;
    let s = cfg
        .get("saying")
        .ok_or_else(|| AppError::Format("缺少 saying".into()))?;
    let url = s.get("url").and_then(Value::as_str).unwrap_or_default();
    let etag = s.get("etag").and_then(Value::as_str).unwrap_or_default();
    if !url.starts_with(HOST) {
        return Err(AppError::Format("一言地址不在允许范围内".into()));
    }
    if !etag.is_empty() && etag == cache.etag && !cache.data.is_empty() {
        return Ok(None);
    }
    let data = parse(&api.get_external(url).await?);
    if data.is_empty() {
        return Err(AppError::Format("一言列表为空".into()));
    }
    Ok(Some(Cache {
        etag: etag.to_owned(),
        data,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_bad_items() {
        let v = serde_json::json!([
            {"game": "G", "role_name": "R", "words": "W"},
            {"game": "G", "role_name": "R", "words": "  "},
            {"game": "G"},
            1
        ]);
        let list = parse(&v);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].words, "W");
        assert_eq!(parse(&serde_json::json!({})), Vec::<Saying>::new());
    }
}
