import { api } from "../../lib/api";
import type { History, Message } from "../types";
import { currentChannelId } from "../ui";
import { toMessage } from "./convert";
import { denied } from "./access";
import { isDmKey, report, session, setSession, st } from "./state";

// 列表代次：跳转替换列表后递增，替换前发出的上翻或补拉结果作废，避免拼出缺口。
export const listGen = new Map<string, number>();
export const genOf = (channelId: string) => listGen.get(channelId) ?? 0;
// 跳转片段期间到达的实时消息，回到最新时合并。
export const held = new Map<string, Message[]>();
// 每个频道最近一次跳转的序号，迟到的旧跳转结果丢弃。
export const jumps = new Map<string, number>();
let focusSeq = 0;

// 正在拉取历史的频道。期间收到或发出的消息先暂存，结果返回后合并，避免被覆盖。
export const loading = new Map<string, Message[]>();

// 按 ID 去重后按时间排序（稳定排序，相同时间保持原顺序）。
export function mergeMessages(...lists: Message[][]): Message[] {
  const seen = new Set<string>();
  const out: Message[] = [];
  for (const l of lists) for (const m of l) if (!seen.has(m.id)) (seen.add(m.id), out.push(m));
  return out.sort((a, b) => a.createAt - b.createAt);
}

// 已删除的消息 ID（按频道）。删除前发出、删除后返回的历史页不能把它带回来。
export const deleted = new Map<string, Set<string>>();
// 进行中的历史拉取期间对消息的修改（编辑、回应、进行中标记），结果返回后重放到拉取到的副本上。
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

// 过滤已删除的消息，并把拉取期间的修改重放到新副本上。
export function settleFetched(channelId: string, list: Message[], log: PatchLog = []): Message[] {
  const gone = deleted.get(channelId);
  const out = gone ? list.filter((m) => !gone.has(m.id)) : list;
  for (const [id, f] of log) for (const m of out) if (m.id === id) f(m);
  return out;
}

// 同一消息同时存在于已有列表与服务端结果时，以服务端的内容、编辑时间、提及与回应为准；
// 本端进行中的状态保留（编辑进行中时保留本端新内容），已标记失效的引用保留。
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

// 正在进行的首次加载，重复调用复用同一请求，避免替换暂存区。
export const firstLoads = new Map<string, Promise<void>>();
// 补齐断档时最多回溯的页数；超过则改用最新一页重建列表，不留下看不见的缺口。
export const MAX_GAP_PAGES = 10;

// 拉取最新一页。首次进入时建立列表；已有列表时（如网关新会话）向前补齐到与已有消息衔接为止。
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
    // 等待期间失去查看权限：丢弃结果，不把已移除的内容带回。
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

// 已有列表时补拉：从最新页向前翻，直到与已有消息重叠或到达开头。不修改上翻的 loading / failed 状态。
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
  // 补拉从最新页开始，结果已覆盖跳转片段之后的消息。
  if (session.history[channelId]?.newer) {
    patchHistory(channelId, { newer: false });
    got.push(...(held.get(channelId) ?? []));
    held.delete(channelId);
  }
  if (joined || known.size === 0) {
    setSession("messages", channelId, (l) => mergeMessages(freshen(l, got), got));
  } else if (end) {
    // 翻到开头仍未衔接：服务端已无旧消息（例如被删除），以服务端为准；拉取期间新到的实时消息与回执保留。
    setSession("messages", channelId, (l) => mergeMessages(got, l.filter((x) => !known.has(x.id))));
    patchHistory(channelId, { end: true });
  } else {
    // 断档超过上限：丢弃已加载的旧消息，从最新处重新开始，上翻可继续加载。
    setSession("messages", channelId, (l) => mergeMessages(got, l.filter((x) => !known.has(x.id))));
    patchHistory(channelId, { end: false, failed: false });
  }
  return true;
}

// 以当前最早一条消息为游标拉取更早一页。结果只前插去重，不影响期间到达的实时消息与发送回执。
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

// 跳转片段之后的下一页。到达最新后合并暂存的实时消息并恢复正常跟随。
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

// 放弃跳转片段，回到最新消息。
export async function jumpLatest(channelId: string) {
  if (!session.history[channelId]?.newer) return;
  listGen.set(channelId, genOf(channelId) + 1);
  jumps.set(channelId, (jumps.get(channelId) ?? 0) + 1);
  held.delete(channelId);
  setSession("messages", channelId, undefined!);
  setSession("history", channelId, undefined!);
  await loadHistory(channelId);
}

// 跳转结果，供界面提示。
export type JumpResult = "shown" | "gone" | "elsewhere" | "failed" | "stale";

export function markQuoteGone(channelId: string, id: string) {
  const list = session.messages[channelId];
  if (!list || !id) return;
  list.forEach((m, i) => {
    if (m.quote?.id === id && !m.quote.gone) setSession("messages", channelId, i, "quote", "gone", true);
  });
}

// 跳转到被引用的消息：已加载则直接定位；否则确认消息存在后，加载其前后各一页替换列表。
// 消息已删除、ID 无效与无权限服务端无法区分，统一提示。切换频道、退出登录或再次跳转后，迟到结果丢弃。
export async function jumpTo(channelId: string, id: string | undefined): Promise<JumpResult> {
  const focus = () => {
    setSession("focus", { channelId, id: id!, seq: ++focusSeq });
    return "shown" as const;
  };
  if (!id) {
    setSession("error", "无法定位原消息");
    return "failed";
  }
  // 每次跳转都使更早的进行中跳转失效，包括目标已加载的情况。
  const seq = (jumps.get(channelId) ?? 0) + 1;
  jumps.set(channelId, seq);
  if (session.messages[channelId]?.some((m) => m.id === id)) return focus();
  // 私信没有按消息定位的接口，只能跳转到已加载的消息。
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
    // after 不含游标本身：先取更新的一页，再以其首条为游标取更早一页（含目标）。目标即最新消息时直接取最新页。
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
    // 合并后须仍是一个连续区间：片段与列表开头或末尾重叠，或两者都延伸到最新消息。否则以片段替换列表。
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
  // 跳转片段之后还有未加载的消息，直接追加会留下缺口：暂存，回到最新时合并。
  if (session.history[channelId]?.newer) {
    const h = held.get(channelId) ?? [];
    if (!h.some((x) => x.id === m.id)) h.push(m);
    held.set(channelId, h);
    return;
  }
  setSession("messages", channelId, (l) => [...l, m]);
}

// 退出登录时清空，listGen 与 newerLoads 保留。
export function resetHistory() {
  loading.clear();
  deleted.clear();
  fetchLogs.clear();
  firstLoads.clear();
  held.clear();
  jumps.clear();
}
