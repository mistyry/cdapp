//! 音效缓存。系统音效地址固定，装扮提示音来自物品索引，均下载到数据根 `cache/sounds/`。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::api::Api;
use crate::error::AppResult;

const SYSTEM_BASE: &str = "https://static.kookapp.cn/app/assets/audio/";
const ITEM_HOST: &str = "https://img.kookapp.cn/";
const INDEX: &str = "sounds/index.json";
const CONCURRENCY: usize = 8;

/// 客户端用到的系统音效。
pub const SYSTEM: &[&str] = &[
    "user-join.mp3",
    "user-leave.mp3",
    "exit-channel.mp3",
    "mkf-mute.mp3",
    "mkf-cancel-mute.mp3",
    "head-mute.mp3",
    "cancel-head-mute.mp3",
    "mention.mp3",
    "mention-self.mp3",
];

#[derive(Default, Serialize, Deserialize)]
struct Index {
    v: u64,
    /// 物品 id 到音频地址。
    items: BTreeMap<u64, String>,
}

/// 同步进度。
#[derive(Clone, Copy, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// 同步结果。
#[derive(Serialize)]
pub struct Summary {
    pub total: usize,
    pub downloaded: usize,
    pub failed: usize,
}

fn load_index() -> Index {
    crate::storage::cache_file(INDEX)
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_index(index: &Index) {
    let Some(path) = crate::storage::cache_file(INDEX) else {
        return;
    };
    let r = serde_json::to_vec(index)
        .map_err(std::io::Error::from)
        .and_then(|b| crate::storage::write(&path, &b));
    if let Err(e) = r {
        log::warn!("音效索引保存失败：{e}");
    }
}

/// 从物品列表中取出带音频的语音提示音。
#[must_use]
pub fn parse_items(v: &Value) -> BTreeMap<u64, String> {
    let mut out = BTreeMap::new();
    for item in v.as_array().into_iter().flatten() {
        let voice = item
            .get("categories")
            .and_then(Value::as_array)
            .is_some_and(|c| c.iter().any(|x| x == "notice_voice"));
        let id = item.get("id").and_then(Value::as_u64);
        let src = item.pointer("/resources/src").and_then(Value::as_str);
        if let (true, Some(id), Some(src)) = (voice, id, src)
            && src.starts_with(ITEM_HOST)
            && local_name(src).is_some()
        {
            out.insert(id, src.to_owned());
        }
    }
    out
}

/// 由地址得到缓存文件名。只保留路径部分，非常规字符替换为下划线。
#[must_use]
pub fn local_name(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix(ITEM_HOST)
        .or_else(|| url.strip_prefix(SYSTEM_BASE))?;
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    if path.is_empty() || path.ends_with('/') {
        return None;
    }
    let name: String = path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    (!name.starts_with('.')).then_some(name)
}

/// 系统音效的缓存路径。
#[must_use]
pub fn system_file(name: &str) -> Option<PathBuf> {
    crate::storage::cache_file(&format!("sounds/system/{name}"))
}

/// 物品音效的缓存路径。
#[must_use]
pub fn item_file(url: &str) -> Option<PathBuf> {
    crate::storage::cache_file(&format!("sounds/item/{}", local_name(url)?))
}

/// 更新物品索引，网络失败时沿用本地索引。
async fn refresh_index(api: &Api) -> Index {
    match api.get("/api/v3/item/list", &()).await {
        Ok(v) => {
            let items = parse_items(&v);
            if items.is_empty() {
                log::warn!("物品索引中没有语音提示音");
                return load_index();
            }
            let index = Index { v: 1, items };
            save_index(&index);
            index
        }
        Err(e) => {
            log::info!("物品索引更新失败，使用缓存：{e}");
            load_index()
        }
    }
}

async fn download(api: &Api, url: &str, path: &std::path::Path) -> bool {
    match api.get_bytes(url).await {
        Ok(bytes) if !bytes.is_empty() => match crate::storage::write(path, &bytes) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("音效保存失败 {url}：{e}");
                false
            }
        },
        Ok(_) => false,
        Err(e) => {
            log::info!("音效下载失败 {url}：{e}");
            false
        }
    }
}

/// 物品音效的本地文件。索引中没有或文件缺失时按 ID 查询并下载，失败返回 None。
pub async fn item_sound(api: &Api, id: u64) -> Option<PathBuf> {
    let known = load_index().items.get(&id).cloned();
    let url = if let Some(u) = known {
        u
    } else {
        let v = api
            .get("/api/v3/item/list", &[("id", id.to_string())])
            .await
            .ok()?;
        parse_items(&v).remove(&id)?
    };
    let path = item_file(&url)?;
    if path.is_file() || download(api, &url, &path).await {
        Some(path)
    } else {
        None
    }
}

/// 检查音效索引并下载本地缺少的文件。`progress` 在开始与每完成一个文件时调用。
///
/// # Errors
///
/// 数据目录不可用时返回错误。
pub async fn sync(api: &Api, progress: impl Fn(Progress)) -> AppResult<Summary> {
    if crate::storage::cache_file(INDEX).is_none() {
        return Err(crate::error::AppError::Other("数据目录不可用".into()));
    }
    let index = refresh_index(api).await;
    let mut jobs: Vec<(String, PathBuf)> = SYSTEM
        .iter()
        .filter_map(|n| Some((format!("{SYSTEM_BASE}{n}"), system_file(n)?)))
        .collect();
    let mut seen = std::collections::HashSet::new();
    jobs.extend(
        index
            .items
            .values()
            .filter(|u| seen.insert(u.as_str()))
            .filter_map(|u| Some((u.clone(), item_file(u)?))),
    );
    jobs.retain(|(_, p)| !p.is_file());
    let total = jobs.len();
    progress(Progress { done: 0, total });
    let mut done = 0;
    let mut downloaded = 0;
    let mut tasks = futures_util::stream::iter(jobs)
        .map(|(url, path)| async move { download(api, &url, &path).await })
        .buffer_unordered(CONCURRENCY);
    while let Some(ok) = tasks.next().await {
        done += 1;
        downloaded += usize::from(ok);
        progress(Progress { done, total });
    }
    Ok(Summary {
        total,
        downloaded,
        failed: total - downloaded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_voice_items_with_audio() {
        let v = serde_json::json!([
            {"id": 1, "categories": ["decoration", "notice_voice"], "resources": {"src": "https://img.kookapp.cn/assets/item/resources/a.mp3"}},
            {"id": 2, "categories": ["notice_voice"], "resources": {}},
            {"id": 3, "categories": ["avatar"], "resources": {"src": "https://img.kookapp.cn/b.mp3"}},
            {"id": 4, "categories": ["notice_voice"], "resources": {"src": "https://evil.example/c.mp3"}},
            "x"
        ]);
        let items = parse_items(&v);
        assert_eq!(items.len(), 1);
        assert!(items.contains_key(&1));
    }

    #[test]
    fn local_name_sanitizes_path() {
        assert_eq!(
            local_name("https://img.kookapp.cn/assets/item/resources/a b.mp3?x=1").as_deref(),
            Some("assets_item_resources_a_b.mp3")
        );
        assert_eq!(
            local_name("https://static.kookapp.cn/app/assets/audio/mention.mp3").as_deref(),
            Some("mention.mp3")
        );
        assert_eq!(local_name("https://img.kookapp.cn/"), None);
        assert_eq!(local_name("https://other.cn/a.mp3"), None);
        assert_eq!(local_name("https://img.kookapp.cn/.."), None);
    }
}
