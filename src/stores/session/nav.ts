import { api } from "../../lib/api";
import { parseFolders } from "../../lib/guildFolders";
import { ui, setUi, NONE, DM_VIEW } from "../ui";
import { leaveHidden, loadAccess, refreshAccess, visible } from "./access";
import { toGroups } from "./convert";
import { loadDms } from "./dm";
import { loadHistory } from "./history";
import { channelById, guildOf, report, session, sessionUp, setSession, st } from "./state";
import { readCurrent } from "./unread";

// 读取服务器栏文件夹。失败时保留平铺列表，可重试；不发起任何修改分组的请求。
// 重试与登录时的加载可能重叠，只采用最后一次请求的结果。
let folderSeq = 0;
export async function loadFolders() {
  const gen = st.account;
  const seq = ++folderSeq;
  const mine = () => gen === st.account && seq === folderSeq;
  setSession("foldersFailed", false);
  try {
    const folders = parseFolders(await api.guildFolders());
    if (mine()) setSession("folders", folders);
  } catch {
    if (mine()) setSession("foldersFailed", true);
  }
}

export async function loadGuilds() {
  const gen = st.account;
  setSession("loadFailed", false);
  void loadFolders();
  try {
    const list = await api.guilds();
    if (gen !== st.account) return;
    setSession("guilds", list.map((g) => ({ id: String(g.id), name: g.name, icon: g.icon || undefined, groups: [] })));
  } catch (e) {
    if (gen !== st.account) return;
    setSession("loadFailed", true);
    report(e);
    return;
  }
  const first = session.guilds[0];
  if (first && !ui.view) await selectGuild(first.id);
}

export const activated = new Set<string>();

// 进入私信入口：不沿用旧频道，未加载会话列表时加载第一页。
export function openDm() {
  st.epoch++;
  setUi({ view: DM_VIEW, target: NONE });
  if (!session.dmList.loaded && !session.dmList.loading) void loadDms();
}

export async function selectGuild(id: string) {
  const my = ++st.epoch;
  setUi({ view: id, target: NONE });
  const idx = session.guilds.findIndex((g) => g.id === id);
  if (idx < 0) return;
  // 已有依据时节流刷新，不阻塞进入；没有依据时等待。
  const access = session.access[id]?.status === "ready" ? void refreshAccess(id) : loadAccess(id);
  // 已加载时在后台刷新：未激活期间的语音频道进出没有推送。
  if (session.guilds[idx].loaded) void reloadChannels(id);
  else {
    const gen = st.account;
    try {
      const groups = toGroups(await api.channels(id));
      const i = session.guilds.findIndex((g) => g.id === id);
      if (gen === st.account && i >= 0) setSession("guilds", i, { groups, loaded: true, failed: false });
    } catch (e) {
      const i = session.guilds.findIndex((g) => g.id === id);
      if (gen === st.account && i >= 0) setSession("guilds", i, "failed", true);
      if (my === st.epoch) report(e);
    }
  }
  // 等待权限依据，避免先选中随后被判定不可见的频道。失败时不选中频道，界面显示重试。
  await access;
  if (my !== st.epoch) return;
  activate(id);
  const texts = (session.guilds.find((g) => g.id === id)?.groups ?? []).flatMap((g) => g.channels).filter((c) => c.type === "text" && visible(c));
  const pick = texts.find((c) => c.id === ui.lastChannel[id]) ?? texts[0];
  if (pick) await selectChannel(pick.id);
}

// 进入过的服务器。新会话须全部重新激活，否则其频道不再推送。
export const visited = new Set<string>();

// 返回是否已激活（或无需激活）。失败时移出已激活集合，下次进入服务器或补偿时重试。
export function activate(id: string): Promise<boolean> {
  visited.add(id);
  if (!sessionUp() || activated.has(id)) return Promise.resolve(true);
  activated.add(id);
  const gen = st.account;
  return api.activateGuild(id).then(
    () => true,
    (e) => {
      activated.delete(id);
      if (gen === st.account) report(e);
      return false;
    },
  );
}

export async function selectChannel(id: string) {
  const guild = guildOf(id);
  if (!guild) return;
  const hit = channelById(id);
  if (!hit || !visible(hit.channel)) return;
  const my = ++st.epoch;
  setUi({ view: guild.id, target: { kind: "channel", guildId: guild.id, channelId: id } });
  setUi("lastChannel", guild.id, id);
  if (!session.messages[id]) await loadHistory(id);
  if (my === st.epoch) readCurrent();
}

// 重新拉取频道列表（频道或覆盖变化），不改变当前会话。并发调用只采用最后一次请求的结果。
const channelSeq = new Map<string, number>();
export async function reloadChannels(guildId: string) {
  const gen = st.account;
  const seq = (channelSeq.get(guildId) ?? 0) + 1;
  channelSeq.set(guildId, seq);
  try {
    const groups = toGroups(await api.channels(guildId));
    const i = session.guilds.findIndex((g) => g.id === guildId);
    if (gen === st.account && i >= 0 && channelSeq.get(guildId) === seq) {
      setSession("guilds", i, { groups, loaded: true, failed: false });
      leaveHidden();
    }
  } catch {
    // 保留旧列表，下次进入或补偿时再试。
  }
}

// 权限依据与频道覆盖一并刷新。短时间内的多次触发合并为一次。
const pendingRefresh = new Map<string, ReturnType<typeof setTimeout>>();
export const REFRESH_DELAY = 300;
export function refreshGuild(guildId: string) {
  if (pendingRefresh.has(guildId)) return;
  const gen = st.account;
  pendingRefresh.set(guildId, setTimeout(() => {
    pendingRefresh.delete(guildId);
    if (gen !== st.account) return;
    void loadAccess(guildId, true);
    if (session.guilds.some((g) => g.id === guildId && g.loaded)) void reloadChannels(guildId);
  }, REFRESH_DELAY));
}

export function resetNav() {
  for (const t of pendingRefresh.values()) clearTimeout(t);
  pendingRefresh.clear();
}

// 重试读取权限依据。成功且仍停留在该服务器、尚未选中会话时，按常规规则选中频道。
export async function retryAccess(guildId: string) {
  const my = st.epoch;
  if (!(await loadAccess(guildId, true))) return;
  if (my === st.epoch && ui.view === guildId && ui.target.kind === "none") await selectGuild(guildId);
}
