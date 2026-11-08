import { invoke as rawInvoke, type Channel, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import type { GuildAccess, Overwrite, UserOverwrite } from "./perms";

// 后端命令封装。接口返回原始 JSON，字段以网页端抓包为准。
export type Raw = Record<string, any>;
export type QrStatus = { status: "waiting" | "scanned" | "confirmed" | "unknown" };

export type Restore = { status: "none" } | { status: "ready"; me: Raw } | { status: "expired"; message: string };

// 任一命令返回凭据失效时通知会话层结束登录。
// 请求发起时记录登录代次，旧登录的迟到 401 不结束新登录。
let onUnauthorized: (message: string, gen: number) => void = () => {};
let generation: () => number = () => 0;
export const setUnauthorizedHandler = (f: (message: string, gen: number) => void, gen: () => number) => {
  onUnauthorized = f;
  generation = gen;
};

async function invoke<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  const gen = generation();
  try {
    return await rawInvoke<T>(cmd, args, options);
  } catch (e) {
    if (errorKind(e) === "unauthorized") onUnauthorized(errorText(e), gen);
    throw e;
  }
}

// 后端 audio::capture::InputDevice。
export type InputDevice = { id: string; name: string; default: boolean };
// 后端 audio::store::AudioPrefs。设备为 null 时跟随系统默认。
export type AudioPrefs = { input: string | null; output: string | null; gain: number; master: number; effectVolume: number; effectsOff: number[] };

// 后端 kook::model::Channel。kind 为 category / text / voice / unknown。
export type ChannelDto = {
  id: string; name: string; kind: "category" | "text" | "voice" | "unknown"; rawType: number;
  parentId: string | null; level: number; limit: number;
  overwrites?: Overwrite[]; userOverwrites?: UserOverwrite[];
  hasPassword?: boolean; permissionSync?: boolean;
  users?: VoiceUserDto[]; preview?: string;
};
// 后端 kook::model::VoiceUser。
export type VoiceUserDto = { id: string; username: string; nickname: string; identifyNum: string; avatar: string };

// 后端 kook::model::Member 与 MemberPage。成员按服务端顺序（在线在前），人数取服务端统计。
export type MemberDto = {
  id: string; username: string; nickname: string; identifyNum: string; avatar: string;
  online: boolean; bot: boolean; roles: number[]; color: number;
};
export type MemberPageDto = { items: MemberDto[]; page: number; pageTotal: number; total: number; online: number; offline: number };

// 一页历史，按时间从旧到新；end 表示没有更早的消息。
export type HistoryPage = { items: Raw[]; end: boolean };

// 后端 kook::model::ChannelUnread，只含有未读或提及的频道。
export type UnreadDto = { guildId: string; channelId: string; unread: boolean; mentions: number; readAt: number };

// 后端 kook::model::MessageLocation。id 为 rong_id。
export type MessageLocation = { id: string; channelId: string; createAt: number };

// 后端 kook::model::DmChat / DmPage。code 为 chat_code（不是用户 ID，也不是频道 ID），对方用户 ID 为 target.id。
export type DmUserDto = { id: string; username: string; nickname: string; identifyNum: string; avatar: string; online: boolean; bot: boolean; system: boolean };
export type DmChatDto = { code: string; target: DmUserDto; preview: string; latestAt: number; readAt: number; unread: number };
export type DmPageDto = { items: DmChatDto[]; page: number; pageTotal: number; total: number };

// 后端 kook::model::DmSent。id 为消息 ID，code 为 chat_code。
export type DmSentDto = { id: string; code: string; createAt: number };

// 后端 kook::emoji::EmojiSet。服务器表情按服务器分组；个人表情读取失败时 personalFailed 为 true。
export type EmojiItemDto = { id: string; name: string; animated: boolean };
export type EmojiSetDto = { guilds: Array<{ guildId: string; name: string; icon: string; items: EmojiItemDto[] }>; personal: EmojiItemDto[]; personalFailed: boolean; joined: string[] | null };

export const MsgType = { TEXT: 1, IMAGE: 2, FILE: 4, KMARKDOWN: 9 } as const;

// 后端 kook::model::Asset。上传进度事件 upload://progress 为 {id, sent, total}。
export type AssetDto = { url: string };
export type UploadProgress = { id: string; sent: number; total: number };
// 实测普通账号上限 5 MiB（含）；空文件会被服务端换成默认图片。后端同样检查。
export const UPLOAD_LIMIT = 5 * 1024 * 1024;

export interface Saying { game: string; role_name: string; words: string }

// 后端 storage::Info。
export type StorageInfo = {
  status: "ready" | "missing" | "unwritable" | "locked";
  message?: string;
  root: string;
  defaultRoot: string;
  custom: boolean;
  cacheBytes: number;
};

export const api = {
  storageInfo: () => invoke<StorageInfo>("storage_info"),
  storageRelocate: (path: string) => invoke<void>("storage_relocate", { path }),
  storageSwitch: (path: string | null) => invoke<void>("storage_switch", { path }),
  storageClearCache: () => invoke<void>("storage_clear_cache"),
  accountState: (user: string) => invoke<Record<string, unknown>>("account_state", { user }),
  setAccountState: (user: string, key: string, value: unknown) => invoke<void>("set_account_state", { user, key, value }),
  restart: () => invoke<void>("restart"),
  restoreSession: () => invoke<Restore>("restore_session"),
  sayings: () => invoke<Saying[]>("sayings"),
  soundSync: (progress: Channel<{ done: number; total: number }>) =>
    invoke<{ total: number; downloaded: number; failed: number }>("sound_sync", { progress }),
  loginQrCode: () => invoke<string>("login_qr_code"),
  loginQrPoll: (content: string) => invoke<QrStatus>("login_qr_poll", { content }),
  loginQrCancel: (content: string) => invoke<void>("login_qr_cancel", { content }),
  logout: () => invoke<void>("logout"),
  me: () => invoke<Raw>("me"),
  guilds: () => invoke<Raw[]>("guilds"),
  guildFolders: () => invoke<unknown>("guild_folders"),
  channels: (guildId: string) => invoke<ChannelDto[]>("channels", { guildId }),
  activateGuild: (guildId: string) => invoke<Raw>("activate_guild", { guildId }),
  guildAccess: (guildId: string) => invoke<GuildAccess>("guild_access", { guildId }),
  emojis: () => invoke<EmojiSetDto>("emojis"),
  members: (guildId: string, channelId: string | undefined, page: number) => invoke<MemberPageDto>("members", { guildId, channelId, page }),
  subscribeMembers: (channelId: string, start: number, end: number) => invoke<void>("subscribe_members", { channelId, start, end }),
  messages: (channelId: string, before?: string) => invoke<HistoryPage>("messages", { channelId, before }),
  messagesAfter: (channelId: string, after: string) => invoke<HistoryPage>("messages_after", { channelId, after }),
  locateMessage: (msgId: string) => invoke<MessageLocation | null>("locate_message", { msgId }),
  sendMessage: (channelId: string, kind: number, content: string, quote?: string) =>
    invoke<Raw>("send_message", { channelId, kind, content, quote }),
  updateMessage: (msgId: string, content: string) => invoke<Raw>("update_message", { msgId, content }),
  deleteMessage: (msgId: string) => invoke<Raw>("delete_message", { msgId }),
  reaction: (msgId: string, emoji: string, add: boolean) => invoke<Raw>("reaction", { msgId, emoji, add }),
  markRead: (channelId: string, msgId: string, time: number) => invoke<Raw>("mark_read", { channelId, msgId, time }),
  unread: () => invoke<UnreadDto[]>("unread"),
  dmChats: (page: number) => invoke<DmPageDto>("dm_chats", { page }),
  openDm: (userId: string) => invoke<DmChatDto>("open_dm", { userId }),
  dmMessages: (chatCode: string, before?: string) => invoke<HistoryPage>("dm_messages", { chatCode, before }),
  markDmRead: (chatCode: string, msgId: string, time: number) => invoke<void>("mark_dm_read", { chatCode, msgId, time }),
  // 私信写操作只走私信接口：频道删除接口对私信返回成功但不删除，频道编辑接口返回 40012。
  sendDirect: (userId: string, kind: number, content: string, quote?: string) =>
    invoke<DmSentDto>("send_direct", { userId, kind, content, quote }),
  updateDmMessage: (msgId: string, content: string) => invoke<void>("update_dm_message", { msgId, content }),
  deleteDmMessage: (msgId: string) => invoke<void>("delete_dm_message", { msgId }),
  dmReaction: (msgId: string, emoji: string, add: boolean) => invoke<void>("dm_reaction", { msgId, emoji, add }),
  // 原始字节作为请求体，文件名经 URI 编码放在请求头中。
  upload: (id: string, name: string, bytes: Uint8Array) =>
    invoke<AssetDto>("upload", bytes, { headers: { "x-upload-id": id, "x-file-name": encodeURIComponent(name) } }),
  uploadCancel: (id: string) => invoke<void>("upload_cancel", { id }),
  gatewayStart: () => invoke<void>("gateway_start"),
  gatewayReconnect: () => invoke<void>("gateway_reconnect"),
  gatewayStop: () => invoke<void>("gateway_stop"),
  // 返回会话代次，进度与成员经 voice://event 推送。
  // request 为前端请求序号，后端据此丢弃已被后发的加入或退出取代的加入。
  // password 仅用于本次加入，不保存。
  voiceJoin: (channelId: string, request: number, muted = false, deafened = false, micBeforeDeaf: boolean | null = null, password: string | null = null) =>
    invoke<number>("voice_join", { channelId, request, audio: { muted, deafened, micBeforeDeaf }, password }),
  voiceLeave: (request?: number) => invoke<void>("voice_leave", { request: request ?? null }),
  // 通知语音会话系统已唤醒或网络已恢复。
  voiceWake: () => invoke<void>("voice_wake"),
  voiceSetMic: (generation: number, paused: boolean) => invoke<void>("voice_set_mic", { generation, paused }),
  // 列出输入设备，不打开设备，不触发系统权限请求。
  audioInputs: () => invoke<{ devices: InputDevice[]; selected: string | null }>("audio_inputs"),
  setAudioInput: (id: string | null) => invoke<void>("set_audio_input", { id }),
  // 关闭耳机时后端同时静音麦克风。
  voiceSetDeafen: (generation: number, deafened: boolean) => invoke<void>("voice_set_deafen", { generation, deafened }),
  audioOutputs: () => invoke<{ devices: InputDevice[]; selected: string | null }>("audio_outputs"),
  setAudioOutput: (id: string | null) => invoke<void>("set_audio_output", { id }),
  // 成员音量与总音量的上限为 200。
  voiceSetVolume: (userId: string, percent: number) => invoke<void>("voice_set_volume", { userId, percent }),
  voiceSetMaster: (percent: number) => invoke<void>("voice_set_master", { percent }),
  // 麦克风输入增益，上限 200，返回实际生效的值。
  voiceSetGain: (percent: number) => invoke<number>("voice_set_gain", { percent }),
  audioPrefs: () => invoke<AudioPrefs>("audio_prefs"),
  effectPlay: (id: number, preview = false) => invoke<void>("effect_play", { id, preview }),
  setEffectVolume: (percent: number) => invoke<void>("set_effect_volume", { percent }),
  setEffectEnabled: (id: number, on: boolean) => invoke<void>("set_effect_enabled", { id, on }),
};

// 后端错误带结果确定性：not_sent 未发出，rejected 服务端拒绝，uncertain 可能已被处理，local 本地错误，
// unauthorized 凭据已失效（请求未被处理），rate_limited 被限流（请求未被处理，稍后可重试）。
export type ErrorKind = "not_sent" | "rejected" | "uncertain" | "local" | "unauthorized" | "rate_limited";
const KINDS: ErrorKind[] = ["not_sent", "rejected", "uncertain", "local", "unauthorized", "rate_limited"];

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && typeof (e as Raw).message === "string") return (e as Raw).message;
  return "未知错误";
}

// 无法识别的错误按不确定处理，避免误判为未发出而重复发送。
export function errorKind(e: unknown): ErrorKind {
  const k = e && typeof e === "object" ? (e as Raw).kind : undefined;
  return KINDS.includes(k) ? k : "uncertain";
}
