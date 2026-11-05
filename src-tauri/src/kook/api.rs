use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Method, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::model::{
    Asset, Channel, DmChat, DmPage, DmSent, GuildAccess, History, MemberPage, MessageLocation,
    Page, merge_channels,
};
use super::{BASE, CLIENT_VERSION, USER_AGENT};
use crate::error::{AppError, AppResult};

/// REST 客户端。v3 接口返回 `{code, message, data}`，v2 接口直接返回数据本体。
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
        // 凭据无效时返回 HTTP 401，响应体为 {code: 401, message}。须在 error_for_status 之前识别。
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            return Err(unauthorized(&body));
        }
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::RateLimited);
        }
        // 业务错误可能以 HTTP 4xx 返回，响应体仍为 {code, message}，例如语音令牌的 40310 密码错误。
        // 能识别业务码时按业务错误处理，否则保留 HTTP 错误。
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

    /// GET 并附带网关会话头，用于激活服务器推送。
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

/// 4xx 响应体中的业务错误。没有非零 code 时返回 `None`。
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

/// 频道列表分页上限，防止异常 meta 导致无限请求。50 条每页，约 5000 个频道。
const MAX_CHANNEL_PAGES: u32 = 100;

/// 历史消息每页条数，即服务端上限。
pub const HISTORY_PAGE: usize = 50;
/// 私信会话每页数量（服务端默认值）。
pub const DM_PAGE: usize = 50;
/// 消息不存在或无权限查看。
const MESSAGE_GONE: i64 = 40012;

/// 消息类型：1 文本，2 图片，9 `KMarkdown`。
pub mod msg_type {
    pub const TEXT: u8 = 1;
    pub const IMAGE: u8 = 2;
    /// 文件。服务端转为卡片消息（类型 10，含 file 模块）。
    pub const FILE: u8 = 4;
    pub const KMARKDOWN: u8 = 9;
}

/// 回应请求被拒绝，但消息已处于目标状态：重复添加为“你已经点过该表情”，取消不存在的回应为“该reaction不存在”。
/// 两者都是 code 40000，与无效表情相同，只能按提示区分。
/// 私信删除的 HTTP 400 响应：code 40001 为已删除（重复删除），其余照常为错误。
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

/// 上传大小上限。实测普通账号 5 MiB（含）可上传，多 1 字节为 code 40014；服务端收完全部内容才拒绝，故先在本地检查。
pub const UPLOAD_LIMIT: u64 = 5 * 1024 * 1024;
const UPLOAD_CHUNK: usize = 64 * 1024;

/// 空文件会被服务端换成默认图片，超限文件会在传完后被拒绝，两者都在本地拦截。
///
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
    pub async fn channels(&self, guild_id: &str) -> AppResult<Vec<Channel>> {
        // 服务端每页最多 50 条，忽略更大的 page_size。按 meta.page_total 读完所有页。
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
        let (list, skipped) = merge_channels(pages);
        if skipped > 0 {
            log::warn!("频道列表中 {skipped} 个条目缺少 ID，已跳过");
        }
        Ok(list)
    }

    /// 当前账号在服务器中的角色与权限依据。
    ///
    /// # Errors
    ///
    /// 请求失败或响应格式异常时返回错误；无效服务器返回 HTTP 204，按格式错误处理。
    pub async fn guild_access(&self, guild_id: &str) -> AppResult<GuildAccess> {
        let data = self.get("/api/v2/guild/view", &[("id", guild_id)]).await?;
        GuildAccess::from_raw(&data).map_err(AppError::Format)
    }

    /// 一页服务器成员。传入频道时只返回可查看该频道的成员。页大小固定为 50。
    ///
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

    /// 订阅频道成员列表区间 `[start, end]` 的在线状态推送（`updated_user_list`），`-1, -1` 取消。须带网关会话。
    ///
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

    /// 激活服务器，使当前网关会话接收该服务器的频道消息。
    ///
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
        // 服务端每页最多 50 条。msgid 为空取最新一页，否则取严格早于该消息的一页（内部 id 或 rong_id 均可）。
        self.history_page(channel_id, "before", before.unwrap_or(""))
            .await
    }

    /// 取严格晚于 `after` 的一页，按时间升序，不含游标本身；`end` 表示已到达最新。游标消息已删除时仍可用。
    /// 不使用 `flag=around`：它忽略 `page_size`，会返回整个频道。
    ///
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

    /// 按 `rong_id` 查找消息所在频道。消息已删除、ID 无效或无权限时服务端都返回 code 40012，无法区分，统一返回 `None`。
    ///
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

    /// 编辑本人消息。内容与原来相同时服务端同样返回成功。
    ///
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

    /// 删除消息。成功为 204；重复删除同样为 204；无权删除他人消息为 HTTP 403。
    ///
    /// # Errors
    ///
    /// 请求失败时返回错误。
    pub async fn delete_message(&self, msg_id: &str) -> AppResult<()> {
        self.delete(&format!("/api/v2/messages/0/{msg_id}"))
            .await
            .map(drop)
    }

    /// 添加或取消回应。Unicode 表情直接传字符本身。
    /// 重复添加与取消不存在的回应已是目标状态，视为成功。
    ///
    /// # Errors
    ///
    /// 请求失败时返回错误，例如表情无效（code 40000）或消息已删除（HTTP 404）。
    pub async fn reaction(&self, msg_id: &str, emoji: &str, add: bool) -> AppResult<()> {
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

    /// 私信会话列表，`page` 从 1 开始，每页 [`DM_PAGE`] 个，按最近消息时间降序。
    ///
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

    /// 按用户 ID 打开私信会话。已有会话时返回同一个 `chat_code`，不新建，可重复调用。
    ///
    /// # Errors
    ///
    /// 请求失败或响应缺少会话字段时返回错误。
    pub async fn open_dm(&self, user_id: &str) -> AppResult<DmChat> {
        let data = self
            .post("/api/v3/user-chat/create", &json!({ "target_id": user_id }))
            .await?;
        DmChat::from_raw(&data).ok_or_else(|| AppError::Format("私信会话格式异常".into()))
    }

    /// 私信历史，按 `chat_code` 查询。`before` 为空取最新一页，否则取严格早于该消息（`id`）的一页。
    ///
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

    /// 标记私信已读到 `msg_id`（`time` 为其 `create_at`）。读取历史不会自动标记。
    ///
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

    /// 发送私信。`user_id` 为对方用户 ID（不是 `chat_code`），`quote` 为被引用消息的 `msg_id`。
    /// 服务端不按 nonce 去重，结果不确定时不得直接重发。
    ///
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

    /// 编辑私信。服务端不校验作者（对方也能改本人消息），调用方只能对本人消息调用。
    ///
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

    /// 删除本人私信。频道删除接口对私信返回 204 但不删除，不能混用。
    /// 重复删除为 HTTP 400 code 40001，视为已是目标状态；他人消息同为 40001，调用方只能对本人消息调用。
    ///
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

    /// 添加或取消私信回应。重复添加与取消不存在的回应视为成功（与频道相同）。
    ///
    /// # Errors
    ///
    /// 请求失败时返回错误，例如消息已删除（HTTP 404）。
    pub async fn dm_reaction(&self, msg_id: &str, emoji: &str, add: bool) -> AppResult<()> {
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

    /// 上传资源（multipart 字段 `file`）。`progress(已交给连接的字节, 总字节)` 随发送调用。
    ///
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

    /// 主网关地址。每个地址只能建立一次新会话。
    ///
    /// # Errors
    ///
    /// 请求失败或响应缺少 url 时返回错误。
    pub async fn gateway_url(&self) -> AppResult<String> {
        let data = self
            .get("/api/v3/gateway/index", &[("compress", "1")])
            .await?;
        str_field(&data, "url")
    }

    /// 语音信令地址。每个地址只能使用一次。密码频道需要密码，服务端记住后可传空串；错误返回 40310。
    ///
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
}
