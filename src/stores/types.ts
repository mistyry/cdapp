import type { GuildAccess, Overwrite, UserOverwrite } from "../lib/perms";
import type { LiveInfo, MemberDto } from "../lib/api";
import type { Attachment } from "../lib/content";
export type User = { id: string; name: string; tag?: string; avatar?: string; color?: string; online: boolean; activity?: string };
export type VoiceUser = User & { live?: LiveInfo };
export type Channel = {
  id: string; name: string; type: "text" | "voice" | "unknown"; radio?: boolean;
  preview?: string; limit?: number;
  users?: VoiceUser[];
  overwrites?: Overwrite[]; userOverwrites?: UserOverwrite[];
  password?: boolean; sync?: boolean;
};

export type Access = { status: "loading" | "ready" | "failed"; data?: GuildAccess };

export type MemberList = {
  key: string; guildId: string; channelId: string;
  items: MemberDto[]; page: number; pageTotal: number; total: number; online: number; offline: number;
  loading: boolean; failed: boolean;
};
export type Group = { id: string; name: string; loose?: boolean; channels: Channel[] };
export type Guild = { id: string; name: string; icon?: string; groups: Group[]; loaded?: boolean; failed?: boolean; liveCount?: number };
export type Quote = { id?: string; authorId?: string; author: string; type: number; content: string; text: string; gone?: boolean };
export type Reaction = { id: string; name: string; count: number; me: boolean; users: Record<string, boolean> };
export type Message = {
  id: string; rongId?: string; author: string; createAt: number; time: string; short: string;
  type: number; content: string; quote?: Quote; attachment?: Attachment;
  name?: string; avatar?: string; system?: boolean;
  updatedAt?: number; reactions?: Reaction[]; pending?: "editing" | "deleting";
};

export type OutgoingFile = { name: string; size: number; image: boolean; url?: string; uploadId?: string; sent: number };

export type Outgoing = {
  localId: string; channelId: string; text: string; reply?: { id: string; authorId?: string; author: string; type?: number; content?: string; text: string };
  createAt: number; status: "uploading" | "sending" | "failed" | "uncertain"; error?: string; file?: OutgoingFile;
};

export type History = { end: boolean; loading: boolean; failed: boolean; newer?: boolean };

export type Focus = { channelId: string; id: string; seq: number };

export type Unread = { guildId: string; unread: boolean; mentions: number; readAt: number };

export type DmList = { loaded: boolean; loading: boolean; failed: boolean; page: number; pageTotal: number };
