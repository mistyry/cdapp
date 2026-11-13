import { createStore } from "solid-js/store";
import { errorText, type DmChatDto } from "../../lib/api";
import type { FolderDto } from "../../lib/guildFolders";
import type { Access, Channel, DmList, Focus, Guild, History, MemberList, Message, Outgoing, Unread, User } from "../types";

export type State = {
  phase: "boot" | "restore_failed" | "login" | "ready";
  notice: string | null;
  loadFailed: boolean;
  me: User | null;
  users: Record<string, User>;
  guilds: Guild[];
  folders: FolderDto[] | null;
  foldersFailed: boolean;
  messages: Record<string, Message[]>;
  history: Record<string, History>;
  outbox: Record<string, Outgoing[]>;
  unread: Record<string, Unread>;
  access: Record<string, Access>;
  members: MemberList | null;
  gateway: "offline" | "connecting" | "online" | "reconnecting" | "syncing";
  gatewayInfo: { reason: string; retryAt: number } | null;
  recovery: "resumed" | "rebuilt" | "partial" | null;
  error: string | null;
  refs: { roles: Record<string, string>; channels: Record<string, string> };
  focus: Focus | null;
  dms: DmChatDto[];
  dmList: DmList;
  dmByUser: Record<string, string>;
};

export const DM_LIST0: DmList = { loaded: false, loading: false, failed: false, page: 0, pageTotal: 1 };

export const [session, setSession] = createStore<State>({
  phase: "boot", notice: null, loadFailed: false, me: null, users: {}, guilds: [], folders: null, foldersFailed: false, messages: {}, history: {}, outbox: {}, unread: {}, access: {}, members: null, gateway: "offline", gatewayInfo: null, recovery: null, error: null, refs: { roles: {}, channels: {} }, focus: null, dms: [], dmList: { ...DM_LIST0 }, dmByUser: {},
});

export const report = (e: unknown) => setSession("error", errorText(e));

export const st = { account: 0, epoch: 0 };
export const accountGen = () => st.account;

export const sessionUp = () => session.gateway === "online" || session.gateway === "syncing";

export function guildOf(channelId: string) {
  return session.guilds.find((g) => g.groups.some((grp) => grp.channels.some((c) => c.id === channelId && c.type === "text")));
}

export function channelById(channelId: string): { guildId: string; channel: Channel } | undefined {
  for (const g of session.guilds)
    for (const grp of g.groups) for (const c of grp.channels) if (c.id === channelId) return { guildId: g.id, channel: c };
  return undefined;
}

export function guildOfAny(channelId: string) {
  return session.guilds.find((g) => g.groups.some((grp) => grp.channels.some((c) => c.id === channelId)));
}

export const dmKey = (code: string) => `dm:${code}`;
export const isDmKey = (k: string) => k.startsWith("dm:");
