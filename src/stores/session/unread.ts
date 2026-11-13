import { produce, reconcile } from "solid-js/store";
import { api, type UnreadDto } from "../../lib/api";
import type { Unread } from "../types";
import { ui, currentChannelId } from "../ui";
import { readDm } from "./dm";
import { SEEN_LIMIT } from "./gateway";
import { denied } from "./access";
import { guildOfAny, report, session, setSession, st } from "./state";

export const latestAt = new Map<string, number>();
export const readUpTo = new Map<string, number>();

export function noteUnread(channelId: string, guildId: string, at: number, mentions: number) {
  if (denied(channelId)) return;
  const cur = session.unread[channelId];
  if (at && at <= Math.max(cur?.readAt ?? 0, readUpTo.get(channelId) ?? 0)) return;
  if (mentions > 0 && !firstMention(`${channelId}:${at}`)) mentions = 0;
  latestAt.set(channelId, Math.max(latestAt.get(channelId) ?? 0, at));
  unreadSync?.touched.add(channelId);
  setSession("unread", channelId, {
    guildId: guildId || cur?.guildId || guildOfAny(channelId)?.id || "",
    unread: true, mentions: (cur?.mentions ?? 0) + mentions, readAt: cur?.readAt ?? 0,
  });
  if (channelId === currentChannelId() && reading()) readCurrent();
}

export const mentionSeen = new Set<string>();
export function firstMention(key: string): boolean {
  if (mentionSeen.has(key)) return false;
  mentionSeen.add(key);
  if (mentionSeen.size > SEEN_LIMIT) mentionSeen.delete(mentionSeen.values().next().value!);
  return true;
}

export function noteRead(channelId: string, time: number) {
  readUpTo.set(channelId, Math.max(readUpTo.get(channelId) ?? 0, time));
  const cur = session.unread[channelId];
  unreadSync?.read.set(channelId, Math.max(unreadSync.read.get(channelId) ?? 0, time));
  if (!cur) return;
  if ((latestAt.get(channelId) ?? 0) > time) return setSession("unread", channelId, "readAt", Math.max(cur.readAt, time));
  setSession("unread", produce((u: Record<string, Unread>) => void delete u[channelId]));
}

let unreadSync: { touched: Set<string>; read: Map<string, number>; gen: number } | null = null;

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

export function windowActive(): boolean {
  if (typeof document === "undefined") return true;
  return document.visibilityState === "visible" && document.hasFocus();
}

export const reading = () => windowActive() && !ui.settings && ui.atBottom;

export const marking = new Map<string, number>();

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
