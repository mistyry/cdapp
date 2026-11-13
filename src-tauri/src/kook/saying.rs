use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::api::Api;
use crate::error::{AppError, AppResult};

const FILE: &str = "saying.json";
const HOST: &str = "https://img.kookapp.cn/";

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

fn load() -> Cache {
    crate::storage::cache_file(FILE)
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(cache: &Cache) {
    let Some(path) = crate::storage::cache_file(FILE) else {
        return;
    };
    let r = serde_json::to_vec(cache)
        .map_err(std::io::Error::from)
        .and_then(|b| crate::storage::write(&path, &b));
    if let Err(e) = r {
        log::warn!("一言缓存保存失败：{e}");
    }
}

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

/// # Errors
///
/// 没有缓存且下载失败时返回错误。
pub async fn list(api: Api) -> AppResult<Vec<Saying>> {
    let cache = load();
    if !cache.data.is_empty() {
        let data = cache.data.clone();
        tauri::async_runtime::spawn(async move {
            match fetch(&api, &cache).await {
                Ok(Some(fresh)) => save(&fresh),
                Ok(None) => {}
                Err(e) => log::info!("一言更新失败：{e}"),
            }
        });
        return Ok(data);
    }
    let fresh = fetch(&api, &cache)
        .await?
        .ok_or_else(|| AppError::Format("一言列表为空".into()))?;
    save(&fresh);
    Ok(fresh.data)
}

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
