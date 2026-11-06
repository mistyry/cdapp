//! 接口响应到客户端模型的转换边界。只保留界面实际使用的字段，缺少可选字段时取默认值。

use serde::Serialize;
use serde_json::Value;

/// 频道类型。协议值 0 分组，1 文字，2 语音；其余值保留原值并降级显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Category,
    Text,
    Voice,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    /// 协议中的原始类型值，用于诊断未知类型。
    pub raw_type: i64,
    /// 所属分组 ID；顶层频道与分组本身为 `None`。
    pub parent_id: Option<String>,
    /// 同级排序值，越小越靠前。不保证唯一。
    pub level: i64,
    /// 语音人数上限，0 表示不限。
    pub limit: u64,
    /// 角色覆盖（`role_id` 0 为 @全体成员）。`channel/list` 返回已生效的覆盖，同步分组的频道也带完整数组。
    pub overwrites: Vec<Overwrite>,
    /// 用户覆盖。
    pub user_overwrites: Vec<UserOverwrite>,
    /// 频道设有密码。服务端不返回密码本身。
    pub has_password: bool,
    /// 与所属分组同步覆盖。服务端已把分组覆盖复制到本频道，客户端不再叠加父分组。
    pub permission_sync: bool,
    /// 语音频道内的在线用户，来自 guild/view；`channel/list` 不提供，为空。
    pub users: Vec<VoiceUser>,
    /// 最新消息的单行预览，形如 `昵称：内容`；最新消息被删除时为服务端给出的说明。
    pub preview: String,
}

/// 语音频道内用户的显示资料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceUser {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub identify_num: String,
    pub avatar: String,
}

impl VoiceUser {
    /// 缺少 ID 时返回 `None`。
    #[must_use]
    pub fn from_raw(v: &Value) -> Option<Self> {
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Some(Self {
            id: id_of(v.get("id"))?,
            username: s("username"),
            nickname: s("nickname"),
            identify_num: s("identify_num"),
            avatar: s("avatar"),
        })
    }
}

/// 频道角色覆盖。角色 ID 为数字，0 表示 @全体成员（不能用 `id_of`，它把 0 视为缺失）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overwrite {
    pub role_id: i64,
    pub allow: u64,
    pub deny: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserOverwrite {
    pub user_id: String,
    pub allow: u64,
    pub deny: u64,
}

fn bits_of(v: Option<&Value>) -> u64 {
    int_of(v).and_then(|n| u64::try_from(n).ok()).unwrap_or(0)
}

fn overwrites_of(v: &Value) -> (Vec<Overwrite>, Vec<UserOverwrite>) {
    let list = |k: &str| {
        v.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let roles = list("permission_overwrites")
        .iter()
        .filter_map(|o| {
            Some(Overwrite {
                role_id: int_of(o.get("role_id"))?,
                allow: bits_of(o.get("allow")),
                deny: bits_of(o.get("deny")),
            })
        })
        .collect();
    let users = list("permission_users")
        .iter()
        .filter_map(|o| {
            Some(UserOverwrite {
                user_id: id_of(o.get("user_id")).or_else(|| id_of(o.get("user")?.get("id")))?,
                allow: bits_of(o.get("allow")),
                deny: bits_of(o.get("deny")),
            })
        })
        .collect();
    (roles, users)
}

/// 服务器角色。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub id: i64,
    pub name: String,
    /// 0xRRGGBB，0 表示无颜色。
    pub color: u32,
    /// 越小越靠前。
    pub position: i64,
    /// 是否在成员列表中单独分组。
    pub hoist: bool,
    pub permissions: u64,
}

/// 当前账号在服务器中的权限依据，来自 `guild/view`（普通成员无权调用 `guild-role/list`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildAccess {
    pub guild_id: String,
    pub owner_id: String,
    /// 当前账号是否为服务器主。
    pub master: bool,
    pub my_roles: Vec<i64>,
    pub roles: Vec<Role>,
}

impl GuildAccess {
    /// # Errors
    ///
    /// 响应不是对象或缺少 `roles` 时返回描述；无效服务器为 HTTP 204（`Null`）。
    pub fn from_raw(v: &Value) -> Result<Self, String> {
        let Value::Object(_) = v else {
            return Err(format!("服务器信息格式异常：{}", kind_name(v)));
        };
        let Some(Value::Array(roles)) = v.get("roles") else {
            return Err("服务器信息缺少 roles".into());
        };
        let roles = roles
            .iter()
            .filter_map(|r| {
                Some(Role {
                    id: int_of(r.get("role_id"))?,
                    name: r
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    color: int_of(r.get("color"))
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0),
                    position: int_of(r.get("position")).unwrap_or(0),
                    hoist: int_of(r.get("hoist")).unwrap_or(0) != 0,
                    permissions: bits_of(r.get("permissions")),
                })
            })
            .collect();
        Ok(Self {
            guild_id: id_of(v.get("id")).unwrap_or_default(),
            owner_id: id_of(v.get("user_id")).unwrap_or_default(),
            master: v.get("is_master").and_then(Value::as_bool).unwrap_or(false),
            my_roles: v
                .get("my_roles")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| int_of(Some(x))).collect())
                .unwrap_or_default(),
            roles,
        })
    }
}

/// 服务器成员（服务器维度：昵称、角色、颜色）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub id: String,
    pub username: String,
    /// 服务器内昵称，可能为空。
    pub nickname: String,
    pub identify_num: String,
    pub avatar: String,
    pub online: bool,
    pub bot: bool,
    pub roles: Vec<i64>,
    /// 0xRRGGBB，0 表示无颜色。
    pub color: u32,
}

impl Member {
    #[must_use]
    pub fn from_raw(v: &Value) -> Option<Self> {
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Some(Self {
            id: id_of(v.get("id"))?,
            username: s("username"),
            nickname: s("nickname"),
            identify_num: s("identify_num"),
            avatar: s("avatar"),
            online: v.get("online").and_then(Value::as_bool).unwrap_or(false),
            bot: v.get("bot").and_then(Value::as_bool).unwrap_or(false),
            roles: v
                .get("roles")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| int_of(Some(x))).collect())
                .unwrap_or_default(),
            color: int_of(v.get("color"))
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
        })
    }
}

/// 一页成员。服务端在线成员在前、离线在后；人数直接取服务端统计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberPage {
    pub items: Vec<Member>,
    pub page: u32,
    pub page_total: u32,
    pub total: u64,
    pub online: u64,
    pub offline: u64,
}

impl MemberPage {
    /// # Errors
    ///
    /// 响应不是分页对象时返回描述，不当作空列表。
    pub fn from_raw(data: &Value) -> Result<Self, String> {
        let Some(Value::Array(items)) = data.get("items") else {
            return Err(format!("成员列表格式异常：{}", kind_name(data)));
        };
        let meta = data.get("meta");
        let num = |v: Option<&Value>| int_of(v).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        let items: Vec<Member> = items.iter().filter_map(Member::from_raw).collect();
        let total = num(meta.and_then(|m| m.get("total")));
        let online = num(data.get("online_count"));
        Ok(Self {
            page: u32::try_from(num(meta.and_then(|m| m.get("page"))))
                .unwrap_or(1)
                .max(1),
            page_total: u32::try_from(num(meta.and_then(|m| m.get("page_total")))).unwrap_or(1),
            total,
            online,
            offline: data
                .get("offline_count")
                .map_or_else(|| total.saturating_sub(online), |v| num(Some(v))),
            items,
        })
    }
}

/// 公开的 ID 读取，规则同 `id_of`。
#[must_use]
pub fn id_str(v: Option<&Value>) -> Option<String> {
    id_of(v)
}

/// 字符串或数字形式的 ID 统一为字符串；空串、"0" 与 0 视为缺失。
fn id_of(v: Option<&Value>) -> Option<String> {
    let s = match v? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!s.is_empty() && s != "0").then_some(s)
}

fn int_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

impl Channel {
    /// 缺少 ID 的条目无法引用，返回 `None` 由调用方记录。
    #[must_use]
    pub fn from_raw(v: &Value) -> Option<Self> {
        let id = id_of(v.get("id"))?;
        let raw_type = int_of(v.get("type")).unwrap_or(-1);
        let category = v.get("is_category").and_then(Value::as_bool) == Some(true);
        let kind = match raw_type {
            0 => ChannelKind::Category,
            _ if category => ChannelKind::Category,
            1 => ChannelKind::Text,
            2 => ChannelKind::Voice,
            _ => ChannelKind::Unknown,
        };
        let (overwrites, user_overwrites) = overwrites_of(v);
        Some(Self {
            overwrites,
            user_overwrites,
            has_password: int_of(v.get("has_password")).unwrap_or(0) != 0,
            permission_sync: int_of(v.get("permission_sync")).unwrap_or(0) != 0,
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            kind,
            raw_type,
            parent_id: id_of(v.get("parent_id")).filter(|p| *p != id),
            level: int_of(v.get("level")).unwrap_or(0),
            limit: int_of(v.get("limit_amount"))
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
            preview: v
                .get("last_msg_content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            users: v
                .get("users")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(VoiceUser::from_raw).collect())
                .unwrap_or_default(),
            id,
        })
    }
}

/// 一页频道列表。`data` 为 `{items, meta: {page, page_total}}`。
pub struct Page {
    pub items: Vec<Value>,
    pub page_total: u32,
}

impl Page {
    /// # Errors
    ///
    /// 响应不是分页对象时返回描述性错误，避免把格式错误当作空列表覆盖状态。
    pub fn from_raw(data: Value) -> Result<Self, String> {
        let Value::Object(mut map) = data else {
            return Err("频道列表响应不是对象".into());
        };
        let Some(Value::Array(items)) = map.remove("items") else {
            return Err("频道列表响应缺少 items".into());
        };
        let page_total = map
            .get("meta")
            .and_then(|m| int_of(m.get("page_total")))
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(1);
        Ok(Self { items, page_total })
    }
}

/// 展开 guild/view 的嵌套频道：分组后紧跟其子频道，子频道缺少 `parent_id` 时以所在分组补齐。
#[must_use]
pub fn flatten_view_channels(top: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for c in top {
        let mut c = c.clone();
        let children = c
            .as_object_mut()
            .and_then(|o| o.remove("channels"))
            .and_then(|v| match v {
                Value::Array(a) => Some(a),
                _ => None,
            })
            .unwrap_or_default();
        let parent = c.get("id").cloned();
        out.push(c);
        for mut child in children {
            if let (Some(o), Some(p)) = (child.as_object_mut(), parent.as_ref()) {
                let missing = o.get("parent_id").is_none_or(|v| v.as_str() == Some(""));
                if missing {
                    o.insert("parent_id".into(), p.clone());
                }
            }
            out.push(child);
        }
    }
    out
}

/// 合并各页条目：按 ID 去重（翻页期间列表变化可能产生重复），跳过无 ID 条目并返回其数量。
#[must_use]
pub fn merge_channels(pages: impl IntoIterator<Item = Vec<Value>>) -> (Vec<Channel>, usize) {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut skipped = 0;
    for v in pages.into_iter().flatten() {
        match Channel::from_raw(&v) {
            Some(c) if seen.insert(c.id.clone()) => out.push(c),
            Some(_) => {}
            None => skipped += 1,
        }
    }
    (out, skipped)
}

/// 一页历史消息，按时间从旧到新。`end` 表示没有更早的消息。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct History {
    pub items: Vec<Value>,
    pub end: bool,
}

impl History {
    /// v2 历史接口：有数据时返回数组；没有更早消息、空频道或游标无效时返回 HTTP 204（此处为 `Null`）。
    /// 条数少于页大小同样视为结束。缺少 ID 的条目跳过，按时间升序整理。
    ///
    /// # Errors
    ///
    /// 响应既不是数组也不是空响应时返回描述，不当作空历史。
    pub fn from_raw(data: Value, page_size: usize) -> Result<Self, String> {
        let list = match data {
            Value::Null => {
                return Ok(Self {
                    items: Vec::new(),
                    end: true,
                });
            }
            Value::Array(a) => a,
            other => return Err(format!("历史消息格式异常：{}", kind_name(&other))),
        };
        let full = list.len() >= page_size;
        let mut items: Vec<Value> = list
            .into_iter()
            .filter(|m| id_of(m.get("rong_id")).is_some() || id_of(m.get("id")).is_some())
            .collect();
        items.sort_by_key(|m| int_of(m.get("create_at")).unwrap_or(0));
        Ok(Self { items, end: !full })
    }
}

/// 私信会话的对方用户。`system` 为官方或系统会话（`is_sys`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmUser {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub identify_num: String,
    pub avatar: String,
    pub online: bool,
    pub bot: bool,
    pub system: bool,
}

/// 私信会话。`code` 为 32 位十六进制 `chat_code`，双方相同；它既不是用户 ID 也不是频道 ID。
/// 对方用户 ID 在 `target.id`。时间均为毫秒。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmChat {
    pub code: String,
    pub target: DmUser,
    /// 最近消息摘要（`last_msg_content`）。
    pub preview: String,
    pub latest_at: i64,
    pub read_at: i64,
    pub unread: u64,
}

impl DmChat {
    /// 缺少 `code` 或对方 ID 的会话无法打开，返回 `None`。
    #[must_use]
    pub fn from_raw(v: &Value) -> Option<Self> {
        let code = v
            .get("code")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())?;
        let t = v.get("target_info")?;
        let s = |o: &Value, k: &str| {
            o.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let flag = |k: &str| match t.get(k) {
            Some(Value::Bool(b)) => *b,
            other => int_of(other).unwrap_or(0) != 0,
        };
        Some(Self {
            code: code.to_owned(),
            target: DmUser {
                id: id_of(t.get("id"))?,
                username: s(t, "username"),
                nickname: s(t, "nickname"),
                identify_num: s(t, "identify_num"),
                avatar: s(t, "avatar"),
                online: flag("online"),
                bot: flag("bot"),
                system: flag("is_sys"),
            },
            preview: s(v, "last_msg_content"),
            latest_at: int_of(v.get("latest_msg_time")).unwrap_or(0),
            read_at: int_of(v.get("last_read_time")).unwrap_or(0),
            unread: int_of(v.get("unread_count"))
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
        })
    }
}

/// 一页私信会话，按最近消息时间从新到旧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmPage {
    pub items: Vec<DmChat>,
    pub page: u32,
    pub page_total: u32,
    pub total: u64,
}

impl DmPage {
    /// # Errors
    ///
    /// 响应不是分页对象时返回描述，不当作空列表。
    pub fn from_raw(data: &Value) -> Result<Self, String> {
        let Some(Value::Array(items)) = data.get("items") else {
            return Err(format!("私信列表格式异常：{}", kind_name(data)));
        };
        let meta = data.get("meta");
        let num = |k: &str| int_of(meta.and_then(|m| m.get(k))).unwrap_or(0);
        let mut items: Vec<DmChat> = items.iter().filter_map(DmChat::from_raw).collect();
        items.sort_by_key(|c| std::cmp::Reverse(c.latest_at));
        Ok(Self {
            page: u32::try_from(num("page")).unwrap_or(1).max(1),
            page_total: u32::try_from(num("page_total")).unwrap_or(1),
            total: u64::try_from(num("total")).unwrap_or(0),
            items,
        })
    }
}

impl History {
    /// v3 私信历史：`{items}`，没有 meta，条目 ID 为 `id`（无 `rong_id`）。条数少于页大小视为没有更早消息。
    ///
    /// # Errors
    ///
    /// 响应不含 `items` 数组时返回描述。
    pub fn from_dm(data: &Value, page_size: usize) -> Result<Self, String> {
        let Some(Value::Array(list)) = data.get("items") else {
            return Err(format!("私信历史格式异常：{}", kind_name(data)));
        };
        let full = list.len() >= page_size;
        let mut items: Vec<Value> = list
            .iter()
            .filter(|m| id_of(m.get("id")).is_some())
            .cloned()
            .collect();
        items.sort_by_key(|m| int_of(m.get("create_at")).unwrap_or(0));
        Ok(Self { items, end: !full })
    }
}

/// 已上传的资源。`url` 位于 KOOK 资源域名，发送图片或文件时作为消息内容。
/// 资源不绑定频道或用户，同一内容重复上传返回相同地址。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub url: String,
}

impl Asset {
    /// `/api/v3/asset/create` 的 data（`{url, media_id}`）。
    ///
    /// # Errors
    ///
    /// 缺少地址或地址不是 https 时返回描述。
    pub fn from_raw(data: &Value) -> Result<Self, String> {
        let url = data
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("https://"))
            .ok_or("上传结果缺少资源地址")?;
        Ok(Self {
            url: url.to_owned(),
        })
    }
}

/// 私信发送结果。`code` 为会话的 `chat_code`，`id` 为消息 ID（编辑、删除、回应与网关事件的 `rong_id`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmSent {
    pub id: String,
    pub code: String,
    pub create_at: i64,
}

impl DmSent {
    /// `/api/v3/direct-message/create` 的 data（`{code, msg_id, msg_timestamp, nonce}`）。
    ///
    /// # Errors
    ///
    /// 缺少消息 ID 时返回描述（请求已被处理，结果按不确定对待）。
    pub fn from_raw(data: &Value) -> Result<Self, String> {
        let id = id_of(data.get("msg_id")).ok_or("私信发送结果缺少消息 ID")?;
        Ok(Self {
            id,
            code: id_of(data.get("code")).unwrap_or_default(),
            create_at: int_of(data.get("msg_timestamp")).unwrap_or(0),
        })
    }
}

/// 被查找消息的位置。`id` 为 `rong_id`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageLocation {
    pub id: String,
    pub channel_id: String,
    pub create_at: i64,
}

impl MessageLocation {
    /// `/api/v3/message/view` 的 data。
    ///
    /// # Errors
    ///
    /// 缺少消息 ID 或频道 ID 时返回描述。
    pub fn from_raw(data: &Value) -> Result<Self, String> {
        let id = id_of(data.get("id")).ok_or("消息查询结果缺少 ID")?;
        let channel_id = id_of(data.get("channel_id")).ok_or("消息查询结果缺少频道")?;
        Ok(Self {
            id,
            channel_id,
            create_at: int_of(data.get("create_at")).unwrap_or(0),
        })
    }
}

/// 单个频道的未读状态。`unread` 只表示是否有未读（服务端计数最大为 1）；`mentions` 为提及条数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelUnread {
    pub guild_id: String,
    pub channel_id: String,
    pub unread: bool,
    pub mentions: u32,
    /// 最后已读消息时间（毫秒），0 表示未知。
    pub read_at: i64,
}

/// `GET /api/v2/messages/unread`：`{服务器ID: {id, channels: {频道ID: {...}}}}`，覆盖全部已加入服务器。
/// 只返回有未读或提及的频道；缺少 ID 的条目跳过。
///
/// # Errors
///
/// 响应不是对象时返回描述，不当作“没有未读”。
pub fn parse_unread(data: &Value) -> Result<Vec<ChannelUnread>, String> {
    let Value::Object(guilds) = data else {
        return Err(format!("未读响应格式异常：{}", kind_name(data)));
    };
    let mut out = Vec::new();
    for (key, g) in guilds {
        let guild_id = id_of(g.get("id")).unwrap_or_else(|| key.clone());
        let Some(Value::Object(chans)) = g.get("channels") else {
            continue;
        };
        for (ck, c) in chans {
            let channel_id = id_of(c.get("id")).unwrap_or_else(|| ck.clone());
            let unread = int_of(c.get("message_count")).unwrap_or(0) > 0;
            let mentions = int_of(c.get("mention_count"))
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0);
            if !unread && mentions == 0 {
                continue;
            }
            out.push(ChannelUnread {
                guild_id: guild_id.clone(),
                channel_id,
                unread: unread || mentions > 0,
                mentions,
                read_at: int_of(c.get("last_read_message_time")).unwrap_or(0),
            });
        }
    }
    Ok(out)
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_sent_parsed() {
        let s = DmSent::from_raw(
            &json!({"code": "c".repeat(32), "msg_id": "m-1", "msg_timestamp": 5, "nonce": "n"}),
        )
        .unwrap();
        assert_eq!((s.id.as_str(), s.code.len(), s.create_at), ("m-1", 32, 5));
        assert!(DmSent::from_raw(&json!({"code": "c"})).is_err());
        assert!(DmSent::from_raw(&json!([])).is_err());
    }
    use serde_json::json;

    #[test]
    fn unread_snapshot() {
        let v = json!({
            "1": {"id": "1", "channels": {
                "a": {"id": "a", "message_count": 1, "mention_count": 0, "last_read_message_time": 5},
                "b": {"id": "b", "message_count": 0, "mention_count": 2},
                "c": {"id": "c", "message_count": 0, "mention_count": 0},
                "d": {}
            }},
            "2": {"id": "2"}
        });
        let mut u = parse_unread(&v).unwrap();
        u.sort_by(|x, y| x.channel_id.cmp(&y.channel_id));
        assert_eq!(u.len(), 2);
        assert_eq!((u[0].unread, u[0].mentions, u[0].read_at), (true, 0, 5));
        assert_eq!(
            (u[1].unread, u[1].mentions, u[1].guild_id.as_str()),
            (true, 2, "1")
        );
        assert!(parse_unread(&json!([])).is_err());
        assert!(parse_unread(&Value::Null).is_err());
    }

    #[test]
    fn converts_full_and_sparse_items() {
        let c = Channel::from_raw(&json!({
            "id": "1", "name": "n", "type": 2, "parent_id": "9", "level": 3, "limit_amount": 5,
            "is_category": false
        }))
        .unwrap();
        assert_eq!(c.kind, ChannelKind::Voice);
        assert_eq!(c.parent_id.as_deref(), Some("9"));
        assert_eq!((c.level, c.limit), (3, 5));

        // 只有 id：其余取默认值，未知类型降级。
        let c = Channel::from_raw(&json!({"id": 7})).unwrap();
        assert_eq!(c.id, "7");
        assert_eq!(c.kind, ChannelKind::Unknown);
        assert_eq!(
            (c.name.as_str(), c.parent_id, c.level, c.limit),
            ("", None, 0, 0)
        );

        assert_eq!(
            Channel::from_raw(&json!({"id": "3", "type": 1, "parent_id": "0"}))
                .unwrap()
                .parent_id,
            None
        );
        assert_eq!(
            Channel::from_raw(&json!({"id": "3", "type": 9, "is_category": true}))
                .unwrap()
                .kind,
            ChannelKind::Category
        );
        assert!(Channel::from_raw(&json!({"name": "x"})).is_none());
        assert!(Channel::from_raw(&json!({"id": ""})).is_none());
    }

    #[test]
    fn parses_pages_and_rejects_malformed() {
        let p =
            Page::from_raw(json!({"items": [{"id": "1"}], "meta": {"page": 1, "page_total": 3}}))
                .unwrap();
        assert_eq!((p.items.len(), p.page_total), (1, 3));
        assert_eq!(Page::from_raw(json!({"items": []})).unwrap().page_total, 1);
        assert!(Page::from_raw(json!([])).is_err());
        assert!(Page::from_raw(json!({})).is_err());
        assert!(Page::from_raw(Value::Null).is_err());
    }

    #[test]
    fn flattens_view_channels() {
        let top = vec![
            json!({"id": "c1", "type": 0, "channels": [
                {"id": "t1", "type": 1, "parent_id": "c1"},
                {"id": "t2", "type": 2, "users": [{"id": "u1", "username": "a", "nickname": "b"}, {"name": "x"}]},
            ]}),
            json!({"id": "t3", "type": 1, "parent_id": "", "last_msg_content": "甲：你好"}),
        ];
        let (list, _) = merge_channels([flatten_view_channels(&top)]);
        let ids: Vec<_> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["c1", "t1", "t2", "t3"]);
        assert_eq!(list[0].kind, ChannelKind::Category);
        assert_eq!(list[1].parent_id.as_deref(), Some("c1"));
        assert_eq!(list[2].parent_id.as_deref(), Some("c1"));
        assert_eq!(list[3].parent_id, None);
        assert_eq!(list[3].preview, "甲：你好");
        assert_eq!(list[2].users.len(), 1);
        assert_eq!(list[2].users[0].id, "u1");
        assert_eq!(list[2].users[0].nickname, "b");
        assert_eq!(list[1].users.len(), 0);
        assert!(!list[0].has_password);
    }

    #[test]
    fn merges_without_duplicates() {
        let (list, skipped) = merge_channels([
            vec![json!({"id": "1"}), json!({"id": "2"})],
            vec![json!({"id": "2"}), json!({"x": 1}), json!({"id": "3"})],
        ]);
        let ids: Vec<_> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn overwrites_keep_everyone_role() {
        let c = Channel::from_raw(&json!({
            "id": "1", "type": 1,
            "permission_overwrites": [{"role_id": 0, "allow": 0, "deny": 2048}, {"role_id": 5, "allow": 4096, "deny": 0}, {"x": 1}],
            "permission_users": [{"user": {"id": "u1"}, "user_id": "u1", "allow": 2048, "deny": 0}, {"user": {"id": 9}, "allow": 1, "deny": 0}, {}]
        }))
        .unwrap();
        assert_eq!(c.overwrites.len(), 2);
        assert_eq!((c.overwrites[0].role_id, c.overwrites[0].deny), (0, 2048));
        let ids: Vec<_> = c
            .user_overwrites
            .iter()
            .map(|u| u.user_id.as_str())
            .collect();
        assert_eq!(ids, ["u1", "9"]);
    }

    #[test]
    fn guild_access() {
        let a = GuildAccess::from_raw(&json!({
            "id": "g", "user_id": "o", "is_master": false, "my_roles": [5],
            "roles": [{"role_id": 0, "type": 255, "permissions": 6144}, {"role_id": 5, "name": "r", "color": 49050, "position": 1, "hoist": 1, "permissions": 1}]
        }))
        .unwrap();
        assert_eq!(
            (a.owner_id.as_str(), a.master, a.my_roles.as_slice()),
            ("o", false, &[5][..])
        );
        assert_eq!(a.roles[0].id, 0);
        assert!(a.roles[1].hoist && a.roles[1].color == 49050);
        assert!(GuildAccess::from_raw(&Value::Null).is_err());
        assert!(GuildAccess::from_raw(&json!({"id": "g"})).is_err());
    }

    #[test]
    fn member_page() {
        let p = MemberPage::from_raw(&json!({
            "items": [{"id": "1", "nickname": "n", "online": true, "roles": [3], "color": 0}, {"username": "无 ID"}, {"id": 2}],
            "meta": {"page": 2, "page_total": 4, "page_size": 50, "total": 160},
            "online_count": 10, "offline_count": 150
        }))
        .unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(
            (p.page, p.page_total, p.total, p.online, p.offline),
            (2, 4, 160, 10, 150)
        );
        assert!(!p.items[1].online && p.items[1].roles.is_empty());
        let p =
            MemberPage::from_raw(&json!({"items": [], "meta": {"total": 5}, "online_count": 2}))
                .unwrap();
        assert_eq!((p.page, p.offline), (1, 3));
        assert!(MemberPage::from_raw(&json!({})).is_err());
        assert!(MemberPage::from_raw(&Value::Null).is_err());
    }

    #[test]
    fn history_page() {
        let empty = History::from_raw(Value::Null, 50).unwrap();
        assert!(empty.end && empty.items.is_empty());
        let raw = serde_json::json!([
            {"id": "2", "rong_id": "b", "create_at": 20},
            {"id": "1", "rong_id": "a", "create_at": 10},
            {"content": "无 ID"}
        ]);
        let h = History::from_raw(raw.clone(), 3).unwrap();
        assert_eq!(h.items.len(), 2);
        assert_eq!(h.items[0]["rong_id"], "a");
        assert!(!h.end, "满页不视为结束");
        assert!(History::from_raw(raw, 50).unwrap().end);
        assert!(History::from_raw(serde_json::json!({"items": []}), 50).is_err());
    }

    #[test]
    fn message_location() {
        let l = MessageLocation::from_raw(
            &serde_json::json!({"id": "r1", "channel_id": 42, "create_at": 7, "quote": null}),
        )
        .unwrap();
        assert_eq!(
            l,
            MessageLocation {
                id: "r1".into(),
                channel_id: "42".into(),
                create_at: 7
            }
        );
        assert!(MessageLocation::from_raw(&serde_json::json!({"id": "r1"})).is_err());
        assert!(MessageLocation::from_raw(&Value::Null).is_err());
    }

    #[test]
    fn asset() {
        let a = Asset::from_raw(&serde_json::json!({"url": "https://x/a.png", "media_id": "m"}))
            .unwrap();
        assert_eq!(a.url, "https://x/a.png");
        assert!(Asset::from_raw(&serde_json::json!({"url": "http://x/a.png"})).is_err());
        assert!(Asset::from_raw(&serde_json::json!({"media_id": "m"})).is_err());
        assert!(Asset::from_raw(&Value::Null).is_err());
    }

    #[test]
    fn dm_page() {
        let user = |id: Value| json!({"id": id, "username": "u", "online": true, "is_sys": 1});
        let p = DmPage::from_raw(&json!({
            "items": [
                {"code": "a", "target_info": user(json!("1")), "latest_msg_time": 10, "unread_count": 2, "last_msg_content": "hi"},
                {"code": "b", "target_info": user(json!(2)), "latest_msg_time": 30},
                {"code": "", "target_info": user(json!("3"))},
                {"code": "c"}
            ],
            "meta": {"page": 1, "page_total": 2, "page_size": 50, "total": 60}
        }))
        .unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].code, "b", "按最近消息时间降序");
        assert_eq!(p.items[0].target.id, "2");
        let a = &p.items[1];
        assert_eq!(
            (
                a.unread,
                a.preview.as_str(),
                a.target.system,
                a.target.online
            ),
            (2, "hi", true, true)
        );
        assert_eq!((p.page, p.page_total, p.total), (1, 2, 60));
        assert!(DmPage::from_raw(&json!({})).is_err());
        assert!(DmPage::from_raw(&Value::Null).is_err());
    }

    #[test]
    fn dm_history() {
        let h = History::from_dm(
            &json!({"items": [{"id": "b", "create_at": 20}, {"id": "a", "create_at": 10}, {"content": "无 ID"}]}),
            2,
        )
        .unwrap();
        assert_eq!(h.items.len(), 2);
        assert_eq!(h.items[0]["id"], "a");
        assert!(!h.end);
        assert!(History::from_dm(&json!({"items": []}), 50).unwrap().end);
        assert!(History::from_dm(&json!([]), 50).is_err());
        assert!(History::from_dm(&Value::Null, 50).is_err());
    }
}
