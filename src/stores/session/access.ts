import { api } from "../../lib/api";
import { channelPerms, has, Perm } from "../../lib/perms";
import type { Channel } from "../types";
import { ui, setUi, NONE, type Target } from "../ui";
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

// 当前频道失去查看权限（权限或频道覆盖变化）：切到该服务器的首个可见文字频道，没有则清空会话。
export function leaveHidden() {
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

// 是否可见。依据未知时按服务端列表显示，只在确认无查看权限时隐藏。
export function visible(c: Channel): boolean {
  const p = permsOf(c.id);
  return p === null || has(p, Perm.VIEW);
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
}
