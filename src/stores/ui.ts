import { createStore } from "solid-js/store";

// 当前对话。chatId 为私信 chat_code；进入私信入口未选会话时为 none，绝不沿用旧频道。
export type Target =
  | { kind: "none" }
  | { kind: "channel"; guildId: string; channelId: string }
  | { kind: "dm"; chatId: string };

export type Reply = { id: string; authorId?: string; author: string; type?: number; content?: string; text: string };

// 已选择、尚未发送的附件。channelId 为选择时的会话，发送时不随当前会话改变。
export type Staged = { id: string; channelId: string; name: string; size: number; image: boolean };

export const NONE: Target = { kind: "none" };
export const DM_VIEW = "dm";

// 草稿与引用按会话键存放，切换会话不会互相残留。
export function targetKey(t: Target): string {
  if (t.kind === "channel") return `channel:${t.channelId}`;
  if (t.kind === "dm") return `dm:${t.chatId}`;
  return "";
}

const initial = () => ({
  view: "", // 服务器 ID 或 DM_VIEW
  target: NONE as Target,
  lastChannel: {} as Record<string, string>,
  drafts: {} as Record<string, string>,
  replies: {} as Record<string, Reply | undefined>,
  staged: {} as Record<string, Staged[]>,
  // 打开的全屏设置页，null 表示未打开。
  settings: null as null | "account" | "voice",
  showMembers: true,
  // 消息列表是否位于底部（读到最新）。由 MessageList 维护，用于判断是否可标记已读。
  atBottom: true,
  collapsed: {} as Record<string, boolean>,
});

export const [ui, setUi] = createStore(initial());

export function resetUi() {
  const { showMembers, collapsed } = ui;
  setUi({ ...initial(), showMembers, collapsed: { ...collapsed } });
}

export const currentKey = () => targetKey(ui.target);
export const currentChannelId = () => (ui.target.kind === "channel" ? ui.target.channelId : "");
export const currentChatCode = () => (ui.target.kind === "dm" ? ui.target.chatId : "");
// 当前会话的消息键：频道为频道 ID，私信为 dm:<chat_code>（与 targetKey 相同），两者永不冲突。
export const currentMsgKey = () => (ui.target.kind === "channel" ? ui.target.channelId : ui.target.kind === "dm" ? `dm:${ui.target.chatId}` : "");
