import { api } from "../../lib/api";
import { channelPerms, checkPerms, has, Perm, type PermCheck } from "../../lib/perms";
import type { Channel } from "../types";
import { ui, setUi, NONE, type Target } from "../ui";
import { createStore } from "solid-js/store";
import { joinVoice, revokeVoice, setPasswordHandler, voice, voiceActive } from "../voice";
import { dmOf } from "./dm";
import { selectChannel } from "./nav";
import { channelById, dmKey, guildOf, report, session, setSession, st } from "./state";

// 私信可发送：会话已在列表中、对方 ID 已知且不是系统会话。
export function canSend(t: Target = ui.target): boolean {
  if (!session.me) return false;
  if (t.kind === "dm") {
    const c = dmOf(t.chatId);
    return Boolean(c && c.target.id && !c.target.system);
  }
  return t.kind === "channel" && Boolean(guildOf(t.channelId)) && sendBlock(t) === null;
}

// 会话的消息键（outbox、messages、history 共用）：频道为频道 ID，私信为 dm:<chat_code>。
export function msgKey(t: Target): string {
  return t.kind === "channel" ? t.channelId : t.kind === "dm" ? dmKey(t.chatId) : "";
}

// 权限

// 拉取服务器权限依据。并发调用复用同一请求；失败时状态为 failed，界面不视为已获授权。
// 并发调用默认复用进行中的请求；fresh 表示数据可能已变化（权限事件、补偿），须另发请求，只采用最新一次的结果。
// 返回本次是否成功取得依据。
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
      // 已有依据时刷新失败保留旧值，避免短暂网络错误锁住输入；调用方据返回值判断失败。
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

// 权限或覆盖变化后：清除不可见频道的消息、未读与预览，结束失去权限的语音，
// 当前频道不可见时切到该服务器首个可见文字频道，没有则清空会话。
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

// 本人在频道中的有效权限；依据未就绪时为 null（未知）。
export function permsOf(channelId: string): number | null {
  const hit = channelById(channelId);
  const a = hit && session.access[hit.guildId];
  if (!hit || a?.status !== "ready" || !a.data || !session.me) return null;
  return channelPerms(a.data, session.me.id, hit.channel.overwrites, hit.channel.userOverwrites);
}

// 校验本人在频道中的最终权限，附逐层计算过程。依据未就绪时为 null。
// fresh 为 true 时先重新获取服务器权限依据。
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

// 是否可见。服务端列表不过滤无查看频道，依据未知时也不显示，避免暴露名称与预览。
export function visible(c: Channel): boolean {
  const p = permsOf(c.id);
  return p !== null && has(p, Perm.VIEW);
}

// 频道可读取：已知频道且确认有查看权限。迟到的历史与推送据此丢弃。
export function readable(channelId: string): boolean {
  const hit = channelById(channelId);
  return Boolean(hit && visible(hit.channel));
}

// 确认无查看权限（依据已就绪）。依据未知或频道未加载时为 false，由调用方另行处理。
export function denied(channelId: string): boolean {
  const p = permsOf(channelId);
  return p !== null && !has(p, Perm.VIEW);
}

// 移除确认不可见频道的内容。依据未知的频道保留，等依据就绪后再判断。
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

// 服务器权限的服务器可见状态：依据未就绪时界面显示加载或重试。
export function accessState(guildId: string): "loading" | "failed" | "ready" {
  return session.access[guildId]?.status ?? "loading";
}

// 加入语音的阻断原因；null 表示可加入。
export function voiceBlock(channelId: string): null | "loading" | "failed" | "denied" {
  const hit = channelById(channelId);
  if (!hit) return "denied";
  const st = session.access[hit.guildId]?.status;
  if (st === "failed") return "failed";
  const p = permsOf(channelId);
  if (p === null) return st === "ready" ? "denied" : "loading";
  return has(p, Perm.VIEW | Perm.VOICE_CONNECT) ? null : "denied";
}

// 窗口恢复、进入服务器等时机的节流刷新。距上次成功不足 30 秒时不重复请求。
const lastAccess = new Map<string, number>();
export const ACCESS_TTL = 30_000;
// 进行中的请求直接复用；按发起时间节流，失败后同样不立即重发。
export function refreshAccess(guildId: string): Promise<boolean> {
  const running = accessLoads.get(guildId);
  if (running) return running.p;
  if (Date.now() - (lastAccess.get(guildId) ?? 0) < ACCESS_TTL) return Promise.resolve(session.access[guildId]?.status === "ready");
  lastAccess.set(guildId, Date.now());
  return loadAccess(guildId, true);
}

// 密码输入。channelId 为待加入频道；error 为上次结果说明。密码不落盘，只用于本次请求。
export const [pwPrompt, setPwPrompt] = createStore<{ channelId: string; error: string | null }>({ channelId: "", error: null });

// 点击语音频道：先检查查看与连接权限，必要时刷新过期依据，再申请令牌。
// 服务端已记住密码时空密码即可加入；返回 40310 才提示输入。
// 每次点击或提交取新序号；被后来的点击、取消或退出登录取代后不再加入。
let enterSeq = 0;
export async function enterVoice(channelId: string, password: string | null = null): Promise<void> {
  const hit = channelById(channelId);
  if (!hit) return;
  const gen = st.account;
  const my = ++enterSeq;
  await refreshAccess(hit.guildId);
  if (gen !== st.account || my !== enterSeq) return;
  const block = voiceBlock(channelId);
  if (block) {
    // 密码框打开时一并关闭，原因以提示给出。
    if (pwPrompt.channelId === channelId) setPwPrompt({ channelId: "", error: null });
    return void report(block === "denied" ? "没有连接该语音频道的权限" : "尚未取得频道权限，请稍后重试");
  }
  setPwPrompt({ channelId: "", error: null });
  lastPassword = { channelId, used: Boolean(password) };
  await joinVoice(channelId, password);
}

// 服务端返回 40310：本次带了密码则为密码错误，否则提示输入。只对最近一次加入的频道弹出。
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

// 不能发送的原因；null 表示可发送。权限未知时不视为已授权。
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
