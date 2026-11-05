import { api, type MemberDto, type Raw } from "../../lib/api";
import { ui } from "../ui";
import { session, sessionUp, setSession, st } from "./state";

// 成员

export const MEMBER_REFRESH_PAGES = 4;
export const memberKey = (guildId: string, channelId: string) => `${guildId}/${channelId}`;

// 在线状态订阅：服务端只对订阅区间推送 updated_user_list（无订阅时没有上下线推送）。
// 记录当前订阅的频道，切换或关闭时以 -1,-1 取消，新网关会话后重新订阅。
let subscribed: string | null = null;
export function subscribeMembers() {
  const m = session.members;
  const want = m && sessionUp() && m.items.length ? m.channelId : null;
  const gen = st.account;
  if (subscribed && subscribed !== want) {
    const old = subscribed;
    subscribed = null;
    api.subscribeMembers(old, -1, -1).catch(() => {});
  }
  if (!m || !want) return;
  subscribed = want;
  api.subscribeMembers(want, 0, m.items.length - 1).catch(() => {
    if (gen === st.account && subscribed === want) subscribed = null;
  });
}

// 关闭成员栏：取消订阅并丢弃列表，下次打开重新拉取。
export function closeMembers() {
  memberSeq++;
  setSession("members", null);
  subscribeMembers();
}

// 推送的成员资料（与 user-list 条目同形）转为局部更新。
export function memberPatch(r: Raw): Partial<MemberDto> {
  const p: Partial<MemberDto> = {};
  if (typeof r.online === "boolean") p.online = r.online;
  if (typeof r.nickname === "string") p.nickname = r.nickname;
  if (typeof r.username === "string") p.username = r.username;
  if (typeof r.avatar === "string") p.avatar = r.avatar;
  if (typeof r.color === "number") p.color = r.color;
  if (Array.isArray(r.roles)) p.roles = r.roles.map(Number);
  return p;
}

// updated_user_list：订阅区间内成员的最新资料。按 ID 合并到已加载的列表；在线人数随之调整，按在线在前重排。
export function onUserList(channelId: string, list: Raw[]) {
  const m = session.members;
  if (!m || m.channelId !== channelId || !list.length) return;
  let delta = 0;
  const byId = new Map(list.filter((r) => r?.id != null).map((r) => [String(r.id), r]));
  const items = m.items.map((x) => {
    const r = byId.get(x.id);
    if (!r) return x;
    const next = { ...x, ...memberPatch(r) };
    if (next.online !== x.online) delta += next.online ? 1 : -1;
    return next;
  });
  const sorted = [...items.filter((x) => x.online), ...items.filter((x) => !x.online)];
  setSession("members", { items: sorted, online: Math.max(0, m.online + delta), offline: Math.max(0, m.offline - delta) });
}

// 成员请求代次：每次刷新递增，翻页与旧刷新的结果在代次变化后丢弃。
let memberSeq = 0;

// 打开或切换到当前频道的成员列表。与当前 key 相同时不重复拉取。
export function ensureMembers(force = false): Promise<void> {
  const t = ui.target;
  if (t.kind !== "channel") {
    if (session.members) closeMembers();
    return Promise.resolve();
  }
  const key = memberKey(t.guildId, t.channelId);
  const cur = session.members;
  // 同一列表已加载、加载中或已失败（失败由用户点重试或事件强制刷新，不自动重试）时不重复拉取。
  if (!force && cur?.key === key) return Promise.resolve();
  // 切换频道：丢弃旧列表，旧请求的结果按 key 丢弃。
  if (cur?.key !== key) memberSeq++;
  if (cur?.key !== key) setSession("members", { key, guildId: t.guildId, channelId: t.channelId, items: [], page: 0, pageTotal: 1, total: 0, online: 0, offline: 0, loading: false, failed: false });
  return refreshMembers();
}

// 重新拉取已加载的前几页（在线状态无推送，靠此刷新），结果整体替换。
// 新的刷新取代进行中的刷新与翻页。
export async function refreshMembers(): Promise<void> {
  const cur = session.members;
  if (!cur) return;
  const { key, guildId, channelId } = cur;
  const pages = Math.min(Math.max(cur.page, 1), MEMBER_REFRESH_PAGES);
  const gen = st.account;
  const my = ++memberSeq;
  const stale = () => gen !== st.account || my !== memberSeq || session.members?.key !== key;
  setSession("members", { loading: true, failed: false });
  const items: MemberDto[] = [];
  let last;
  try {
    for (let p = 1; p <= pages; p++) {
      last = await api.members(guildId, channelId, p);
      if (stale()) return;
      items.push(...last.items);
      if (p >= last.pageTotal || !last.items.length) break;
    }
  } catch {
    if (stale()) return;
    setSession("members", { loading: false, failed: true });
    return;
  }
  if (!last) return;
  setSession("members", {
    items: dedupeMembers(items), page: Math.min(pages, Math.max(last.pageTotal, 1)), pageTotal: last.pageTotal,
    total: last.total, online: last.online, offline: last.offline, loading: false, failed: false,
  });
  subscribeMembers();
}

export function dedupeMembers(list: MemberDto[]): MemberDto[] {
  const seen = new Set<string>();
  return list.filter((m) => !seen.has(m.id) && (seen.add(m.id), true));
}

// 加载下一页成员（滚动到底部时）。
export async function loadMoreMembers(): Promise<void> {
  const cur = session.members;
  if (!cur || cur.loading || cur.failed || cur.page >= cur.pageTotal) return;
  const { key, guildId, channelId } = cur;
  const next = cur.page + 1;
  const gen = st.account;
  const my = ++memberSeq;
  const stale = () => gen !== st.account || my !== memberSeq || session.members?.key !== key;
  setSession("members", "loading", true);
  try {
    const r = await api.members(guildId, channelId, next);
    if (stale()) return;
    setSession("members", (m) => m && {
      ...m, items: dedupeMembers([...m.items, ...r.items]), page: r.items.length ? next : m.pageTotal,
      pageTotal: r.pageTotal, total: r.total, online: r.online, offline: r.offline, loading: false,
    });
    subscribeMembers();
  } catch {
    if (!stale()) setSession("members", { loading: false, failed: true });
  }
}

// 成员资料局部更新（user_state_change）。只更新提供的字段。
export function patchMember(guildId: string, userId: string, patch: Raw) {
  const m = session.members;
  if (!m || m.guildId !== guildId) return;
  const i = m.items.findIndex((x) => x.id === userId);
  if (i < 0) return;
  const p: Partial<MemberDto> = {};
  if (typeof patch.nickname === "string") p.nickname = patch.nickname;
  if (typeof patch.username === "string") p.username = patch.username;
  if (typeof patch.avatar === "string") p.avatar = patch.avatar;
  if (typeof patch.online === "boolean") p.online = patch.online;
  if (Array.isArray(patch.roles)) p.roles = patch.roles.map(Number);
  if (typeof patch.color === "number") p.color = patch.color;
  setSession("members", "items", i, p);
}

// 服务器的成员或角色发生变化：当前列表属于该服务器时重新拉取。
export function invalidateMembers(guildId: string) {
  if (session.members?.guildId === guildId) void ensureMembers(true);
}

// 订阅属于网关会话，退出登录或新会话后视为未订阅。
export function dropSubscription() {
  subscribed = null;
}
