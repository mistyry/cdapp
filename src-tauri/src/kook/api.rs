use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Method, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::model::{
    Asset, Channel, DmChat, DmPage, DmSent, GuildAccess, History, MemberPage, MessageLocation,
    Page, flatten_view_channels, merge_channels,
};
use super::{BASE, CLIENT_VERSION, USER_AGENT};
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    token: Option<String>,
}

impl Api {
    /// # Errors
    ///
    /// HTTP 客户端初始化失败时返回错误。
    pub fn new(token: Option<String>) -> AppResult<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_LANGUAGE, HeaderValue::from_static("zh-CN"));
        headers.insert("x-client-version", HeaderValue::from_static(CLIENT_VERSION));
        headers.insert(
            "x-client-utm",
            HeaderValue::from_static("official.site....web"),
        );
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://www.kookapp.cn/app/"),
        );
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self { http, token })
    }

    #[must_use]
    pub fn with_token(&self, token: Option<String>) -> Self {
        Self {
            http: self.http.clone(),
            token,
        }
    }

    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let req = self.http.request(method, format!("{BASE}{path}"));
        match &self.token {
            Some(t) => req.header(header::AUTHORIZATION, t),
            None => req,
        }
    }

    async fn send(req: RequestBuilder) -> AppResult<Value> {
        Self::finish(req.send().await?).await
    }

    async fn finish(resp: reqwest::Response) -> AppResult<Value> {
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            return Err(unauthorized(&body));
        }
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::RateLimited);
        }
        if resp.status().is_client_error() {
            let err = resp.error_for_status_ref().map(|_| ()).unwrap_err();
            let bytes = resp.bytes().await.unwrap_or_default();
            return Err(client_error(&bytes).unwrap_or(AppError::Http(err)));
        }
        let bytes = resp.error_for_status()?.bytes().await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        let body: Value = serde_json::from_slice(&bytes)?;
        unwrap(body)
    }

    /// # Errors
    ///
    /// 网络错误、HTTP 错误或响应不是 JSON 时返回错误。
    pub async fn get_external(&self, url: &str) -> AppResult<Value> {
        let bytes = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut out)
                .map_err(|e| AppError::Format(format!("解压失败：{e}")))?;
            return Ok(serde_json::from_slice(&out)?);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// # Errors
    ///
    /// 网络错误或 HTTP 错误时返回错误。
    pub async fn get_bytes(&self, url: &str) -> AppResult<bytes::Bytes> {
        Ok(self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?)
    }

    /// # Errors
    ///
    /// 网络错误、HTTP 错误或接口返回非零 code 时返回错误。
    pub async fn get<Q: Serialize + ?Sized>(&self, path: &str, query: &Q) -> AppResult<Value> {
        Self::send(self.request(Method::GET, path).query(query)).await
    }

    /// # Errors
    ///
    /// 同 [`Api::get`]。
    pub async fn post<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> AppResult<Value> {
        Self::send(self.request(Method::POST, path).json(body)).await
    }

    /// # Errors
    ///
    /// 同 [`Api::get`]。
    pub async fn delete(&self, path: &str) -> AppResult<Value> {
        Self::send(self.request(Method::DELETE, path)).await
    }

    async fn delete_json<B: Serialize + ?Sized>(&self, path: &str, body: &B) -> AppResult<Value> {
        Self::send(self.request(Method::DELETE, path).json(body)).await
    }

    async fn reaction_v2(&self, path: &str, msg_id: &str, emoji: &str, add: bool) -> AppResult<()> {
        let body = json!({ "id": msg_id, "emoji": emoji });
        if add {
            self.post(path, &body).await.map(drop)
        } else {
            self.delete_json(path, &body).await.map(drop)
        }
    }

    async fn get_with_session(
        &self,
        path: &str,
        query: &[(&str, &str)],
        session: &str,
    ) -> AppResult<Value> {
        Self::send(
            self.request(Method::GET, path)
                .query(query)
                .header("x-client-sessionid", session),
        )
        .await
    }

    /// # Errors
    ///
    /// 同 [`Api::get`]，并在反序列化失败时返回错误。
    pub async fn get_as<T: DeserializeOwned, Q: Serialize + ?Sized>(
        &self,
        path: &str,
        query: &Q,
    ) -> AppResult<T> {
        Ok(serde_json::from_value(self.get(path, query).await?)?)
    }
}

fn is_personal_emoji(emoji: &str) -> bool {
    emoji.split('/').count() == 3
}

fn unwrap(body: Value) -> AppResult<Value> {
    if let Value::Object(map) = &body
        && let (Some(code), true) = (
            map.get("code").and_then(Value::as_i64),
            map.contains_key("data"),
        )
    {
        if code == 401 {
            return Err(unauthorized(&body));
        }
        if code != 0 {
            let message = map
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            return Err(AppError::Api { code, message });
        }
        return Ok(map.get("data").cloned().unwrap_or(Value::Null));
    }
    Ok(body)
}

fn client_error(bytes: &[u8]) -> Option<AppError> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let code = body
        .get("code")
        .and_then(Value::as_i64)
        .filter(|c| *c != 0)?;
    if code == 401 {
        return Some(unauthorized(&body));
    }
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(AppError::Api { code, message })
}

fn unauthorized(body: &Value) -> AppError {
    AppError::Unauthorized(
        body.get("message")
            .and_then(Value::as_str)
            .unwrap_or("凭据无效")
            .to_owned(),
    )
}

const MAX_CHANNEL_PAGES: u32 = 100;

fn merged(pages: impl IntoIterator<Item = Vec<Value>>) -> Vec<Channel> {
    let (list, skipped) = merge_channels(pages);
    if skipped > 0 {
        log::warn!("频道列表中 {skipped} 个条目缺少 ID，已跳过");
    }
    list
}

fn channels_of_view(view: &Value) -> Option<Vec<Channel>> {
    let top = view.get("channels")?.as_array().filter(|a| !a.is_empty())?;
    Some(merged([flatten_view_channels(top)]))
}

pub const HISTORY_PAGE: usize = 50;
pub const DM_PAGE: usize = 50;
const MESSAGE_GONE: i64 = 40012;

pub mod msg_type {
    pub const TEXT: u8 = 1;
    pub const IMAGE: u8 = 2;
    pub const FILE: u8 = 4;
    pub const KMARKDOWN: u8 = 9;
}

fn dm_delete_result(body: &Value) -> AppResult<()> {
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(400);
    if code == DM_ALREADY_DELETED {
        return Ok(());
    }
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .unwrap_or("请求被拒绝")
        .to_owned();
    Err(AppError::Api { code, message })
}

const DM_ALREADY_DELETED: i64 = 40001;

fn reaction_settled(add: bool, e: &AppError) -> bool {
    match e {
        AppError::Api {
            code: 40000,
            message,
        } => {
            if add {
                message.contains("已经点过")
            } else {
                message.contains("reaction不存在")
            }
        }
        _ => false,
    }
}

pub const UPLOAD_LIMIT: u64 = 5 * 1024 * 1024;
const UPLOAD_CHUNK: usize = 64 * 1024;

/// # Errors
///
/// 大小为 0 或超过 [`UPLOAD_LIMIT`] 时返回本地错误。
pub fn check_upload(size: u64) -> AppResult<()> {
    if size == 0 {
        return Err(AppError::Other("不能上传空文件".into()));
    }
    if size > UPLOAD_LIMIT {
        return Err(AppError::Other("文件超过 5 MB 上限".into()));
    }
    Ok(())
}

fn nonce() -> String {
    format!("{:x}", rand::random::<u64>())
}

impl Api {
    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn me(&self) -> AppResult<Value> {
        self.get("/api/v3/user/me", &()).await
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn guilds(&self) -> AppResult<Value> {
        self.get(
            "/api/v3/guild/index",
            &[("guild_id", "none"), ("type", "-1")],
        )
        .await
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn guild_folders(&self) -> AppResult<Value> {
        let v = self
            .get("/api/v3/guild/folders", &[("page_size", "1024")])
            .await?;
        Ok(v.get("items").cloned().unwrap_or(Value::Array(Vec::new())))
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn channels(&self, guild_id: &str) -> AppResult<Vec<Channel>> {
        let view = self.get("/api/v2/guild/view", &[("id", guild_id)]).await?;
        if let Some(list) = channels_of_view(&view) {
            return Ok(list);
        }
        self.channel_pages(guild_id).await
    }

    async fn channel_pages(&self, guild_id: &str) -> AppResult<Vec<Channel>> {
        let mut pages = Vec::new();
        let mut page = 1u32;
        loop {
            let n = page.to_string();
            let data = self
                .get(
                    "/api/v3/channel/list",
                    &[("guild_id", guild_id), ("page", &n), ("page_size", "50")],
                )
                .await?;
            let p = Page::from_raw(data).map_err(AppError::Format)?;
            let empty = p.items.is_empty();
            pages.push(p.items);
            if empty || page >= p.page_total.min(MAX_CHANNEL_PAGES) {
                if p.page_total > MAX_CHANNEL_PAGES {
                    log::warn!("频道列表超过 {MAX_CHANNEL_PAGES} 页，已截断");
                }
                break;
            }
            page += 1;
        }
        Ok(merged(pages))
    }

    /// # Errors
    ///
    /// 请求失败或响应缺少服务器 ID 时返回错误。
    pub async fn channel_guild(&self, channel_id: &str) -> AppResult<String> {
        let v = self
            .get("/api/v3/channel/view", &[("target_id", channel_id)])
            .await?;
        super::model::id_str(v.get("guild_id"))
            .filter(|g| !g.is_empty())
            .ok_or_else(|| AppError::Format("频道详情缺少服务器 ID".into()))
    }

    /// # Errors
    ///
    /// 请求失败或响应格式异常时返回错误；无效服务器返回 HTTP 204，按格式错误处理。
    pub async fn guild_access(&self, guild_id: &str) -> AppResult<GuildAccess> {
        let data = self.get("/api/v2/guild/view", &[("id", guild_id)]).await?;
        GuildAccess::from_raw(&data).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 服务器表情请求失败或格式异常时返回错误。
    pub async fn emojis(&self) -> AppResult<super::emoji::EmojiSet> {
        let (g, p, j) = tokio::join!(
            self.get("/api/v3/vip-expression/get-all-emojis", &()),
            self.get("/api/v3/user-emoji/all", &()),
            self.get("/api/v3/vip-expression/join-list", &())
        );
        let guilds = super::emoji::parse_guilds(&g?).map_err(AppError::Format)?;
        let personal = match p.map(|v| super::emoji::parse_personal(&v)) {
            Ok(Ok(list)) => Some(list),
            Ok(Err(e)) => {
                log::warn!("个人表情格式异常：{e}");
                None
            }
            Err(AppError::Unauthorized(m)) => return Err(AppError::Unauthorized(m)),
            Err(e) => {
                log::warn!("个人表情读取失败：{e}");
                None
            }
        };
        let joined = match j.map(|v| super::emoji::parse_joined(&v)) {
            Ok(Ok(list)) => Some(list),
            Ok(Err(e)) => {
                log::warn!("已添加表情组格式异常：{e}");
                None
            }
            Err(AppError::Unauthorized(m)) => return Err(AppError::Unauthorized(m)),
            Err(e) => {
                log::warn!("已添加表情组读取失败：{e}");
                None
            }
        };
        Ok(super::emoji::EmojiSet {
            guilds,
            joined,
            personal_failed: personal.is_none(),
            personal: personal.unwrap_or_default(),
        })
    }

    /// # Errors
    ///
    /// 请求失败或响应格式异常时返回错误。
    pub async fn members(
        &self,
        guild_id: &str,
        channel_id: Option<&str>,
        page: u32,
    ) -> AppResult<MemberPage> {
        let n = page.max(1).to_string();
        let mut q = vec![("guild_id", guild_id), ("page", &n), ("page_size", "50")];
        if let Some(c) = channel_id {
            q.push(("channel_id", c));
        }
        let data = self.get("/api/v3/guild/user-list", &q).await?;
        MemberPage::from_raw(&data).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn subscribe_members(
        &self,
        channel_id: &str,
        start: i64,
        end: i64,
        session: &str,
    ) -> AppResult<Value> {
        Self::send(
            self.request(Method::POST, "/api/v3/guild/users-v4-subscribe")
                .header("x-client-sessionid", session)
                .json(&serde_json::json!({
                    "channel_id": channel_id,
                    "listen_start": start,
                    "listen_end": end,
                })),
        )
        .await
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn activate_guild(&self, guild_id: &str, session_id: &str) -> AppResult<Value> {
        self.get_with_session(
            "/api/v2/guild/view",
            &[("id", guild_id), ("active_id", guild_id)],
            session_id,
        )
        .await
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn messages(&self, channel_id: &str, before: Option<&str>) -> AppResult<History> {
        self.history_page(channel_id, "before", before.unwrap_or(""))
            .await
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn messages_after(&self, channel_id: &str, after: &str) -> AppResult<History> {
        self.history_page(channel_id, "after", after).await
    }

    async fn history_page(&self, channel_id: &str, flag: &str, msgid: &str) -> AppResult<History> {
        let size = HISTORY_PAGE.to_string();
        let data = self
            .get(
                &format!("/api/v2/messages/{channel_id}"),
                &[
                    ("flag", flag),
                    ("page_size", size.as_str()),
                    ("msgid", msgid),
                ],
            )
            .await?;
        History::from_raw(data, HISTORY_PAGE).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 其他请求失败时返回错误。
    pub async fn locate_message(&self, msg_id: &str) -> AppResult<Option<MessageLocation>> {
        match self
            .get("/api/v3/message/view", &[("msg_id", msg_id)])
            .await
        {
            Ok(data) => MessageLocation::from_raw(&data)
                .map(Some)
                .map_err(AppError::Format),
            Err(AppError::Api {
                code: MESSAGE_GONE, ..
            }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn send_message(
        &self,
        channel_id: &str,
        kind: u8,
        content: &str,
        quote: Option<&str>,
    ) -> AppResult<Value> {
        let mut body =
            json!({ "type": kind, "target_id": channel_id, "content": content, "nonce": nonce() });
        if let Some(q) = quote {
            body["quote"] = json!(q);
        }
        self.post("/api/v3/message/create", &body).await
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。内容为空为 code 40000；他人消息、无效 ID、已删除统一为 code 40012。
    pub async fn update_message(&self, msg_id: &str, content: &str) -> AppResult<()> {
        self.post(
            "/api/v3/message/update",
            &json!({ "msg_id": msg_id, "content": content }),
        )
        .await
        .map(drop)
    }

    /// # Errors
    ///
    /// 请求失败或没有权限（HTTP 403）时返回错误。
    pub async fn guild_mute(
        &self,
        guild_id: &str,
        user_id: &str,
        kind: u8,
        on: bool,
    ) -> AppResult<()> {
        let body = json!({ "type": kind, "guild_id": guild_id, "user_id": user_id });
        if on {
            self.post("/api/v2/guild-mutes", &body).await.map(drop)
        } else {
            self.delete_json("/api/v2/guild-mutes", &body)
                .await
                .map(drop)
        }
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn guild_mutes(&self, guild_id: &str) -> AppResult<(Vec<String>, Vec<String>)> {
        let v = self
            .get(&format!("/api/v2/guild-mutes/{guild_id}"), &())
            .await?;
        let ids = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| match x {
                            Value::String(s) if !s.is_empty() => Some(s.clone()),
                            Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok((ids("1"), ids("2")))
    }

    /// # Errors
    ///
    /// 请求失败或没有语音管理权限时返回错误。
    pub async fn voice_kick(&self, channel_id: &str, user_id: &str) -> AppResult<()> {
        self.post(
            "/api/v3/channel/kickout",
            &json!({ "channel_id": channel_id, "user_id": user_id }),
        )
        .await
        .map(drop)
    }

    /// # Errors
    ///
    /// 请求失败或没有语音管理权限时返回错误。
    pub async fn voice_move(&self, channel_id: &str, user_id: &str) -> AppResult<()> {
        self.post(
            "/api/v3/channel/move-user",
            &json!({ "target_id": channel_id, "user_ids": [user_id] }),
        )
        .await
        .map(drop)
    }

    /// # Errors
    ///
    /// 请求失败、没有踢出权限或目标为服务器主时返回错误。
    pub async fn guild_kick(&self, guild_id: &str, user_id: &str) -> AppResult<()> {
        self.post(
            "/api/v3/guild/kickout",
            &json!({ "guild_id": guild_id, "target_id": user_id }),
        )
        .await
        .map(drop)
    }

    /// # Errors
    ///
    /// 请求失败、没有黑名单权限或目标无效时返回错误。
    pub async fn guild_ban(&self, guild_id: &str, user_id: &str) -> AppResult<()> {
        self.post(
            "/api/v3/blacklist/create",
            &json!({ "guild_id": guild_id, "target_id": user_id, "remark": "", "del_msg_days": 0 }),
        )
        .await
        .map(drop)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn delete_message(&self, msg_id: &str) -> AppResult<()> {
        self.delete(&format!("/api/v2/messages/0/{msg_id}"))
            .await
            .map(drop)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误，例如表情无效（code 40000）或消息已删除（HTTP 404）。
    pub async fn reaction(&self, msg_id: &str, emoji: &str, add: bool) -> AppResult<()> {
        if is_personal_emoji(emoji) {
            return self
                .reaction_v2("/api/v2/messages/reaction", msg_id, emoji, add)
                .await;
        }
        let path = if add {
            "/api/v3/message/add-reaction"
        } else {
            "/api/v3/message/delete-reaction"
        };
        match self
            .post(path, &json!({ "msg_id": msg_id, "emoji": emoji }))
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if reaction_settled(add, &e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn mark_read(&self, channel_id: &str, msg_id: &str, time: i64) -> AppResult<Value> {
        let body = json!([{
            "time": time, "msg_id": msg_id, "type": "message",
            "channel_or_code": channel_id, "clear_type": "channel",
        }]);
        self.post("/api/v3/message/batch-clear", &body).await
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn unread(&self) -> AppResult<Vec<crate::kook::model::ChannelUnread>> {
        let data = self.get("/api/v2/messages/unread", &()).await?;
        crate::kook::model::parse_unread(&data).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn conversations(&self) -> AppResult<Value> {
        self.get("/api/v3/conversation/list-v2", &()).await
    }

    /// # Errors
    ///
    /// 请求失败或响应格式异常时返回错误。
    pub async fn dm_chats(&self, page: u32) -> AppResult<DmPage> {
        let (page, size) = (page.max(1).to_string(), DM_PAGE.to_string());
        let data = self
            .get(
                "/api/v3/user-chat/list",
                &[("page", page.as_str()), ("page_size", size.as_str())],
            )
            .await?;
        DmPage::from_raw(&data).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 请求失败或响应缺少会话字段时返回错误。
    pub async fn open_dm(&self, user_id: &str) -> AppResult<DmChat> {
        let data = self
            .post("/api/v3/user-chat/create", &json!({ "target_id": user_id }))
            .await?;
        DmChat::from_raw(&data).ok_or_else(|| AppError::Format("私信会话格式异常".into()))
    }

    /// # Errors
    ///
    /// 请求失败时返回错误；会话码无效为 code 40000。
    pub async fn dm_messages(&self, chat_code: &str, before: Option<&str>) -> AppResult<History> {
        let size = HISTORY_PAGE.to_string();
        let mut query = vec![("chat_code", chat_code), ("page_size", size.as_str())];
        if let Some(id) = before.filter(|s| !s.is_empty()) {
            query.extend([("msg_id", id), ("flag", "before")]);
        }
        let data = self.get("/api/v3/direct-message/list", &query).await?;
        History::from_dm(&data, HISTORY_PAGE).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn mark_dm_read(&self, chat_code: &str, msg_id: &str, time: i64) -> AppResult<()> {
        let body = json!([{
            "time": time, "msg_id": msg_id, "type": "message",
            "channel_or_code": chat_code, "clear_type": "user_chat",
        }]);
        self.post("/api/v3/message/batch-clear", &body)
            .await
            .map(drop)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误；对方无效或无权限为 code 40000；响应缺少消息 ID 为格式错误。
    pub async fn send_direct(
        &self,
        user_id: &str,
        kind: u8,
        content: &str,
        quote: Option<&str>,
    ) -> AppResult<DmSent> {
        let mut body =
            json!({ "type": kind, "target_id": user_id, "content": content, "nonce": nonce() });
        if let Some(q) = quote.filter(|q| !q.is_empty()) {
            body["quote"] = json!(q);
        }
        let data = self.post("/api/v3/direct-message/create", &body).await?;
        DmSent::from_raw(&data).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。内容为空为 code 40000；无效或已删除为 code 40012。
    pub async fn update_dm_message(&self, msg_id: &str, content: &str) -> AppResult<()> {
        self.post(
            "/api/v3/direct-message/update",
            &json!({ "msg_id": msg_id, "content": content }),
        )
        .await
        .map(drop)
    }

    /// # Errors
    ///
    /// 其余请求失败时返回错误。
    pub async fn delete_dm_message(&self, msg_id: &str) -> AppResult<()> {
        let resp = self
            .request(
                Method::DELETE,
                &format!("/api/v2/user-chats/message/{msg_id}"),
            )
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::BAD_REQUEST {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            return dm_delete_result(&body);
        }
        Self::finish(resp).await.map(drop)
    }

    /// # Errors
    ///
    /// 请求失败时返回错误，例如消息已删除（HTTP 404）。
    pub async fn dm_reaction(&self, msg_id: &str, emoji: &str, add: bool) -> AppResult<()> {
        if is_personal_emoji(emoji) {
            return self
                .reaction_v2("/api/v2/user-chats/reaction", msg_id, emoji, add)
                .await;
        }
        let path = if add {
            "/api/v3/direct-message/add-reaction"
        } else {
            "/api/v3/direct-message/delete-reaction"
        };
        match self
            .post(path, &json!({ "msg_id": msg_id, "emoji": emoji }))
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if reaction_settled(add, &e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// # Errors
    ///
    /// 本地检查失败（空文件、超过 [`UPLOAD_LIMIT`]）时不发出请求；服务端拒绝为 code 40014（文件过大）
    /// 或 40000；响应缺少地址时为格式错误。
    pub async fn upload(
        &self,
        file_name: String,
        bytes: Vec<u8>,
        progress: impl Fn(u64, u64) + Send + Sync + 'static,
    ) -> AppResult<Asset> {
        check_upload(bytes.len() as u64)?;
        let total = bytes.len() as u64;
        let chunks: Vec<Vec<u8>> = bytes.chunks(UPLOAD_CHUNK).map(<[u8]>::to_vec).collect();
        let mut sent = 0u64;
        let stream = futures_util::stream::iter(chunks.into_iter().map(move |c| {
            sent += c.len() as u64;
            progress(sent, total);
            Ok::<_, std::io::Error>(c)
        }));
        let part =
            reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), total)
                .file_name(file_name);
        let form = reqwest::multipart::Form::new().part("file", part);
        let data = Self::send(
            self.request(Method::POST, "/api/v3/asset/create")
                .multipart(form),
        )
        .await?;
        Asset::from_raw(&data).map_err(AppError::Format)
    }

    /// # Errors
    ///
    /// 请求失败或响应缺少 url 时返回错误。
    pub async fn gateway_url(&self) -> AppResult<String> {
        let data = self
            .get("/api/v3/gateway/index", &[("compress", "1")])
            .await?;
        str_field(&data, "url")
    }

    /// # Errors
    ///
    /// 请求失败或响应缺少 `gateway_url` 时返回错误。
    pub async fn voice_gateway(&self, channel_id: &str, password: &str) -> AppResult<String> {
        let body = json!({
            "invisible_join": false, "ip_discovery": [], "password": password,
            "streaming": "[null,null,\"\",\"\"]",
        });
        let data = self
            .post(&format!("/api/v2/channels/token/{channel_id}"), &body)
            .await?;
        str_field(&data, "gateway_url")
    }

    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn golive_owned(&self) -> AppResult<bool> {
        let data = self.get("/api/v3/product/golive-info", &()).await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Ok(golive_active(&data, now))
    }
}

const GOLIVE_ITEMS: [i64; 2] = [10015, 10016];

fn golive_active(data: &Value, now: u64) -> bool {
    data.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|i| {
            i.get("id")
                .and_then(Value::as_i64)
                .is_some_and(|id| GOLIVE_ITEMS.contains(&id))
        })
        .filter_map(|i| i.get("expire").and_then(Value::as_u64))
        .any(|e| e > now)
}

pub(crate) fn str_field(v: &Value, key: &str) -> AppResult<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AppError::Other(format!("响应缺少字段 {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golive_owned_by_expire() {
        let info = |std: u64, pro: u64| json!({ "items": [{ "id": 10015, "expire": std }, { "id": 10016, "expire": pro }] });
        assert!(!golive_active(&info(0, 0), 100));
        assert!(golive_active(&info(200, 0), 100));
        assert!(golive_active(&info(0, 200), 100));
        assert!(!golive_active(&info(50, 0), 100));
        assert!(!golive_active(
            &json!({ "items": [{ "id": 1, "expire": 200 }] }),
            100
        ));
        assert!(!golive_active(&json!({}), 100));
    }

    #[test]
    fn upload_size_checked_locally() {
        assert!(check_upload(0).is_err());
        assert!(check_upload(1).is_ok());
        assert!(check_upload(UPLOAD_LIMIT).is_ok());
        let e = check_upload(UPLOAD_LIMIT + 1).unwrap_err();
        assert_eq!(e.kind(), crate::error::ErrorKind::Local);
    }

    #[test]
    fn client_error_body_keeps_code() {
        let e = client_error(
            br#"{"code":40310,"message":"\u9891\u9053\u5bc6\u7801\u9519\u8bef","data":[]}"#,
        )
        .unwrap();
        assert!(matches!(e, AppError::Api { code: 40310, .. }));
        assert!(matches!(
            client_error(br#"{"code":401,"message":"x"}"#),
            Some(AppError::Unauthorized(_))
        ));
        assert!(client_error(b"<html>").is_none());
        assert!(client_error(br#"{"code":0}"#).is_none());
    }

    #[tokio::test]
    async fn http_401_status_is_unauthorized() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            for body in [r#"{"code":0,"message":"token 失效"}"#, "<html>"] {
                let (mut s, _) = l.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        let api = Api::new(None).unwrap();
        for _ in 0..2 {
            let resp = api
                .http
                .get(format!("http://{addr}/api/v3/gateway/index"))
                .send()
                .await
                .unwrap();
            let e = Api::finish(resp).await.unwrap_err();
            assert!(matches!(e, AppError::Unauthorized(_)), "{e:?}");
        }
    }

    #[test]
    fn code_401_is_unauthorized() {
        let e = unwrap(json!({"code": 401, "message": "请重新登录", "data": {}})).unwrap_err();
        assert!(matches!(e, AppError::Unauthorized(ref m) if m == "请重新登录"));
        assert!(matches!(
            unwrap(json!({"code": 40000, "message": "x", "data": {}})),
            Err(AppError::Api { code: 40000, .. })
        ));
        assert_eq!(unwrap(json!({"code": 0, "data": 1})).unwrap(), json!(1));
    }

    #[test]
    fn dm_repeat_delete_is_settled() {
        assert!(dm_delete_result(&json!({"code": 40001, "message": "", "data": {}})).is_ok());
        let e = dm_delete_result(&json!({"code": 40000, "message": "", "data": {}})).unwrap_err();
        assert!(matches!(e, AppError::Api { code: 40000, ref message } if message == "请求被拒绝"));
        assert_eq!(e.kind(), crate::error::ErrorKind::Rejected);
        assert!(matches!(
            dm_delete_result(&Value::Null),
            Err(AppError::Api { code: 400, .. })
        ));
    }

    #[test]
    fn reaction_already_settled_is_success() {
        let api = |m: &str| AppError::Api {
            code: 40000,
            message: m.into(),
        };
        assert!(reaction_settled(true, &api("你已经点过该表情")));
        assert!(reaction_settled(false, &api("该reaction不存在")));
        assert!(!reaction_settled(false, &api("你已经点过该表情")));
        assert!(!reaction_settled(true, &api("该reaction不存在")));
        assert!(!reaction_settled(true, &api("表情无效")));
        assert!(!reaction_settled(
            true,
            &AppError::Api {
                code: 40012,
                message: "你已经点过该表情".into()
            }
        ));
    }

    #[test]
    fn personal_emoji_detection() {
        assert!(super::is_personal_emoji("1/22/abc"));
        assert!(!super::is_personal_emoji("100/abc"));
        assert!(!super::is_personal_emoji("👍"));
    }
}
