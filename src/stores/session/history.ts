import { api } from "../../lib/api";
import type { History, Message } from "../types";
import { currentChannelId } from "../ui";
import { toMessage } from "./convert";
import { denied } from "./access";
import { isDmKey, report, session, setSession, st } from "./state";

export const listGen = new Map<string, number>();
export const genOf = (channelId: string) => listGen.get(channelId) ?? 0;
export const held = new Map<string, Message[]>();
export const jumps = new Map<string, number>();
let focusSeq = 0;

export const loading = new Map<string, Message[]>();

export function mergeMessages(...lists: Message[][]): Message[] {
  const seen = new Set<string>();
  const out: Message[] = [];
  for (const l of lists) for (const m of l) if (!seen.has(m.id)) (seen.add(m.id), out.push(m));
  return out.sort((a, b) => a.createAt - b.createAt);
}

export const deleted = new Map<string, Set<string>>();
export type PatchLog = Array<[string, (m: Message) => void]>;
export const fetchLogs = new Map<string, Set<PatchLog>>();

export function trackFetch(channelId: string): PatchLog {
  const log: PatchLog = [];
  const set = fetchLogs.get(channelId) ?? new Set();
  set.add(log);
  fetchLogs.set(channelId, set);
  return log;
}

export function untrackFetch(channelId: string, log: PatchLog) {
  const set = fetchLogs.get(channelId);
  if (!set) return;
  set.delete(log);
  if (!set.size) fetchLogs.delete(channelId);
}

export function settleFetched(channelId: string, list: Message[], log: PatchLog = []): Message[] {
  const gone = deleted.get(channelId);
  const out = gone ? list.filter((m) => !gone.has(m.id)) : list;
  for (const [id, f] of log) for (const m of out) if (m.id === id) f(m);
  return out;
}

export function freshen(local: Message[], fetched: Message[]): Message[] {
  const by = new Map(fetched.map((m) => [m.id, m]));
  return local.map((m) => {
    const f = by.get(m.id);
    if (!f) return m;
    return {
      ...f,
      pending: m.pending,
      content: m.pending === "editing" ? m.content : f.content,
      quote: m.quote?.gone ? m.quote : f.quote,
    };
  });
}

export function patchHistory(channelId: string, patch: Partial<History>) {
  const cur = session.history[channelId] ?? { end: false, loading: false, failed: false };
  setSession("history", channelId, { ...cur, ...patch });
}

export const firstLoads = new Map<string, Promise<void>>();
export const MAX_GAP_PAGES = 10;

export function loadHistory(channelId: string): Promise<void> {
  if (session.messages[channelId]) return refreshLatest(channelId).then(() => {});
  const running = firstLoads.get(channelId);
  if (running) return running;
  const p = firstLoad(channelId).finally(() => firstLoads.delete(channelId));
  firstLoads.set(channelId, p);
  return p;
}

export async function firstLoad(channelId: string) {
  const gen = st.account;
  loading.set(channelId, []);
  const log = trackFetch(channelId);
  patchHistory(channelId, { failed: false, loading: true });
  try {
    const page = await api.messages(channelId);
    if (gen !== st.account || denied(channelId)) return;
    const fetched = settleFetched(channelId, page.items.map(toMessage), log);
    const cur = session.messages[channelId] ?? [];
    setSession("messages", channelId, mergeMessages(freshen(cur, fetched), loading.get(channelId) ?? [], fetched));
    patchHistory(channelId, { end: page.end, loading: false });
  } catch (e) {
    if (gen !== st.account) return;
    patchHistory(channelId, { failed: true, loading: false });
    if (currentChannelId() === channelId) report(e);
  } finally {
    if (gen === st.account) {
      loading.delete(channelId);
      untrackFetch(channelId, log);
    }
  }
}

export async function refreshLatest(channelId: string): Promise<boolean> {
  const gen = st.account;
  const lg = genOf(channelId);
  const known = new Set((session.messages[channelId] ?? []).map((m) => m.id));
  const got: Message[] = [];
  let end = false;
  let joined = false;
  const log = trackFetch(channelId);
  try {
    let cursor: string | undefined;
    for (let i = 0; i < MAX_GAP_PAGES; i++) {
      const page = await api.messages(channelId, cursor);
      if (gen !== st.account || !session.messages[channelId] || lg !== genOf(channelId)) return true;
      const fetched = page.items.map(toMessage);
      got.unshift(...fetched);
      end = page.end;
      joined = fetched.some((m) => known.has(m.id));
      if (joined || end || !fetched.length) break;
      cursor = fetched[0].id;
    }
  } catch (e) {
    if (gen === st.account && currentChannelId() === channelId) report(e);
    return false;
  } finally {
    if (gen === st.account) untrackFetch(channelId, log);
  }
  if (gen !== st.account || !session.messages[channelId] || lg !== genOf(channelId) || denied(channelId)) return true;
  got.splice(0, got.length, ...settleFetched(channelId, got.slice(), log));
  if (session.history[channelId]?.newer) {
    patchHistory(channelId, { newer: false });
    got.push(...(held.get(channelId) ?? []));
    held.delete(channelId);
  }
  if (joined || known.size === 0) {
    setSession("messages", channelId, (l) => mergeMessages(freshen(l, got), got));
  } else if (end) {
    setSession("messages", channelId, (l) => mergeMessages(got, l.filter((x) => !known.has(x.id))));
    patchHistory(channelId, { end: true });
  } else {
    setSession("messages", channelId, (l) => mergeMessages(got, l.filter((x) => !known.has(x.id))));
    patchHistory(channelId, { end: false, failed: false });
  }
  return true;
}

export async function loadOlder(channelId: string) {
  const h = session.history[channelId];
  const list = session.messages[channelId];
  if (!h || !list || h.end || h.loading) return;
  const cursor = list[0]?.id;
  if (!cursor) return patchHistory(channelId, { end: true });
  const gen = st.account;
  const lg = genOf(channelId);
  patchHistory(channelId, { loading: true, failed: false });
  try {
    const page = await api.messages(channelId, cursor);
    if (gen !== st.account || !session.messages[channelId]) return;
    if (lg !== genOf(channelId)) return patchHistory(channelId, { loading: false });
    const fetched = settleFetched(channelId, page.items.map(toMessage));
    const fresh = fetched.filter((m) => !session.messages[channelId].some((x) => x.id === m.id));
    if (fresh.length) setSession("messages", channelId, (l) => mergeMessages(fresh, l));
    patchHistory(channelId, { end: page.end || fresh.length === 0, loading: false });
  } catch {
    if (gen === st.account) patchHistory(channelId, { failed: lg === genOf(channelId), loading: false });
  }
}

export const newerLoads = new Set<string>();
export async function loadNewer(channelId: string) {
  const list = session.messages[channelId];
  if (!session.history[channelId]?.newer || !list?.length || newerLoads.has(channelId)) return;
  const gen = st.account;
  const lg = genOf(channelId);
  newerLoads.add(channelId);
  try {
    const page = await api.messagesAfter(channelId, list[list.length - 1].id);
    if (gen !== st.account || lg !== genOf(channelId) || !session.messages[channelId]) return;
    const fetched = settleFetched(channelId, page.items.map(toMessage));
    if (page.end || !fetched.length) {
      setSession("messages", channelId, (l) => mergeMessages(l, fetched, held.get(channelId) ?? []));
      held.delete(channelId);
      patchHistory(channelId, { newer: false });
    } else setSession("messages", channelId, (l) => mergeMessages(l, fetched));
  } catch (e) {
    if (gen === st.account && currentChannelId() === channelId) report(e);
  } finally {
    if (gen === st.account) newerLoads.delete(channelId);
  }
}

export async function jumpLatest(channelId: string) {
  if (!session.history[channelId]?.newer) return;
  listGen.set(channelId, genOf(channelId) + 1);
  jumps.set(channelId, (jumps.get(channelId) ?? 0) + 1);
  held.delete(channelId);
  setSession("messages", channelId, undefined!);
  setSession("history", channelId, undefined!);
  await loadHistory(channelId);
}

export type JumpResult = "shown" | "gone" | "elsewhere" | "failed" | "stale";

export function markQuoteGone(channelId: string, id: string) {
  const list = session.messages[channelId];
  if (!list || !id) return;
  list.forEach((m, i) => {
    if (m.quote?.id === id && !m.quote.gone) setSession("messages", channelId, i, "quote", "gone", true);
  });
}

export async function jumpTo(channelId: string, id: string | undefined): Promise<JumpResult> {
  const focus = () => {
    setSession("focus", { channelId, id: id!, seq: ++focusSeq });
    return "shown" as const;
  };
  if (!id) {
    setSession("error", "无法定位原消息");
    return "failed";
  }
  const seq = (jumps.get(channelId) ?? 0) + 1;
  jumps.set(channelId, seq);
  if (session.messages[channelId]?.some((m) => m.id === id)) return focus();
  if (isDmKey(channelId)) {
    setSession("error", "原消息不在已加载的私信记录中，请向上翻页后再试");
    return "failed";
  }
  const gen = st.account;
  const stale = () => gen !== st.account || jumps.get(channelId) !== seq || currentChannelId() !== channelId || !session.messages[channelId];
  try {
    const loc = await api.locateMessage(id);
    if (stale()) return "stale";
    if (!loc) {
      markQuoteGone(channelId, id);
      setSession("error", "原消息已被删除，或你没有权限查看");
      return "gone";
    }
    if (loc.channelId !== channelId) {
      setSession("error", "原消息不在当前频道");
      return "elsewhere";
    }
    if (session.messages[channelId]?.some((m) => m.id === id)) return focus();
    const after = await api.messagesAfter(channelId, id);
    if (stale()) return "stale";
    const newer = after.items.map(toMessage);
    const before = await api.messages(channelId, newer[0]?.id);
    if (stale()) return "stale";
    const older = before.items.map(toMessage);
    const fragment = settleFetched(channelId, mergeMessages(older, newer));
    if (!fragment.some((m) => m.id === id)) {
      markQuoteGone(channelId, id);
      setSession("error", "原消息已被删除，或你没有权限查看");
      return "gone";
    }
    const list = session.messages[channelId];
    const h = session.history[channelId] ?? { end: false, loading: false, failed: false };
    const reachesLatest = !newer.length || after.end;
    const has = (m: Message | undefined) => Boolean(m && fragment.some((x) => x.id === m.id));
    const joinsHead = has(list[0]);
    const joinsTail = has(list[list.length - 1]);
    const bothLatest = reachesLatest && !h.newer;
    listGen.set(channelId, genOf(channelId) + 1);
    if (list.length && (joinsHead || joinsTail || bothLatest)) {
      const startsEarlier = fragment[0].createAt <= list[0].createAt;
      const latest = reachesLatest || (!joinsTail && !h.newer);
      setSession("messages", channelId, (l) => mergeMessages(fragment, l, latest ? held.get(channelId) ?? [] : []));
      if (latest) held.delete(channelId);
      setSession("history", channelId, { end: startsEarlier ? before.end : h.end, loading: false, failed: false, newer: !latest });
    } else {
      setSession("messages", channelId, fragment);
      setSession("history", channelId, { end: before.end, loading: false, failed: false, newer: !reachesLatest });
      if (reachesLatest) {
        setSession("messages", channelId, (l) => mergeMessages(l, held.get(channelId) ?? []));
        held.delete(channelId);
      }
    }
    return focus();
  } catch (e) {
    if (stale()) return "stale";
    report(e);
    return "failed";
  }
}

export function append(channelId: string, m: Message) {
  if (denied(channelId)) return;
  const buffer = loading.get(channelId);
  if (buffer && !buffer.some((x) => x.id === m.id)) buffer.push(m);
  if (!session.messages[channelId]) return;
  if (session.messages[channelId].some((x) => x.id === m.id)) return;
  if (session.history[channelId]?.newer) {
    const h = held.get(channelId) ?? [];
    if (!h.some((x) => x.id === m.id)) h.push(m);
    held.set(channelId, h);
    return;
  }
  setSession("messages", channelId, (l) => [...l, m]);
}

export function resetHistory() {
  loading.clear();
  deleted.clear();
  fetchLogs.clear();
  firstLoads.clear();
  held.clear();
  jumps.clear();
}
