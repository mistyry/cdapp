import { has, Perm } from "./perms";
import type { Channel } from "../stores/types";

export type ChannelIcon = { base: "text" | "voice" | "unknown"; lock: boolean; restricted: boolean; label: string; parts: string[] };

export function channelIcon(c: Channel, p: number | null): ChannelIcon {
  const base = c.type;
  const lock = base === "voice" && Boolean(c.password);
  const limit = base === "voice" ? Perm.VIEW | Perm.VOICE_CONNECT : Perm.VIEW;
  const restricted = (c.overwrites ?? []).some((o) => o.roleId === 0 && (o.deny & limit) !== 0);
  const kind = base === "text" ? "文字频道" : base === "voice" ? "语音频道" : "暂不支持此类频道";
  const parts = [
    p !== null && base === "voice" && !has(p, Perm.VOICE_CONNECT) ? "没有连接权限" : "",
    p !== null && base === "text" && !has(p, Perm.SEND) ? "只读" : "",
    lock ? "需要密码" : "",
    restricted ? "受限频道" : "",
    kind,
  ].filter(Boolean);
  return { base, lock, restricted, label: parts[0], parts };
}
