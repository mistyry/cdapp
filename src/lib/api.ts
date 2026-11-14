import { invoke as rawInvoke, type Channel, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import type { GuildAccess, Overwrite, UserOverwrite } from "./perms";

export type Raw = Record<string, any>;
export type QrStatus = { status: "waiting" | "scanned" | "confirmed" | "unknown" };

export type Restore = { status: "none" } | { status: "ready"; me: Raw } | { status: "expired"; message: string };

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

export type InputDevice = { id: string; name: string; default: boolean };
export type AudioPrefs = { input: string | null; output: string | null; gain: number; master: number; effectVolume: number; effectsOff: number[]; muted: boolean; deafened: boolean; denoise?: boolean; denoiseMode?: DenoiseMode };
export type DenoiseMode = "kook" | "rnnoise";

export type ChannelDto = {
  id: string; name: string; kind: "category" | "text" | "voice" | "radio" | "unknown"; rawType: number;
  parentId: string | null; level: number; limit: number;
  overwrites?: Overwrite[]; userOverwrites?: UserOverwrite[];
  hasPassword?: boolean; permissionSync?: boolean;
  users?: VoiceUserDto[]; preview?: string;
};
export type VoiceUserDto = { id: string; username: string; nickname: string; identifyNum: string; avatar: string; live: LiveInfo | null };
export type LiveInfo = { resolution: number; frameRate: number; startTime: number; audienceCount: number; audienceLimit: number };

export type MemberDto = {
  id: string; username: string; nickname: string; identifyNum: string; avatar: string;
  online: boolean; bot: boolean; roles: number[]; color: number;
};
export type MemberPageDto = { items: MemberDto[]; page: number; pageTotal: number; total: number; online: number; offline: number };

export type HistoryPage = { items: Raw[]; end: boolean };

export type UnreadDto = { guildId: string; channelId: string; unread: boolean; mentions: number; readAt: number };

export type MessageLocation = { id: string; channelId: string; createAt: number };

export type DmUserDto = { id: string; username: string; nickname: string; identifyNum: string; avatar: string; online: boolean; bot: boolean; system: boolean };
export type DmChatDto = { code: string; target: DmUserDto; preview: string; latestAt: number; readAt: number; unread: number };
export type DmPageDto = { items: DmChatDto[]; page: number; pageTotal: number; total: number };

export type DmSentDto = { id: string; code: string; createAt: number };

export type EmojiItemDto = { id: string; name: string; animated: boolean };
export type EmojiSetDto = { guilds: Array<{ guildId: string; name: string; icon: string; items: EmojiItemDto[] }>; personal: EmojiItemDto[]; personalFailed: boolean; joined: string[] | null };

export const MsgType = { TEXT: 1, IMAGE: 2, FILE: 4, KMARKDOWN: 9 } as const;

export type AssetDto = { url: string };
export type UploadProgress = { id: string; sent: number; total: number };
export const UPLOAD_LIMIT = 5 * 1024 * 1024;

export interface Saying { game: string; role_name: string; words: string }

export type StorageInfo = {
  status: "ready" | "missing" | "unwritable" | "locked";
  message?: string;
  root: string;
  defaultRoot: string;
  custom: boolean;
  cacheBytes: number;
};

export type ShareSdk = { status: "ready"; version: string } | { status: "unavailable"; message: string };
export type ShareSource = {
  kind: "screen" | "window"; id: string; pid: number; primary: boolean;
  width: number; height: number; name: string; application: string;
};
export type ShareEvent =
  | { kind: "live"; width: number; height: number; watchable: boolean }
  | { kind: "tier"; resolution: number; frameRate: number; width: number; height: number }
  | { kind: "stats"; kbps: number; fps: number; width: number; height: number }
  | { kind: "ended"; message: string | null };
export type ShareStart = {
  channelId: string; sourceKind: "screen" | "window"; sourceId: string; sourcePid: number;
  resolution: number; frameRate: number; audio: boolean;
};
export type WatchEvent =
  | { kind: "waiting" }
  | { kind: "playing" }
  | { kind: "stats"; kbps: number; width: number; height: number; stalls: number }
  | { kind: "paused"; paused: boolean }
  | { kind: "ended"; message: string | null };

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
  denoiseSync: () => invoke<void>("denoise_sync"),
  shareSdkSync: () => invoke<ShareSdk>("share_sdk_sync"),
  sharePreviewOpen: () => invoke<void>("share_preview_open"),
  sharePreviewClose: () => invoke<void>("share_preview_close"),
  shareSources: () => invoke<ShareSource[]>("share_sources"),
  shareThumbnail: (kind: ShareSource["kind"], id: string) => invoke<string | null>("share_thumbnail", { kind, id }),
  shareStart: (params: ShareStart) => invoke<number>("share_start", { params }),
  shareStop: () => invoke<void>("share_stop"),
  shareUpdate: (generation: number, resolution: number, frameRate: number) =>
    invoke<void>("share_update", { generation, resolution, frameRate }),
  watchStart: (channelId: string, liver: string, volume: number, frames: Channel<ArrayBuffer>) =>
    invoke<number>("watch_start", { channelId, liver, volume, frames }),
  watchStop: () => invoke<void>("watch_stop"),
  watchKeyFrame: (generation: number) => invoke<void>("watch_key_frame", { generation }),
  watchVolume: (generation: number, percent: number) => invoke<void>("watch_volume", { generation, percent }),
  watchRedirect: (generation: number, frames: Channel<ArrayBuffer>) =>
    invoke<void>("watch_redirect", { generation, frames }),
  watchPipOpen: (generation: number, seq: number) => invoke<void>("watch_pip_open", { generation, seq }),
  watchPipRatio: (width: number, height: number) => invoke<void>("watch_pip_ratio", { width, height }),
  watchPipClose: () => invoke<void>("watch_pip_close"),
  watchPipCurrent: () => invoke<{ generation: number; seq: number } | null>("watch_pip_current"),
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
  channelGuild: (channelId: string) => invoke<string>("channel_guild", { channelId }),
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
  sendDirect: (userId: string, kind: number, content: string, quote?: string) =>
    invoke<DmSentDto>("send_direct", { userId, kind, content, quote }),
  updateDmMessage: (msgId: string, content: string) => invoke<void>("update_dm_message", { msgId, content }),
  deleteDmMessage: (msgId: string) => invoke<void>("delete_dm_message", { msgId }),
  dmReaction: (msgId: string, emoji: string, add: boolean) => invoke<void>("dm_reaction", { msgId, emoji, add }),
  upload: (id: string, name: string, bytes: Uint8Array) =>
    invoke<AssetDto>("upload", bytes, { headers: { "x-upload-id": id, "x-file-name": encodeURIComponent(name) } }),
  uploadCancel: (id: string) => invoke<void>("upload_cancel", { id }),
  gatewayStart: () => invoke<void>("gateway_start"),
  gatewayReconnect: () => invoke<void>("gateway_reconnect"),
  gatewayStop: () => invoke<void>("gateway_stop"),
  clientLog: (level: "info" | "warn" | "debug", message: string) => rawInvoke<void>("client_log", { level, message }),
  voiceJoin: (channelId: string, request: number, muted = false, deafened = false, micBeforeDeaf: boolean | null = null, password: string | null = null) =>
    invoke<number>("voice_join", { channelId, request, audio: { muted, deafened, micBeforeDeaf }, password }),
  voiceLeave: (request?: number) => invoke<void>("voice_leave", { request: request ?? null }),
  voiceWake: () => invoke<void>("voice_wake"),
  goliveAllowed: () => invoke<boolean>("golive_allowed"),
  voiceSetMic: (generation: number, paused: boolean) => invoke<void>("voice_set_mic", { generation, paused }),
  audioInputs: () => invoke<{ devices: InputDevice[]; selected: string | null }>("audio_inputs"),
  setAudioInput: (id: string | null) => invoke<void>("set_audio_input", { id }),
  voiceSetDeafen: (generation: number, deafened: boolean) => invoke<void>("voice_set_deafen", { generation, deafened }),
  audioOutputs: () => invoke<{ devices: InputDevice[]; selected: string | null }>("audio_outputs"),
  setAudioOutput: (id: string | null) => invoke<void>("set_audio_output", { id }),
  voiceSetVolume: (userId: string, percent: number) => invoke<void>("voice_set_volume", { userId, percent }),
  guildMute: (guildId: string, userId: string, kind: "mute" | "deafen", on: boolean) => invoke<void>("guild_mute", { guildId, userId, kind, on }),
  guildMutes: (guildId: string) => invoke<{ mute: string[]; deafen: string[] }>("guild_mutes", { guildId }),
  voiceKick: (channelId: string, userId: string) => invoke<void>("voice_kick", { channelId, userId }),
  voiceMove: (channelId: string, userId: string) => invoke<void>("voice_move", { channelId, userId }),
  guildKick: (guildId: string, userId: string) => invoke<void>("guild_kick", { guildId, userId }),
  guildBan: (guildId: string, userId: string) => invoke<void>("guild_ban", { guildId, userId }),
  voiceSetPeerDenoise: (userId: string, on: boolean) => invoke<void>("voice_set_peer_denoise", { userId, on }),
  voiceSetMaster: (percent: number) => invoke<void>("voice_set_master", { percent }),
  voiceSetGain: (percent: number) => invoke<number>("voice_set_gain", { percent }),
  micTest: (on: boolean) => invoke<void>("mic_test", { on }),
  voiceSetDenoise: (on: boolean, mode?: DenoiseMode) => invoke<void>("voice_set_denoise", { on, mode: mode ?? null }),
  audioPrefs: () => invoke<AudioPrefs>("audio_prefs"),
  setVoiceSwitches: (muted: boolean, deafened: boolean) => invoke<void>("set_voice_switches", { muted, deafened }),
  effectPlay: (id: number, preview = false) => invoke<void>("effect_play", { id, preview }),
  effectPlayItem: (item: number) => invoke<void>("effect_play_item", { item }),
  setEffectVolume: (percent: number) => invoke<void>("set_effect_volume", { percent }),
  setEffectEnabled: (id: number, on: boolean) => invoke<void>("set_effect_enabled", { id, on }),
};

export type ErrorKind = "not_sent" | "rejected" | "uncertain" | "local" | "unauthorized" | "rate_limited";
const KINDS: ErrorKind[] = ["not_sent", "rejected", "uncertain", "local", "unauthorized", "rate_limited"];

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && typeof (e as Raw).message === "string") return (e as Raw).message;
  return "未知错误";
}

export function errorKind(e: unknown): ErrorKind {
  const k = e && typeof e === "object" ? (e as Raw).kind : undefined;
  return KINDS.includes(k) ? k : "uncertain";
}
