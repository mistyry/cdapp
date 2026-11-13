import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api, MsgType, type Raw } from "../../lib/api";
import type { Message } from "../types";
import { isText, parseCards, toAttachment, Kind } from "../../lib/content";
import { voice, voiceActive, voiceNetworkBack } from "../voice";
import { Effect, playEffect } from "../effects";
import { ui, currentChannelId, currentChatCode } from "../ui";
import { enterVoice, loadAccess } from "./access";
import { expire } from "./auth";
import { parse, plainContent, remember, rememberRefs, stamp, toLive, toQuote, toUser, voiceUsers } from "./convert";
import { loadDms, noteDmRead, onPrivate, refreshDm } from "./dm";
import { applyReaction, onUpdated, removeMessage } from "./edit";
import { append, refreshLatest } from "./history";
import { dropSubscription, ensureMembers, invalidateMembers, onUserList, patchMember } from "./members";
import { activate, activated, refreshGuild, reloadChannels, visited } from "./nav";
import { channelById, dmKey, guildOf, isDmKey, report, session, setSession, st } from "./state";
import { noteRead, noteUnread, syncUnread, windowActive } from "./unread";

let unlisten: UnlistenFn[] = [];

export function stopListeners() {
  unlisten.forEach((f) => f());
  unlisten = [];
  activated.clear();
  visited.clear();
}

let syncGen = 0;

let syncPending = false;

export async function resync(quiet = false) {
  const my = ++syncGen;
  const gen = st.account;
  syncPending = true;
  activated.clear();
  setSession("gateway", "syncing");
  const activations = Promise.all([...visited].map((id) => activate(id)));
  const accesses = Promise.all([...visited].map((id) => loadAccess(id, true)));
  for (const g of session.guilds) if (g.loaded) void reloadChannels(g.id);
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
  if (failed) diag(`补偿完成，${failed} 项失败`, "warn");
  setSession({ gateway: "online", recovery: quiet && !failed ? null : failed ? "partial" : "rebuilt" });
}

export function reconnectNow() {
  if (session.gateway === "reconnecting") api.gatewayReconnect().catch(report);
}

export function networkBack() {
  reconnectNow();
  voiceNetworkBack();
}

export type GatewayStatus =
  | { state: "ready"; kind: "new" | "resumed" | "rebuilt" }
  | { state: "disconnected"; reason: string; retry_in_ms: number };

export const diag = (message: string, level: "info" | "warn" | "debug" = "info") => void api.clientLog(level, message).catch(() => {});

export function onGatewayStatus(st: GatewayStatus) {
  diag(st.state === "disconnected" ? `网关状态 disconnected，${st.retry_in_ms}ms 后重连` : `网关状态 ready ${st.kind}`);
  if (st.state === "disconnected") {
    syncGen++;
    setSession({ gateway: "reconnecting", gatewayInfo: { reason: st.reason, retryAt: Date.now() + st.retry_in_ms }, recovery: null });
    return;
  }
  setSession({ gatewayInfo: null });
  if (st.kind !== "new") voiceNetworkBack();
  if (st.kind === "resumed" && !syncPending) {
    setSession({ gateway: "online", recovery: "resumed" });
    return;
  }
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
    if (msg.type === Kind.FILE && parseCards(msg.content)?.length) msg.type = Kind.CARD;
    if (!isText(msg.type) && msg.type !== Kind.IMAGE) msg.attachment = toAttachment(extra.attachments);
    append(channelId, msg);
    if (author.id === session.me?.id) return;
    const guildId = String(extra.guild_id ?? "") || guildOf(channelId)?.id || "";
    noteUnread(channelId, guildId, createAt, 0);
  }
}

export function setPreview(channelId: string, text: string) {
  for (const [gi, g] of session.guilds.entries()) {
    for (const [i, grp] of g.groups.entries()) {
      const j = grp.channels.findIndex((c) => c.id === channelId);
      if (j >= 0) return void setSession("guilds", gi, "groups", i, "channels", j, "preview", text || undefined);
    }
  }
}

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

export function onLiveStatus(body: Raw) {
  const uid = String(body.user?.id ?? "");
  const cid = String(body.channel?.id ?? "");
  if (!uid || !cid) return;
  const live = toLive(body.user?.live_info);
  session.guilds.forEach((g, gi) => g.groups.forEach((grp, i) => grp.channels.forEach((ch, j) => {
    if (ch.id !== cid || ch.type !== "voice") return;
    const list = ch.users ?? [];
    if (!list.some((x) => x.id === uid)) return;
    setSession("guilds", gi, "groups", i, "channels", j, "users", list.map((x) => {
      if (x.id !== uid) return x;
      const { live: _, ...rest } = x;
      return live ? { ...rest, live } : rest;
    }));
  })));
}

export function liveOf(channelId: string, userId: string) {
  return channelById(channelId)?.channel.users?.find((x) => x.id === userId)?.live;
}

export function onSystem(c: Raw | null, target = "") {
  if (!c) return;
  const body = c.body ?? {};
  switch (c.type) {
    case "live_status_changed":
      onLiveStatus(body);
      break;
    case "updated_user_list": {
      const data = body.user_list_data?.data;
      if (Array.isArray(data)) onUserList(String(body.channel_id ?? ""), data);
      break;
    }
    case "move_to_channel": {
      const to = String(body.id ?? "");
      if (to && voiceActive() && voice.channelId !== to) void enterVoice(to);
      break;
    }
    case "joined_channel":
    case "exited_channel":
      onVoicePresence(body, c.type === "joined_channel");
      break;
    case "user_state_change": {
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
      const gid = String(body.guild_id ?? target);
      if (session.guilds.some((g) => g.id === gid && g.loaded)) void reloadChannels(gid);
      break;
    }
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
      const id = String(body.channel_id ?? "");
      if (!id) break;
      const at = Number(body.msg_time ?? body.time ?? 0);
      noteUnread(id, String(body.guild_id ?? ""), at, Math.max(0, Number(body.mention_count ?? 0)));
      break;
    }
    case "mark_as_batchread":
      for (const c of Array.isArray(body.channel) ? body.channel : []) if (c?.channel_id) noteRead(String(c.channel_id), Number(c.time ?? 0));
      for (const c of Array.isArray(body.user_chat) ? body.user_chat : []) noteDmRead(c);
      break;
    case "private_message_updated": {
      const code = String(body.code ?? "");
      const id = String(body.rong_id ?? body.msg_id ?? "");
      if (!code || !id) break;
      onUpdated(dmKey(code), id, body);
      const list = session.messages[dmKey(code)];
      const i = session.dms.findIndex((c) => c.code === code);
      if (i >= 0 && list?.[list.length - 1]?.id === id) setSession("dms", i, "preview", plainContent(Number(body.type ?? Kind.KMARKDOWN), String(body.content ?? "")));
      break;
    }
    case "private_message_deleted": {
      const code = String(body.code ?? "");
      const id = String(body.rong_id ?? body.msg_id ?? "");
      if (code && id) removeMessage(dmKey(code), id);
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
      if (i >= 0) setSession("guilds", i, { name: body.name, icon: body.icon || undefined, ...(body.live_count != null && { liveCount: Number(body.live_count) || 0 }) });
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

export function dropSelfFromVoice(channelId: string) {
  const uid = session.me?.id;
  if (uid && channelId) onVoicePresence({ id: uid, channel: { id: channelId } }, false);
}
