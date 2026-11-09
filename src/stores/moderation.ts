import { api, errorText } from "../lib/api";
import { has, Perm } from "../lib/perms";
import type { Channel } from "./types";
import { permsOf, session } from "./session";
import { channelById, guildOfAny } from "./session/state";
import { setVoice, voice } from "./voice";

// 语音成员管理。权限按当前语音频道的有效权限判断，服务端仍会校验。
// 已用两个测试账号核对：闭麦需第 25 位，静音需第 24 位，踢出语音与移动需第 16 位，
// 踢出服务器需第 6 位，黑名单需第 7 位。

export type ModPerms = { voiceManage: boolean; mute: boolean; deafen: boolean; kick: boolean; ban: boolean };

export function modPerms(channelId: string): ModPerms {
  const p = permsOf(channelId);
  const ok = (bit: number) => p !== null && has(p, bit);
  return {
    voiceManage: ok(Perm.VOICE_MANAGE),
    mute: ok(Perm.SERVER_MUTE),
    deafen: ok(Perm.SERVER_DEAFEN),
    kick: ok(Perm.KICK),
    ban: ok(Perm.BAN),
  };
}

export const anyMod = (m: ModPerms) => m.voiceManage || m.mute || m.deafen || m.kick || m.ban;

// 可移入的其他语音频道：同一服务器、本人可见且可连接。
export function moveTargets(channelId: string): Channel[] {
  const g = guildOfAny(channelId);
  if (!g) return [];
  const out: Channel[] = [];
  for (const grp of g.groups)
    for (const c of grp.channels) {
      if (c.type !== "voice" || c.id === channelId) continue;
      const p = permsOf(c.id);
      if (p !== null && has(p, Perm.VIEW)) out.push(c);
    }
  return out;
}

const guildIdOf = (channelId: string) => channelById(channelId)?.guildId ?? "";

function patchPeer(id: string, patch: { serverMute?: boolean; serverDeafen?: boolean }) {
  const i = voice.peers.findIndex((p) => p.id === id);
  if (i >= 0) setVoice("peers", i, patch);
}

async function run(f: () => Promise<void>): Promise<string | null> {
  try {
    await f();
    return null;
  } catch (e) {
    return errorText(e);
  }
}

// 服务器闭麦或静音。成功后先更新本地，服务端随后经语音信令确认。
export async function setServerMute(channelId: string, userId: string, kind: "mute" | "deafen", on: boolean) {
  const err = await run(() => api.guildMute(guildIdOf(channelId), userId, kind, on));
  if (!err) patchPeer(userId, kind === "mute" ? { serverMute: on } : { serverDeafen: on });
  return err;
}

export const kickFromVoice = (channelId: string, userId: string) => run(() => api.voiceKick(channelId, userId));
export const moveToVoice = (targetId: string, userId: string) => run(() => api.voiceMove(targetId, userId));
export const kickFromGuild = (channelId: string, userId: string) => run(() => api.guildKick(guildIdOf(channelId), userId));
export const banFromGuild = (channelId: string, userId: string) => run(() => api.guildBan(guildIdOf(channelId), userId));

export const isMe = (id: string) => id === session.me?.id;
