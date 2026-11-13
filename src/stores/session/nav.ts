import { api } from "../../lib/api";
import { parseFolders } from "../../lib/guildFolders";
import { ui, setUi, NONE, DM_VIEW } from "../ui";
import { leaveHidden, loadAccess, refreshAccess, visible } from "./access";
import { toGroups } from "./convert";
import { loadDms } from "./dm";
import { loadHistory } from "./history";
import { channelById, guildOf, report, session, sessionUp, setSession, st } from "./state";
import { readCurrent } from "./unread";

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
    setSession("guilds", list.map((g) => ({ id: String(g.id), name: g.name, icon: g.icon || undefined, groups: [], liveCount: Number(g.live_count) || 0 })));
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
  const access = session.access[id]?.status === "ready" ? void refreshAccess(id) : loadAccess(id);
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
  await access;
  if (my !== st.epoch) return;
  activate(id);
  const texts = (session.guilds.find((g) => g.id === id)?.groups ?? []).flatMap((g) => g.channels).filter((c) => c.type === "text" && visible(c));
  const pick = texts.find((c) => c.id === ui.lastChannel[id]) ?? texts[0];
  if (pick) await selectChannel(pick.id);
}

export const visited = new Set<string>();

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
  }
}

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

export async function retryAccess(guildId: string) {
  const my = st.epoch;
  if (!(await loadAccess(guildId, true))) return;
  if (my === st.epoch && ui.view === guildId && ui.target.kind === "none") await selectGuild(guildId);
}
