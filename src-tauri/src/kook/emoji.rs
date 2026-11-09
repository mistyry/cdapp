//! 表情列表。服务器表情来自 `/api/v3/vip-expression/get-all-emojis`，按服务器分组；
//! 个人表情来自 `/api/v3/user-emoji/all` 中 `from = 1` 的分组，互动表情不在此列。

use serde::Serialize;
use serde_json::Value;

use super::model::id_str;

/// 单个表情。`id` 为消息中 `(emj)名称(emj)[ID]` 的 ID，服务器表情可直接用于回应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmojiItem {
    pub id: String,
    pub name: String,
    pub animated: bool,
}

/// 一个服务器的表情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildEmojis {
    pub guild_id: String,
    pub name: String,
    pub icon: String,
    pub items: Vec<EmojiItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmojiSet {
    pub guilds: Vec<GuildEmojis>,
    pub personal: Vec<EmojiItem>,
    /// 个人表情读取失败。服务器表情仍可用。
    pub personal_failed: bool,
    /// 用户已添加的服务器表情组（服务器 ID，按用户排序）。读取失败时为 None。
    pub joined: Option<Vec<String>>,
}

/// 解析 `/api/v3/vip-expression/join-list`：`[{expression_id, target_id, type}]`，保留顺序并去重。
///
/// # Errors
///
/// 响应不是数组时返回错误。
pub fn parse_joined(data: &Value) -> Result<Vec<String>, String> {
    let list = data.as_array().ok_or("已添加表情组不是数组")?;
    let mut seen = std::collections::HashSet::new();
    Ok(list
        .iter()
        .filter_map(|x| id_str(x.get("target_id")))
        .filter(|id| seen.insert(id.clone()))
        .collect())
}

/// 表情 ID 只允许字母、数字、下划线、连字符与斜杠分段，与 `KMarkdown` 解析一致。
#[must_use]
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('/').collect();
    (2..=3).contains(&parts.len())
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 64
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 解析服务器表情分组。缺少服务器 ID 的分组、格式不符或重复的表情跳过，不使整个列表失败。
///
/// # Errors
///
/// 响应不是数组时返回错误。
pub fn parse_guilds(data: &Value) -> Result<Vec<GuildEmojis>, String> {
    let list = data.as_array().ok_or("服务器表情列表不是数组")?;
    let mut out = Vec::new();
    for g in list {
        let Some(guild_id) = id_str(g.get("target_id")) else {
            continue;
        };
        let mut seen = std::collections::HashSet::new();
        let items = g
            .get("emojis")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let id = text(e, "id");
                        let prefix_ok = id.split('/').next() == Some(guild_id.as_str());
                        (valid_id(&id) && prefix_ok && seen.insert(id.clone())).then(|| EmojiItem {
                            name: text(e, "name"),
                            animated: e.get("emoji_type").and_then(Value::as_i64) == Some(2),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let icon = text(g, "icon");
        out.push(GuildEmojis {
            name: text(g, "name"),
            icon: if icon.starts_with("https://") {
                icon
            } else {
                String::new()
            },
            guild_id,
            items,
        });
    }
    Ok(out)
}

/// 解析个人表情。只取 `from = 1` 的分组，ID 为 `code`。
///
/// # Errors
///
/// 响应缺少 items 时返回错误。
pub fn parse_personal(data: &Value) -> Result<Vec<EmojiItem>, String> {
    let groups = data
        .get("items")
        .and_then(Value::as_array)
        .ok_or("个人表情缺少 items")?;
    let mut seen = std::collections::HashSet::new();
    Ok(groups
        .iter()
        .filter(|g| g.get("from").and_then(Value::as_i64) == Some(1))
        .flat_map(|g| {
            g.get("all_emojis")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|e| e.get("status").and_then(Value::as_i64).unwrap_or(0) == 0)
        .filter_map(|e| {
            let id = text(e, "code");
            (valid_id(&id) && seen.insert(id.clone())).then(|| EmojiItem {
                name: text(e, "name"),
                animated: e.get("type").and_then(Value::as_i64) == Some(2),
                id,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn guild_groups_filter_bad_and_foreign_ids() {
        let data = json!([
            {"target_id": "100", "name": "甲", "icon": "https://img.example/a.png", "type": 1, "emojis": [
                {"id": "100/aB_1", "name": "笑", "emoji_type": 1},
                {"id": "100/dyn", "name": "动", "emoji_type": 2},
                {"id": "100/aB_1", "name": "重复", "emoji_type": 1},
                {"id": "200/x", "name": "他服", "emoji_type": 1},
                {"id": "100/<b>", "name": "坏", "emoji_type": 1}
            ]},
            {"name": "无 ID", "emojis": []},
            {"target_id": "300", "name": "乙", "icon": "rel/icon.png", "emojis": null}
        ]);
        let g = parse_guilds(&data).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].items.len(), 2);
        assert!(g[0].items[1].animated);
        assert_eq!(g[1].icon, "");
        assert_eq!(g[1].items.len(), 0);
        assert!(parse_guilds(&json!({})).is_err());
    }

    #[test]
    fn joined_keeps_order_and_dedups() {
        let data = json!([{"target_id": "3"}, {"target_id": "1"}, {"target_id": "3"}, {"expression_id": "x"}]);
        assert_eq!(parse_joined(&data).unwrap(), ["3", "1"]);
        assert!(parse_joined(&json!({})).is_err());
    }

    #[test]
    fn personal_takes_only_own_group() {
        let data = json!({"items": [
            {"from": 1, "all_emojis": [
                {"code": "1/22/abc", "name": "我", "type": 1, "status": 0},
                {"code": "1/22/del", "name": "停用", "type": 1, "status": 1},
                {"code": "bad", "name": "坏", "type": 1, "status": 0}
            ]},
            {"from": 2, "all_emojis": [{"code": "0/1/dice", "name": "骰子", "type": 3, "status": 0}]}
        ]});
        let p = parse_personal(&data).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "1/22/abc");
        assert!(parse_personal(&json!([])).is_err());
    }
}
