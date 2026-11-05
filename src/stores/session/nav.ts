import { api } from "../../lib/api";
import { ui, setUi, NONE, DM_VIEW } from "../ui";
import { leaveHidden, loadAccess, visible } from "./access";
import { toGroups } from "./convert";
import { loadDms } from "./dm";
import { loadHistory } from "./history";
import { channelById, guildOf, report, session, sessionUp, setSession, st } from "./state";
import { readCurrent } from "./unread";

export async function loadGuilds() {
  const gen = st.account;
  setSession("loadFailed", false);
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
  const access = session.access[id]?.status === "ready" ? undefined : loadAccess(id);
  if (!session.guilds[idx].loaded) {
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
  // 等待权限依据，避免先选中随后被判定不可见的频道。失败时仍按列表选择，发送入口保持禁用。
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
  if (hit && !visible(hit.channel)) return;
  const my = ++st.epoch;
  setUi({ view: guild.id, target: { kind: "channel", guildId: guild.id, channelId: id } });
  setUi("lastChannel", guild.id, id);
  if (!session.messages[id]) await loadHistory(id);
  if (my === st.epoch) readCurrent();
}

// 重新拉取频道列表（频道或覆盖变化），不改变当前会话。
export async function reloadChannels(guildId: string) {
  const gen = st.account;
  try {
    const groups = toGroups(await api.channels(guildId));
    const i = session.guilds.findIndex((g) => g.id === guildId);
    if (gen === st.account && i >= 0) {
      setSession("guilds", i, { groups, loaded: true, failed: false });
      leaveHidden();
    }
  } catch {
    // 保留旧列表，下次进入或补偿时再试。
  }
}
