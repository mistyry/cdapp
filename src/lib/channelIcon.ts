import { has, Perm } from "./perms";
import type { Channel } from "../stores/types";

// 频道图标状态。base 为基础图形；lock 为密码标识（语音的主要修饰）；
// restricted 表示频道自身对 @全体成员 限制了查看（语音另含连接）；
// label 为可访问名称与提示，补充图形无法表达的当前账号限制。
export type ChannelIcon = { base: "text" | "voice" | "unknown"; lock: boolean; restricted: boolean; label: string };

// p 为本人有效权限，null 表示依据未知。依据未知时不推断限制，只给类型说明。
export function channelIcon(c: Channel, p: number | null): ChannelIcon {
  const base = c.type;
  const lock = base === "voice" && Boolean(c.password);
  const limit = base === "voice" ? Perm.VIEW | Perm.VOICE_CONNECT : Perm.VIEW;
  const restricted = (c.overwrites ?? []).some((o) => o.roleId === 0 && (o.deny & limit) !== 0);
  const parts = [base === "text" ? "文字频道" : base === "voice" ? "语音频道" : "暂不支持此类频道"];
  if (lock) parts.push("需要密码");
  if (restricted) parts.push("受限频道");
  if (p !== null && base === "text" && !has(p, Perm.SEND)) parts.push("只读");
  if (p !== null && base === "voice" && !has(p, Perm.VOICE_CONNECT)) parts.push("没有连接权限");
  return { base, lock, restricted, label: parts.join("，") };
}
