import { api } from "../../lib/api";
import { channelPerms, checkPerms, has, Perm, type PermCheck } from "../../lib/perms";
import type { Channel } from "../types";
import { ui, setUi, NONE, type Target } from "../ui";
import { createStore } from "solid-js/store";
import { joinVoice, revokeVoice, setPasswordHandler, voice, voiceActive } from "../voice";
import { dmOf } from "./dm";
import { reloadChannels, selectChannel } from "./nav";
import { channelById, dmKey, guildOf, report, session, setSession, st } from "./state";

export function canSend(t: Target = ui.target): boolean {
  if (!session.me) return false;
  if (t.kind === "dm") {
    const c = dmOf(t.chatId);
    return Boolean(c && c.target.id && !c.target.system);
  }
  return t.kind === "channel" && Boolean(guildOf(t.channelId)) && sendBlock(t) === null;
}

export function msgKey(t: Target): string {
  return t.kind === "channel" ? t.channelId : t.kind === "dm" ? dmKey(t.chatId) : "";
}

export const accessLoads = new Map<string, { seq: number; p: Promise<boolean> }>();
let accessSeq = 0;
export function loadAccess(guildId: string, fresh = false): Promise<boolean> {
  const running = accessLoads.get(guildId);
  if (running && !fresh) return running.p;
  const gen = st.account;
  const seq = ++accessSeq;
  if (session.access[guildId]?.status !== "ready") setSession("access", guildId, { status: "loading" });
  const latest = () => gen === st.account && accessLoads.get(guildId)?.seq === seq;
  const p = api.guildAccess(guildId).then(
    (data) => {
      if (!latest()) return gen === st.account;
      setSession("access", guildId, { status: "ready", data });
      leaveHidden();
      return true;
    },
    (e) => {
      if (!latest()) return false;
      if (session.access[guildId]?.status !== "ready") setSession("access", guildId, { status: "failed" });
      if (ui.view === guildId) report(e);
      return false;
    },
  ).finally(() => {
    if (accessLoads.get(guildId)?.seq === seq) accessLoads.delete(guildId);
  });
  accessLoads.set(guildId, { seq, p });
  return p;
}

export function leaveHidden() {
  purgeHidden();
  const vc = voice.channelId;
  if (vc && voiceActive() && voiceBlock(vc) === "denied") void revokeVoice();
  const t = ui.target;
  if (t.kind !== "channel") return;
  const hit = channelById(t.channelId);
  if (hit && visible(hit.channel)) return;
  const next = (session.guilds.find((g) => g.id === t.guildId)?.groups ?? []).flatMap((g) => g.channels).find((c) => c.type === "text" && visible(c));
  if (next) void selectChannel(next.id);
  else {
    st.epoch++;
    setUi({ target: NONE });
  }
}

export function permsOf(channelId: string): number | null {
  const hit = channelById(channelId);
  const a = hit && session.access[hit.guildId];
  if (!hit || a?.status !== "ready" || !a.data || !session.me) return null;
  return channelPerms(a.data, session.me.id, hit.channel.overwrites, hit.channel.userOverwrites);
}

export async function checkChannelPerms(channelId: string, fresh = false): Promise<(PermCheck & { channelId: string; guildId: string; name: string }) | null> {
  const hit = channelById(channelId);
  if (!hit) return null;
  if (fresh || session.access[hit.guildId]?.status !== "ready") await loadAccess(hit.guildId, fresh);
  const a = session.access[hit.guildId];
  const cur = channelById(channelId);
  if (!cur || a?.status !== "ready" || !a.data || !session.me) return null;
  const r = checkPerms(a.data, session.me.id, cur.channel.overwrites, cur.channel.userOverwrites);
  return { ...r, channelId, guildId: cur.guildId, name: cur.channel.name };
}

export function visible(c: Channel): boolean {
  const p = permsOf(c.id);
  return p !== null && has(p, Perm.VIEW);
}

export function readable(channelId: string): boolean {
  const hit = channelById(channelId);
  return Boolean(hit && visible(hit.channel));
}

export function denied(channelId: string): boolean {
  const p = permsOf(channelId);
  return p !== null && !has(p, Perm.VIEW);
}

function purgeHidden() {
  for (const g of session.guilds) {
    if (session.access[g.id]?.status !== "ready") continue;
    for (const grp of g.groups)
      for (const c of grp.channels) {
        if (visible(c)) continue;
        if (session.messages[c.id]) setSession("messages", c.id, undefined!);
        if (session.history[c.id]) setSession("history", c.id, undefined!);
        if (session.unread[c.id]) setSession("unread", c.id, undefined!);
      }
  }
}

export function accessState(guildId: string): "loading" | "failed" | "ready" {
  return session.access[guildId]?.status ?? "loading";
}

export function voiceBlock(channelId: string): null | "loading" | "failed" | "denied" {
  const hit = channelById(channelId);
  if (!hit) return "denied";
  const st = session.access[hit.guildId]?.status;
  if (st === "failed") return "failed";
  const p = permsOf(channelId);
  if (p === null) return st === "ready" ? "denied" : "loading";
  return has(p, Perm.VIEW | Perm.VOICE_CONNECT) ? null : "denied";
}

const lastAccess = new Map<string, number>();
export const ACCESS_TTL = 30_000;
export function refreshAccess(guildId: string): Promise<boolean> {
  const running = accessLoads.get(guildId);
  if (running) return running.p;
  if (Date.now() - (lastAccess.get(guildId) ?? 0) < ACCESS_TTL) return Promise.resolve(session.access[guildId]?.status === "ready");
  lastAccess.set(guildId, Date.now());
  return loadAccess(guildId, true);
}

export const [pwPrompt, setPwPrompt] = createStore<{ channelId: string; error: string | null }>({ channelId: "", error: null });

let enterSeq = 0;
export async function enterVoice(channelId: string, password: string | null = null): Promise<void> {
  const gen = st.account;
  const my = ++enterSeq;
  const stale = () => gen !== st.account || my !== enterSeq;
  if (!channelById(channelId)) {
    const gid = await api.channelGuild(channelId).catch(() => "");
    if (stale()) return;
    if (gid && session.guilds.some((g) => g.id === gid)) await reloadChannels(gid);
    if (stale()) return;
  }
  const hit = channelById(channelId);
  if (hit) {
    await refreshAccess(hit.guildId);
    if (stale()) return;
    if (voiceBlock(channelId) !== null && voiceBlock(channelId) !== "denied") await loadAccess(hit.guildId, true);
    if (stale()) return;
    const block = voiceBlock(channelId);
    if (block) {
      if (pwPrompt.channelId === channelId) setPwPrompt({ channelId: "", error: null });
      return void report(block === "denied" ? "没有连接该语音频道的权限" : "无法取得频道权限");
    }
  }
  setPwPrompt({ channelId: "", error: null });
  lastPassword = { channelId, used: Boolean(password) };
  await joinVoice(channelId, password);
}

let lastPassword = { channelId: "", used: false };
setPasswordHandler((channelId) => {
  if (channelId !== lastPassword.channelId || voice.channelId !== channelId) return;
  setPwPrompt({ channelId, error: lastPassword.used ? "密码错误" : null });
});

export function cancelPassword() {
  enterSeq++;
  lastPassword = { channelId: "", used: false };
  setPwPrompt({ channelId: "", error: null });
}

export function sendBlock(t: Target = ui.target): null | "loading" | "failed" | "denied" | "none" {
  if (t.kind !== "channel") return "none";
  const st = session.access[t.guildId]?.status;
  if (st === "failed") return "failed";
  const p = permsOf(t.channelId);
  if (p === null) return st === "ready" ? "none" : "loading";
  return has(p, Perm.VIEW | Perm.SEND) ? null : "denied";
}

export function resetAccess() {
  accessLoads.clear();
  lastAccess.clear();
  enterSeq++;
  lastPassword = { channelId: "", used: false };
  setPwPrompt({ channelId: "", error: null });
}
