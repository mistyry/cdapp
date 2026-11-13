import { createStore } from "solid-js/store";

export type Target =
  | { kind: "none" }
  | { kind: "channel"; guildId: string; channelId: string }
  | { kind: "dm"; chatId: string };

export type Reply = { id: string; authorId?: string; author: string; type?: number; content?: string; text: string };

export type Staged = { id: string; channelId: string; name: string; size: number; image: boolean };

export const NONE: Target = { kind: "none" };
export const DM_VIEW = "dm";

export function targetKey(t: Target): string {
  if (t.kind === "channel") return `channel:${t.channelId}`;
  if (t.kind === "dm") return `dm:${t.chatId}`;
  return "";
}

const initial = () => ({
  view: "",
  target: NONE as Target,
  lastChannel: {} as Record<string, string>,
  drafts: {} as Record<string, string>,
  replies: {} as Record<string, Reply | undefined>,
  staged: {} as Record<string, Staged[]>,
  settings: null as null | "account" | "voice" | "storage",
  showMembers: true,
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
export const currentMsgKey = () => (ui.target.kind === "channel" ? ui.target.channelId : ui.target.kind === "dm" ? `dm:${ui.target.chatId}` : "");
