import { createStore, produce, reconcile } from "solid-js/store";
import { api, type Raw } from "../../lib/api";
import { has, Perm } from "../../lib/perms";
import type { Message } from "../types";
import { isText } from "../../lib/content";
import { permsOf } from "./access";
import { rememberRefs } from "./convert";
import { deleted, fetchLogs, held, loading, markQuoteGone } from "./history";
import { guildOf, isDmKey, report, session, setSession, st } from "./state";

export function locateMsg(channelId: string, id: string): number {
  return session.messages[channelId]?.findIndex((m) => m.id === id) ?? -1;
}

export function patchMsg(channelId: string, id: string, f: (m: Message) => void) {
  const i = locateMsg(channelId, id);
  if (i >= 0) setSession("messages", channelId, i, produce(f));
  for (const m of held.get(channelId) ?? []) if (m.id === id) f(m);
  for (const log of fetchLogs.get(channelId) ?? []) log.push([id, f]);
}

export function removeMessage(channelId: string, id: string) {
  if (!channelId || !id) return;
  const gone = deleted.get(channelId) ?? new Set<string>();
  gone.add(id);
  deleted.set(channelId, gone);
  if (session.messages[channelId]) {
    if (locateMsg(channelId, id) >= 0) setSession("messages", channelId, (l) => l.filter((m) => m.id !== id));
    markQuoteGone(channelId, id);
  }
  const h = held.get(channelId);
  if (h) held.set(channelId, h.filter((m) => m.id !== id));
  const buf = loading.get(channelId);
  if (buf) loading.set(channelId, buf.filter((m) => m.id !== id));
}

export function onUpdated(channelId: string, id: string, body: Raw) {
  if (!channelId || !id) return;
  const at = Number(body.updated_at ?? 0) || Date.now();
  rememberRefs(body.kmarkdown);
  patchMsg(channelId, id, (m) => {
    m.content = String(body.content ?? "");
    m.updatedAt = at;
  });
}

export function applyReaction(channelId: string, msgId: string, emoji: { id: string; name: string }, user: string, add: boolean) {
  if (!channelId || !msgId) return;
  const mine = user === session.me?.id;
  patchMsg(channelId, msgId, (m) => {
    const list = m.reactions ?? [];
    let r = list.find((x) => x.id === emoji.id);
    if (!r) {
      if (!add) return;
      r = { id: emoji.id, name: emoji.name, count: 0, me: false, users: {} };
      list.push(r);
    }
    const known = r.users[user];
    const settled = known !== undefined ? known === add : mine ? r.me === add : false;
    r.users[user] = add;
    if (mine) r.me = add;
    if (!settled) r.count += add ? 1 : -1;
    m.reactions = list.filter((x) => x.count > 0);
  });
}

export function canEdit(channelId: string, m: Message): boolean {
  return Boolean(session.me) && m.author === session.me!.id && isText(m.type) && (isDmKey(channelId) || Boolean(guildOf(channelId)));
}

export function canDelete(channelId: string, m: Message): boolean {
  if (!session.me) return false;
  if (isDmKey(channelId)) return m.author === session.me.id;
  if (!guildOf(channelId)) return false;
  if (m.author === session.me.id) return true;
  const p = permsOf(channelId);
  return p !== null && has(p, Perm.MANAGE_MESSAGES);
}

export function canReact(channelId: string): boolean {
  if (isDmKey(channelId)) return Boolean(session.me);
  const p = permsOf(channelId);
  return Boolean(session.me) && p !== null && has(p, Perm.REACT);
}

export const findMsg = (channelId: string, id: string) => session.messages[channelId]?.find((m) => m.id === id);

export async function editMessage(channelId: string, id: string, content: string): Promise<boolean> {
  const m = findMsg(channelId, id);
  if (m && isDmKey(channelId) && m.author !== session.me?.id) {
    setSession("error", "私信中不能编辑对方的消息");
    return false;
  }
  if (!m || m.pending || !canEdit(channelId, m) || !content.trim()) return false;
  if (content === m.content) return true;
  const gen = st.account;
  const old = { content: m.content, updatedAt: m.updatedAt };
  patchMsg(channelId, id, (x) => {
    x.content = content;
    x.pending = "editing";
  });
  try {
    await (isDmKey(channelId) ? api.updateDmMessage(id, content) : api.updateMessage(id, content));
    if (gen !== st.account) return false;
    patchMsg(channelId, id, (x) => {
      x.pending = undefined;
      if (x.updatedAt === old.updatedAt) x.updatedAt = Date.now();
    });
    return true;
  } catch (e) {
    if (gen !== st.account) return false;
    patchMsg(channelId, id, (x) => {
      x.pending = undefined;
      if (x.updatedAt === old.updatedAt && x.content === content) x.content = old.content;
    });
    report(e);
    return false;
  }
}

export async function deleteMessage(channelId: string, id: string): Promise<boolean> {
  const m = findMsg(channelId, id);
  if (m && isDmKey(channelId) && m.author !== session.me?.id) {
    setSession("error", "私信中不能删除对方的消息");
    return false;
  }
  if (!m || m.pending || !canDelete(channelId, m)) return false;
  const gen = st.account;
  patchMsg(channelId, id, (x) => void (x.pending = "deleting"));
  try {
    await (isDmKey(channelId) ? api.deleteDmMessage(id) : api.deleteMessage(id));
    if (gen !== st.account) return false;
    removeMessage(channelId, id);
    return true;
  } catch (e) {
    if (gen !== st.account) return false;
    patchMsg(channelId, id, (x) => void (x.pending = undefined));
    report(e);
    return false;
  }
}

const [reactState, setReactState] = createStore<Record<string, "pending" | "failed" | undefined>>({});
const failTimers = new Map<string, ReturnType<typeof setTimeout>>();
const rkey = (channelId: string, msgId: string, emoji: string) => `${channelId}/${msgId}/${emoji}`;
export const isReacting = (channelId: string, msgId: string, emoji: string) => reactState[rkey(channelId, msgId, emoji)] === "pending";
export const reactFailed = (channelId: string, msgId: string, emoji: string) => reactState[rkey(channelId, msgId, emoji)] === "failed";

export async function toggleReaction(channelId: string, msgId: string, emoji: { id: string; name: string }, add?: boolean): Promise<boolean> {
  const me = session.me?.id;
  const m = findMsg(channelId, msgId);
  const key = rkey(channelId, msgId, emoji.id);
  if (!me || !m || m.pending === "deleting" || reactState[key] === "pending" || !canReact(channelId)) return false;
  const want = add ?? !m.reactions?.find((r) => r.id === emoji.id)?.me;
  const gen = st.account;
  clearTimeout(failTimers.get(key));
  failTimers.delete(key);
  setReactState(key, "pending");
  applyReaction(channelId, msgId, emoji, me, want);
  try {
    await (isDmKey(channelId) ? api.dmReaction(msgId, emoji.id, want) : api.reaction(msgId, emoji.id, want));
    if (gen !== st.account) return false;
    setReactState(key, undefined);
    return true;
  } catch (e) {
    if (gen !== st.account) return false;
    applyReaction(channelId, msgId, emoji, me, !want);
    setReactState(key, "failed");
    failTimers.set(key, setTimeout(() => {
      failTimers.delete(key);
      if (reactState[key] === "failed") setReactState(key, undefined);
    }, 4000));
    report(e);
    return false;
  }
}

export function resetEdit() {
  for (const t of failTimers.values()) clearTimeout(t);
  failTimers.clear();
  setReactState(reconcile({}));
}
