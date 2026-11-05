import { produce, reconcile } from "solid-js/store";
import { api, type UnreadDto } from "../../lib/api";
import type { Unread } from "../types";
import { ui, currentChannelId } from "../ui";
import { readDm } from "./dm";
import { SEEN_LIMIT } from "./gateway";
import { denied } from "./access";
import { guildOfAny, report, session, setSession, st } from "./state";

// 未读

// 每个频道已知最新未读消息时间。跨端已读的时间早于它时不清除未读。
export const latestAt = new Map<string, number>();
// 每个频道已确认的已读时间（本端标记成功或跨端已读事件）。
export const readUpTo = new Map<string, number>();

// 记录一条未读。当前频道正被阅读时改为标记已读。
export function noteUnread(channelId: string, guildId: string, at: number, mentions: number) {
  if (denied(channelId)) return;
  const cur = session.unread[channelId];
  // 已读时间之前的事件（重复下发或续传补发）不再恢复未读。
  if (at && at <= Math.max(cur?.readAt ?? 0, readUpTo.get(channelId) ?? 0)) return;
  // 提及计数是增量：同一频道同一消息时间的提及事件只计一次。
  if (mentions > 0 && !firstMention(`${channelId}:${at}`)) mentions = 0;
  latestAt.set(channelId, Math.max(latestAt.get(channelId) ?? 0, at));
  unreadSync?.touched.add(channelId);
  // 先记入未读，正在阅读时再标记已读；标记成功后由 noteRead 清除，失败则保留。
  setSession("unread", channelId, {
    guildId: guildId || cur?.guildId || guildOfAny(channelId)?.id || "",
    unread: true, mentions: (cur?.mentions ?? 0) + mentions, readAt: cur?.readAt ?? 0,
  });
  if (channelId === currentChannelId() && reading()) readCurrent();
}

// 已计入的提及事件键。
export const mentionSeen = new Set<string>();
export function firstMention(key: string): boolean {
  if (mentionSeen.has(key)) return false;
  mentionSeen.add(key);
  if (mentionSeen.size > SEEN_LIMIT) mentionSeen.delete(mentionSeen.values().next().value!);
  return true;
}

// 已读到 time：未读消息都不晚于 time 时清除；有更晚的消息则保留未读。
export function noteRead(channelId: string, time: number) {
  readUpTo.set(channelId, Math.max(readUpTo.get(channelId) ?? 0, time));
  const cur = session.unread[channelId];
  unreadSync?.read.set(channelId, Math.max(unreadSync.read.get(channelId) ?? 0, time));
  if (!cur) return;
  if ((latestAt.get(channelId) ?? 0) > time) return setSession("unread", channelId, "readAt", Math.max(cur.readAt, time));
  setSession("unread", produce((u: Record<string, Unread>) => void delete u[channelId]));
}

// 进行中的未读快照拉取。期间到达的未读与已读记下，快照返回后重新应用，避免旧快照覆盖新状态。
let unreadSync: { touched: Set<string>; read: Map<string, number>; gen: number } | null = null;

// 拉取全部服务器的未读快照并替换本地状态。登录与新会话补偿时调用。失败返回 false，本地状态保留。
export async function syncUnread(): Promise<boolean> {
  const gen = st.account;
  const mine = { touched: new Set<string>(), read: new Map<string, number>(), gen };
  unreadSync = mine;
  let list: UnreadDto[];
  try {
    list = await api.unread();
  } catch (e) {
    if (gen === st.account) report(e);
    if (unreadSync === mine) unreadSync = null;
    return false;
  }
  if (gen !== st.account || unreadSync !== mine) return gen === st.account;
  unreadSync = null;
  if (!Array.isArray(list)) return false;
  const next: Record<string, Unread> = {};
  for (const u of list) next[u.channelId] = { guildId: u.guildId, unread: u.unread, mentions: u.mentions, readAt: u.readAt };
  // 拉取期间新到的未读保留（提及数取较大者），拉取期间的已读覆盖快照。
  for (const id of mine.touched) {
    const local = session.unread[id];
    if (local) next[id] = { ...local, mentions: Math.max(local.mentions, next[id]?.mentions ?? 0) };
  }
  for (const [id, time] of mine.read) if (next[id] && (latestAt.get(id) ?? 0) <= time) delete next[id];
  for (const id of Object.keys(next)) if (denied(id)) delete next[id];
  setSession("unread", reconcile(next));
  readCurrent();
  return true;
}

// 服务器未读汇总：是否有未读与提及总数，由频道未读计算，未进入过的服务器同样有效。
export function guildUnread(guildId: string): { unread: boolean; mentions: number } {
  let unread = false;
  let mentions = 0;
  for (const u of Object.values(session.unread)) {
    if (u.guildId !== guildId) continue;
    unread ||= u.unread;
    mentions += u.mentions;
  }
  return { unread, mentions };
}

// 窗口是否可见且有焦点。无 DOM（测试）时视为可见。
export function windowActive(): boolean {
  if (typeof document === "undefined") return true;
  return document.visibilityState === "visible" && document.hasFocus();
}

// 用户正在阅读当前频道最新消息：窗口可见有焦点、未打开设置页、列表位于底部。
export const reading = () => windowActive() && !ui.settings && ui.atBottom;

// 正在标记已读的频道及其目标时间。
export const marking = new Map<string, number>();

// 当前频道满足阅读条件时，以最后一条已送达消息标记已读。请求成功后才清除本地未读，失败时保留以便下次重试。
export function readCurrent() {
  if (ui.target.kind === "dm") return readDm(ui.target.chatId);
  const channelId = currentChannelId();
  if (!channelId || !reading() || session.history[channelId]?.newer) return;
  const last = [...(session.messages[channelId] ?? [])].reverse().find((m) => !m.id.startsWith("local-"));
  if (!last) return;
  const cur = session.unread[channelId];
  if (!cur && (latestAt.get(channelId) ?? 0) <= (readUpTo.get(channelId) ?? 0)) return;
  if ((marking.get(channelId) ?? -1) >= last.createAt) return;
  marking.set(channelId, last.createAt);
  const gen = st.account;
  api.markRead(channelId, last.rongId ?? last.id, last.createAt).then(
    () => {
      if (gen !== st.account) return;
      if (marking.get(channelId) === last.createAt) marking.delete(channelId);
      noteRead(channelId, last.createAt);
    },
    () => {
      if (gen === st.account && marking.get(channelId) === last.createAt) marking.delete(channelId);
    },
  );
}

export function resetUnread() {
  unreadSync = null;
  marking.clear();
  latestAt.clear();
  readUpTo.clear();
  mentionSeen.clear();
}
