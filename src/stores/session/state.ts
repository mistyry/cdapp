import { createStore } from "solid-js/store";
import { errorText, type DmChatDto } from "../../lib/api";
import type { FolderDto } from "../../lib/guildFolders";
import type { Access, Channel, DmList, Focus, Guild, History, MemberList, Message, Outgoing, Unread, User } from "../types";

// boot 恢复中；restore_failed 恢复时网络等临时错误（凭据保留，可重试）；login 未登录；ready 已登录。
export type State = {
  phase: "boot" | "restore_failed" | "login" | "ready";
  // 登录页提示，例如凭据失效原因。
  notice: string | null;
  // 已登录但服务器列表加载失败。
  loadFailed: boolean;
  me: User | null;
  users: Record<string, User>;
  guilds: Guild[];
  // 服务器栏文件夹。null 表示未加载或读取失败，此时服务器栏平铺显示。
  folders: FolderDto[] | null;
  foldersFailed: boolean;
  messages: Record<string, Message[]>;
  history: Record<string, History>;
  outbox: Record<string, Outgoing[]>;
  // 频道未读，键为频道 ID。只保存有未读或提及的频道，覆盖未进入过的服务器。
  unread: Record<string, Unread>;
  // 服务器权限依据，键为服务器 ID。
  access: Record<string, Access>;
  // 当前频道的成员列表。
  members: MemberList | null;
  // offline 未启动；connecting 首次连接；online 在线；reconnecting 断线等待重连；syncing 新会话补偿中。
  gateway: "offline" | "connecting" | "online" | "reconnecting" | "syncing";
  // 最近一次断线原因与预计重连时间（毫秒时间戳），在线后清空。
  gatewayInfo: { reason: string; retryAt: number } | null;
  // 最近一次恢复结果，供界面短暂提示。
  recovery: "resumed" | "rebuilt" | "partial" | null;
  error: string | null;
  // 消息中引用的角色与频道名称（来自 mention_role_part / channel_part），按 ID 存放，作为本地数据缺失时的补充。
  refs: { roles: Record<string, string>; channels: Record<string, string> };
  // 最近一次跳转目标。
  focus: Focus | null;
  // 私信会话，按最近消息时间降序；键 code 为 chat_code。
  dms: DmChatDto[];
  dmList: DmList;
  // 用户 ID → chat_code。两者不可互换，打开私信时按此映射。
  dmByUser: Record<string, string>;
};

export const DM_LIST0: DmList = { loaded: false, loading: false, failed: false, page: 0, pageTotal: 1 };

export const [session, setSession] = createStore<State>({
  phase: "boot", notice: null, loadFailed: false, me: null, users: {}, guilds: [], folders: null, foldersFailed: false, messages: {}, history: {}, outbox: {}, unread: {}, access: {}, members: null, gateway: "offline", gatewayInfo: null, recovery: null, error: null, refs: { roles: {}, channels: {} }, focus: null, dms: [], dmList: { ...DM_LIST0 }, dmByUser: {},
});

export const report = (e: unknown) => setSession("error", errorText(e));

// 代次：account 每次进入或退出登录递增，旧登录的迟到结果一律丢弃；
// epoch 每次切换会话或退出登录递增，异步结果只在代次未变时生效。
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
