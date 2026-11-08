import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api, MsgType, type Raw } from "../../lib/api";
import type { Message } from "../types";
import { isText, parseCards, toAttachment, Kind } from "../../lib/content";
import { voiceNetworkBack } from "../voice";
import { Effect, playEffect } from "../effects";
import { ui, currentChannelId, currentChatCode } from "../ui";
import { loadAccess } from "./access";
import { expire } from "./auth";
import { parse, plainContent, remember, rememberRefs, stamp, toQuote, toUser, voiceUsers } from "./convert";
import { loadDms, noteDmRead, onPrivate, refreshDm } from "./dm";
import { applyReaction, onUpdated, removeMessage } from "./edit";
import { append, refreshLatest } from "./history";
import { dropSubscription, ensureMembers, invalidateMembers, onUserList, patchMember } from "./members";
import { activate, activated, refreshGuild, reloadChannels, visited } from "./nav";
import { dmKey, guildOf, isDmKey, report, session, setSession, st } from "./state";
import { noteRead, noteUnread, syncUnread, windowActive } from "./unread";

// 网关

let unlisten: UnlistenFn[] = [];

export function stopListeners() {
  unlisten.forEach((f) => f());
  unlisten = [];
  activated.clear();
  visited.clear();
}

// 补偿代次：新的补偿或退出登录使进行中的补偿失效。
let syncGen = 0;

// 补偿未完成（被断线打断）。之后即使续传成功，也须重新补偿：续传只补发本次断线期间的事件。
let syncPending = false;

// 新会话后补偿：重新激活所有进入过的服务器，补拉全部已缓存频道（当前频道优先）。
// 单个服务器或频道失败不中断其余，结果汇总为 rebuilt 或 partial；首个会话（quiet）不提示。
export async function resync(quiet = false) {
  const my = ++syncGen;
  const gen = st.account;
  syncPending = true;
  activated.clear();
  setSession("gateway", "syncing");
  const activations = Promise.all([...visited].map((id) => activate(id)));
  // 断线期间角色、权限与在线状态可能变化，无推送补发：重新拉取。
  const accesses = Promise.all([...visited].map((id) => loadAccess(id, true)));
  // 断线期间的语音频道进出无补发：重新拉取已加载服务器的频道列表。
  for (const g of session.guilds) if (g.loaded) void reloadChannels(g.id);
  // 订阅属于网关会话，新会话须重新订阅（刷新成功后进行）。
  dropSubscription();
  if (ui.showMembers) void ensureMembers(true);
  const cur = currentChannelId();
  const dmCodes = Object.keys(session.messages).filter(isDmKey).map((k) => k.slice(3));
  const ids = Object.keys(session.messages).filter((k) => !isDmKey(k)).sort((a, b) => Number(b === cur) - Number(a === cur));
  let failed = 0;
  for (const id of ids) {
    if (my !== syncGen || gen !== st.account) return;
    if (!(await refreshLatest(id))) failed++;
  }
  // 私信：刷新会话列表（未读与最近消息）并补拉已打开过的会话。
  if (session.dmList.loaded && !(await loadDms())) failed++;
  for (const code of dmCodes) {
    if (my !== syncGen || gen !== st.account) return;
    if (!(await refreshDm(code))) failed++;
  }
  failed += (await activations).filter((ok) => !ok).length;
  failed += (await accesses).filter((ok) => !ok).length;
  if (my !== syncGen || gen !== st.account) return;
  if (!(await syncUnread())) failed++;
  if (my !== syncGen || gen !== st.account) return;
  syncPending = false;
  setSession({ gateway: "online", recovery: quiet && !failed ? null : failed ? "partial" : "rebuilt" });
}

// 立即重连（跳过退避），例如恢复联网或用户点击。
export function reconnectNow() {
  if (session.gateway === "reconnecting") api.gatewayReconnect().catch(report);
}

// 网络恢复时立即重连网关，并通知语音会话。
export function networkBack() {
  reconnectNow();
  voiceNetworkBack();
}

export type GatewayStatus =
  | { state: "ready"; kind: "new" | "resumed" | "rebuilt" }
  | { state: "disconnected"; reason: string; retry_in_ms: number };

export function onGatewayStatus(st: GatewayStatus) {
  if (st.state === "disconnected") {
    syncGen++;
    setSession({ gateway: "reconnecting", gatewayInfo: { reason: st.reason, retryAt: Date.now() + st.retry_in_ms }, recovery: null });
    return;
  }
  setSession({ gatewayInfo: null });
  // 网关断线重连后，语音会话可能也已断开。
  if (st.kind !== "new") voiceNetworkBack();
  if (st.kind === "resumed" && !syncPending) {
    // 续传：断线期间的事件已由服务端补发，激活状态在会话内保持。
    setSession({ gateway: "online", recovery: "resumed" });
    return;
  }
  // 首个会话：就绪前已打开的频道可能漏掉推送，同样补拉，但不提示恢复结果。
  void resync(st.kind === "new");
}

export async function startGateway(gen: number) {
  stopListeners();
  setSession("gateway", "connecting");
  const fns = await Promise.all([
    listen<GatewayStatus>("gateway://status", ({ payload }) => onGatewayStatus(payload)),
    listen<string>("session://expired", ({ payload }) => expire(payload, gen)),
    listen<Raw>("gateway://event", ({ payload }) => onEvent(payload)),
  ]);
  if (gen !== st.account) return fns.forEach((f) => f());
  unlisten = fns;
  await api.gatewayStart();
}

// 最近处理过的消息 ID。重复下发的同一消息不重复计入未读（频道未缓存时无法靠列表去重）。
export const seenMsgs = new Set<string>();
export const SEEN_LIMIT = 2000;
export function firstSeen(id: string): boolean {
  if (!id || seenMsgs.has(id)) return false;
  seenMsgs.add(id);
  if (seenMsgs.size > SEEN_LIMIT) seenMsgs.delete(seenMsgs.values().next().value!);
  return true;
}

export function onEvent(d: Raw) {
  if (d.type === "SYS_MSG") return onSystem(parse(d.content), String(d.toUserId ?? d.target_id ?? ""));
  if (d.channelType === "PERSON" || d.type === "PRIVATE_MSG") return onPrivate(d);
  if (d.channelType === "GROUP" || d.type === "CHANNEL_MSG" || d.channel_type === "GROUP") {
    if (!firstSeen(String(d.msgId ?? ""))) return;
    const content = parse(d.content);
    // 推送中的 content 与 extra 都是 JSON 字符串。
    const extra: Raw = parse(d.extra ?? content?.extra) ?? {};
    const channelId = String(d.toUserId ?? d.target_id);
    const author = toUser(extra.author ?? { id: d.fromUserId });
    remember(author);
    const createAt = Number(d.msgTimestamp ?? Date.now());
    const msg: Message = {
      id: String(d.msgId), author: author.id, createAt, ...stamp(createAt),
      type: Number(extra.type ?? content?.type ?? MsgType.TEXT),
      content: String(content?.content ?? (typeof d.content === "string" ? d.content : "")),
      quote: toQuote(extra.quote),
      name: author.name, avatar: author.avatar, system: Boolean(extra.author?.is_sys) || undefined,
    };
    rememberRefs(extra.kmarkdown);
    if (typeof extra.last_msg_content === "string") setPreview(channelId, extra.last_msg_content);
    // 文件消息在服务端转为卡片；推送的 extra.type 未确认，按内容识别。
    if (msg.type === Kind.FILE && parseCards(msg.content)?.length) msg.type = Kind.CARD;
    if (!isText(msg.type) && msg.type !== Kind.IMAGE) msg.attachment = toAttachment(extra.attachments);
    append(channelId, msg);
    // 本账号（其他设备）发送的消息不计未读。
    if (author.id === session.me?.id) return;
    const guildId = String(extra.guild_id ?? "") || guildOf(channelId)?.id || "";
    // 提及条数以服务端 unread_count_changed 为准（含角色提及），此处只记未读。
    noteUnread(channelId, guildId, createAt, 0);
  }
}

// 更新文字频道的单行预览。
export function setPreview(channelId: string, text: string) {
  for (const [gi, g] of session.guilds.entries()) {
    for (const [i, grp] of g.groups.entries()) {
      const j = grp.channels.findIndex((c) => c.id === channelId);
      if (j >= 0) return void setSession("guilds", gi, "groups", i, "channels", j, "preview", text || undefined);
    }
  }
}

// 语音频道成员进出。加入时从同一服务器的其他语音频道移出，避免切换频道时漏收退出事件留下残影。
export function onVoicePresence(body: Raw, joined: boolean) {
  const uid = String(body.id ?? "");
  const cid = String(body.channel?.id ?? "");
  const gid = String(body.channel?.guild_id ?? "") || guildOf(cid)?.id || "";
  const gi = session.guilds.findIndex((g) => g.id === gid);
  if (!uid || !cid || gi < 0) return;
  const [user] = voiceUsers([body]);
  session.guilds[gi].groups.forEach((g, i) => g.channels.forEach((ch, j) => {
    if (ch.type !== "voice") return;
    const list = ch.users ?? [];
    const has = list.some((x) => x.id === uid);
    const want = joined && ch.id === cid;
    if (has && want) return void setSession("guilds", gi, "groups", i, "channels", j, "users", list.map((x) => (x.id === uid ? user : x)));
    if (has === want) return;
    setSession("guilds", gi, "groups", i, "channels", j, "users", want ? [...list, user] : list.filter((x) => x.id !== uid));
  }));
}

export function onSystem(c: Raw | null, target = "") {
  if (!c) return;
  const body = c.body ?? {};
  switch (c.type) {
    case "updated_user_list": {
      const data = body.user_list_data?.data;
      if (Array.isArray(data)) onUserList(String(body.channel_id ?? ""), data);
      break;
    }
    case "joined_channel":
    case "exited_channel":
      onVoicePresence(body, c.type === "joined_channel");
      break;
    case "user_state_change": {
      // 已激活服务器内成员资料变化（如服务器昵称）。toUserId 为服务器 ID，user_id 为数字。
      const uid = String(body.user_id ?? "");
      if (target && uid) patchMember(target, uid, body);
      break;
    }
    case "joined_guild":
    case "exited_guild":
      if (target) invalidateMembers(target);
      break;
    case "user_role_change": {
      const gid = String(body.guild_id ?? target);
      if (!gid) break;
      if (String(body.user_id ?? "") === session.me?.id) void loadAccess(gid, true);
      invalidateMembers(gid);
      break;
    }
    case "updated_role":
    case "added_role":
    case "deleted_role":
      if (target) {
        void loadAccess(target, true);
        invalidateMembers(target);
      }
      break;
    case "added_channel":
    case "updated_channel":
    case "deleted_channel":
    case "batch_added_channel":
    case "batch_updated_channel":
    case "batch_deleted_channel":
    case "sort_channel": {
      // 频道或覆盖变化：重新拉取该服务器频道列表。
      const gid = String(body.guild_id ?? target);
      if (session.guilds.some((g) => g.id === gid && g.loaded)) void reloadChannels(gid);
      break;
    }
    // 消息事件的 ID 统一取 body.rong_id（deleted_message 的 id 不是消息 ID）。
    case "message_updated":
    case "updated_message":
      onUpdated(String(body.channel_id ?? ""), String(body.rong_id ?? body.msg_id ?? ""), body);
      break;
    case "deleted_message": {
      const cid = String(body.channel_id ?? "");
      const id = String(body.rong_id ?? body.msg_id ?? "");
      const list = session.messages[cid];
      const latest = list?.[list.length - 1]?.id === id;
      removeMessage(cid, id);
      // 删除事件不带新的预览；删除的是最新消息或本地没有消息时重新拉取频道列表。
      const gid = String(body.guild_id ?? "") || guildOf(cid)?.id || "";
      if (gid && (latest || !list)) refreshGuild(gid);
      break;
    }
    case "add_reaction":
    case "added_reaction":
    case "delete_reaction":
    case "deleted_reaction": {
      const id = String(body.emoji?.id ?? "");
      const user = String(body.user_id ?? "");
      if (!id || !user) break;
      applyReaction(String(body.channel_id ?? ""), String(body.rong_id ?? body.msg_id ?? ""),
        { id, name: String(body.emoji?.name ?? id) }, user, c.type.startsWith("add"));
      break;
    }
    case "desktop_notification":
      onNotification(body);
      break;
    case "unread_count_changed": {
      // 频道由已读变为未读，或新增提及。未激活的服务器同样下发。
      const id = String(body.channel_id ?? "");
      if (!id) break;
      const at = Number(body.msg_time ?? body.time ?? 0);
      noteUnread(id, String(body.guild_id ?? ""), at, Math.max(0, Number(body.mention_count ?? 0)));
      break;
    }
    case "mark_as_batchread":
      // 本账号在任一端标记已读（包括本端）。
      for (const c of Array.isArray(body.channel) ? body.channel : []) if (c?.channel_id) noteRead(String(c.channel_id), Number(c.time ?? 0));
      // user_chat 条目结构未抓到样本：按常见字段尝试，识别不了时刷新会话列表。
      for (const c of Array.isArray(body.user_chat) ? body.user_chat : []) noteDmRead(c);
      break;
    // 私信事件：以 code（chat_code）定位会话，以 rong_id 定位消息（private_message_deleted 的 id 不是消息 ID）。
    case "private_message_updated": {
      const code = String(body.code ?? "");
      const id = String(body.rong_id ?? body.msg_id ?? "");
      if (!code || !id) break;
      onUpdated(dmKey(code), id, body);
      // 编辑的是最后一条时同步摘要。
      const list = session.messages[dmKey(code)];
      const i = session.dms.findIndex((c) => c.code === code);
      if (i >= 0 && list?.[list.length - 1]?.id === id) setSession("dms", i, "preview", plainContent(Number(body.type ?? Kind.KMARKDOWN), String(body.content ?? "")));
      break;
    }
    case "private_message_deleted": {
      const code = String(body.code ?? "");
      const id = String(body.rong_id ?? body.msg_id ?? "");
      if (code && id) removeMessage(dmKey(code), id);
      // 删除会改变摘要与未读，以服务端为准。
      if (code && session.dmList.loaded) void loadDms();
      break;
    }
    case "private_add_reaction":
    case "private_delete_reaction": {
      const code = String(body.code ?? "");
      const eid = String(body.emoji?.id ?? "");
      const user = String(body.user_id ?? "");
      if (!code || !eid || !user) break;
      applyReaction(dmKey(code), String(body.rong_id ?? body.msg_id ?? ""), { id: eid, name: String(body.emoji?.name ?? eid) }, user, c.type === "private_add_reaction");
      break;
    }
    case "updated_guild": {
      const i = session.guilds.findIndex((g) => g.id === body.id);
      if (i >= 0) setSession("guilds", i, { name: body.name, icon: body.icon || undefined });
      // 频道覆盖变化只推送此事件：同时刷新角色依据与频道覆盖。
      const gid = String(body.id ?? "");
      if (i >= 0 && visited.has(gid)) refreshGuild(gid);
      break;
    }
  }
}

export function resetGateway() {
  syncGen++;
  syncPending = false;
  seenMsgs.clear();
}

// 桌面通知推送已按服务端通知级别过滤，据此播放消息提示音。正在查看的会话不播放。
const notified = new Set<string>();
export function onNotification(body: Raw) {
  const extra: Raw = body.extra ?? {};
  const id = String(extra.msg_id ?? "");
  if (id) {
    if (notified.has(id)) return;
    notified.add(id);
    if (notified.size > 500) notified.delete(notified.values().next().value!);
  }
  if (extra.gift_triggered) return;
  const author = String(extra.author_id ?? extra.chat_user_id ?? "");
  if (author && author === session.me?.id) return;
  const channelId = String(extra.channel_id ?? "");
  const viewing = windowActive() && !ui.settings && (
    (channelId && channelId !== "0" && channelId === currentChannelId()) ||
    (!channelId && !!extra.chat_user_id && session.dms.some((c) => c.code === currentChatCode() && c.target.id === String(extra.chat_user_id)))
  );
  if (viewing) return;
  playEffect(extra.mention ? Effect.MENTION : Effect.MESSAGE);
}

// 本人主动退出语音后立即从频道成员中移除，不等网关事件。
export function dropSelfFromVoice(channelId: string) {
  const uid = session.me?.id;
  if (uid && channelId) onVoicePresence({ id: uid, channel: { id: channelId } }, false);
}
