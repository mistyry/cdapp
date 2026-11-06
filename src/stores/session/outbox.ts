import { listen } from "@tauri-apps/api/event";
import { api, errorKind, errorText, MsgType, UPLOAD_LIMIT, type UploadProgress } from "../../lib/api";
import type { Message, Outgoing, OutgoingFile } from "../types";
import { Kind } from "../../lib/content";
import { ui, setUi, targetKey, type Reply, type Staged, type Target } from "../ui";
import { canSend, msgKey } from "./access";
import { plainContent, stamp, toMessage } from "./convert";
import { dmOf, noteDmMessage } from "./dm";
import { append, jumpLatest } from "./history";
import { setPreview } from "./gateway";
import { isDmKey, session, setSession, st } from "./state";

// 发送：目标在调用时固定，结果写回发起时的频道。KOOK 写接口不按 nonce 去重，因此不自动重发。
let seq = 0;

export function patchOutgoing(channelId: string, localId: string, patch: Partial<Outgoing> | null) {
  const list = session.outbox[channelId] ?? [];
  const next = patch ? list.map((o) => (o.localId === localId ? { ...o, ...patch } : o)) : list.filter((o) => o.localId !== localId);
  setSession("outbox", channelId, next);
}

export const findOutgoing = (channelId: string, localId: string) => session.outbox[channelId]?.find((o) => o.localId === localId);

// 发送文件时服务端转成的卡片，本地回显与之同形，网关回显按消息 ID 去重。
export const fileCard = (f: OutgoingFile) =>
  JSON.stringify([{ type: "card", theme: "none", size: "lg", modules: [{ type: "file", title: f.name, src: f.url, size: f.size }] }]);

// 卡片消息中文件模块的资源地址。按解析结果比对，不依赖服务端的转义方式。
export function cardFileSrcs(content: string): string[] {
  try {
    const cards = JSON.parse(content);
    if (!Array.isArray(cards)) return [];
    return cards.flatMap((c) => (Array.isArray(c?.modules) ? c.modules : []))
      .filter((m: { type?: unknown; src?: unknown }) => m?.type === "file" && typeof m.src === "string")
      .map((m: { src: string }) => m.src);
  } catch {
    return [];
  }
}

// 待上传文件的内容，按 localId 保存（不进入 store）。发送成功或放弃后释放。
export const fileBodies = new Map<string, Blob>();
let progressInit: Promise<void> | null = null;

export function onUploadProgress(p: UploadProgress) {
  for (const [channelId, list] of Object.entries(session.outbox)) {
    const o = list.find((x) => x.file?.uploadId === p.id);
    if (o?.status === "uploading") patchOutgoing(channelId, o.localId, { file: { ...o.file!, sent: p.sent } });
  }
}

export function watchProgress(): Promise<void> {
  // 监听在应用生命周期内保留，进度按上传 ID 匹配，旧账号的事件找不到条目。
  progressInit ??= listen<UploadProgress>("upload://progress", ({ payload }) => onUploadProgress(payload))
    .then(() => {})
    .catch(() => { progressInit = null; });
  return progressInit;
}

// 上传资源。成功后记下地址，之后的重试只重发消息。当前上传被取消或替换、条目已移除或已退出登录时返回 null。
export async function uploadFile(o: Outgoing, gen: number): Promise<string | null> {
  const f = o.file!;
  if (f.url) return f.url;
  const body = fileBodies.get(o.localId);
  if (!body) {
    patchOutgoing(o.channelId, o.localId, { status: "failed", error: "文件内容已丢失，请重新选择" });
    return null;
  }
  const uploadId = `${o.localId}-${++seq}`;
  patchOutgoing(o.channelId, o.localId, { status: "uploading", error: undefined, file: { ...f, uploadId, sent: 0 } });
  const current = () => gen === st.account && findOutgoing(o.channelId, o.localId)?.file?.uploadId === uploadId;
  try {
    await watchProgress();
    const bytes = new Uint8Array(await body.arrayBuffer());
    if (!current()) return null;
    const { url } = await api.upload(uploadId, f.name, bytes);
    if (!current()) return null;
    const cur = findOutgoing(o.channelId, o.localId)!;
    patchOutgoing(o.channelId, o.localId, { file: { ...cur.file!, url, uploadId: undefined, sent: f.size } });
    return url;
  } catch (e) {
    // 上传不产生消息，任何失败都可直接重试（同一内容得到相同地址）。
    if (current()) patchOutgoing(o.channelId, o.localId, { status: "failed", error: errorText(e), file: { ...findOutgoing(o.channelId, o.localId)!.file!, uploadId: undefined } });
    return null;
  }
}

// after：同批前一条消息的发送结果。上传可并行，发送按提交顺序依次进行。
export async function deliver(o: Outgoing, after?: Promise<void>) {
  const gen = st.account;
  const me = session.me!;
  let content = o.text;
  let kind: number = MsgType.KMARKDOWN;
  if (o.file) {
    const url = await uploadFile(o, gen);
    if (!url) return;
    if (after) {
      patchOutgoing(o.channelId, o.localId, { status: "sending", error: undefined });
      await after;
      if (gen !== st.account) return;
    }
    const cur = findOutgoing(o.channelId, o.localId);
    if (!cur) return;
    o = cur;
    content = url;
    kind = o.file!.image ? MsgType.IMAGE : MsgType.FILE;
  }
  patchOutgoing(o.channelId, o.localId, { status: "sending", error: undefined });
  try {
    // 发送接口只由消息键决定：私信键只走私信接口（以对方用户 ID 发送），私信内容不会发往频道。
    let id: string, createAt: number;
    if (isDmKey(o.channelId)) {
      const peer = dmOf(o.channelId.slice(3))?.target;
      if (!peer?.id || peer.system) {
        patchOutgoing(o.channelId, o.localId, { status: "failed", error: "私信会话不可用" });
        return;
      }
      const r = await api.sendDirect(peer.id, kind, content, o.reply?.id);
      if (gen !== st.account) return;
      id = r.id;
      createAt = r.createAt || Date.now();
    } else {
      const r = await api.sendMessage(o.channelId, kind, content, o.reply?.id);
      if (gen !== st.account) return;
      id = String(r.msg_id ?? `local-${o.localId}`);
      createAt = Number(r.msg_timestamp ?? Date.now());
    }
    const sent: Message = {
      id, author: me.id, createAt, ...stamp(createAt),
      type: kind === MsgType.FILE ? Kind.CARD : kind, content: kind === MsgType.FILE ? fileCard(o.file!) : content, quote: o.reply
        ? { id: o.reply.id, authorId: o.reply.authorId, author: o.reply.author, type: o.reply.type ?? Kind.KMARKDOWN, content: o.reply.content ?? o.reply.text, text: o.reply.text }
        : undefined,
    };
    // 本人在该频道的显示名取自已加载消息中本人的最近一条，没有时用账号名称。
    const mine = [...(session.messages[o.channelId] ?? [])].reverse().find((m) => m.author === me.id && m.name);
    sent.name = mine?.name ?? me.name;
    sent.avatar = mine?.avatar ?? me.avatar;
    append(o.channelId, sent);
    if (isDmKey(o.channelId)) noteDmMessage(o.channelId.slice(3), sent);
    // 发送者收不到自己消息的推送，频道预览按服务端格式本地更新。
    else setPreview(o.channelId, `${sent.name}：${plainContent(sent.type, sent.content, sent.attachment)}`);
    patchOutgoing(o.channelId, o.localId, null);
    fileBodies.delete(o.localId);
  } catch (e) {
    if (gen !== st.account) return;
    const kind = errorKind(e);
    patchOutgoing(o.channelId, o.localId, { status: kind === "uncertain" ? "uncertain" : "failed", error: errorText(e) });
  }
}

export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];

// 检查待发文件。服务端不校验内容，格式由客户端决定：常见图片按图片发送，其余按文件发送。
export function checkFile(size: number): string | null {
  if (size <= 0) return "不能发送空文件";
  if (size > UPLOAD_LIMIT) return "文件超过 5 MB 上限";
  return null;
}

// 选择、粘贴或拖入文件：加入目标会话的待发附件。目标在选择时固定，之后切换会话不影响。
// 返回被拒绝文件的原因（空或超限）。
export function stageFiles(target: Target, files: File[]): string[] {
  if (!canSend(target)) return files.length ? ["当前会话不能发送文件"] : [];
  const key = targetKey(target);
  const errors: string[] = [];
  const add: Staged[] = [];
  for (const f of files) {
    const err = checkFile(f.size);
    if (err) {
      errors.push(`${f.name || "文件"}：${err}`);
      continue;
    }
    const id = `s${Date.now().toString(36)}-${++seq}`;
    fileBodies.set(id, f);
    add.push({ id, channelId: msgKey(target), name: f.name || "file", size: f.size, image: IMAGE_TYPES.includes(f.type) });
  }
  if (add.length) setUi("staged", key, (l) => [...(l ?? []), ...add]);
  return errors;
}

export function unstage(key: string, id: string) {
  fileBodies.delete(id);
  setUi("staged", key, (l) => (l ?? []).filter((x) => x.id !== id));
}

// 发送文字与附件：文字（若有）在前，附件各为一条消息，均发往附件选择时的会话。引用附在第一条上。
// 返回是否已提交（调用方据此清空草稿与附件）。
export function submit(target: Target, text: string, reply?: Reply): boolean {
  const key = targetKey(target);
  const staged = key ? ui.staged[key] ?? [] : [];
  if (!staged.length) return send(target, text, reply);
  if (!canSend(target)) return false;
  let first = reply;
  let prev: Promise<void> | undefined;
  if (text.trim()) {
    prev = post(msgKey(target), text, first);
    first = undefined;
  }
  for (const st of staged) {
    const o: Outgoing = {
      localId: st.id, channelId: st.channelId, text: "", reply: first, createAt: Date.now(), status: "uploading",
      file: { name: st.name, size: st.size, image: st.image, sent: 0 },
    };
    first = undefined;
    setSession("outbox", o.channelId, (l) => [...(l ?? []), o]);
    // 发送失败不阻塞后续条目，只保证顺序。
    prev = deliver(o, prev).catch(() => {});
  }
  setUi("staged", key, []);
  const k = msgKey(target);
  if (session.history[k]?.newer) void jumpLatest(k);
  return true;
}

// 取消上传。条目保留为失败，可重试或删除；后端中止请求，迟到结果按上传 ID 丢弃。
export function cancelUpload(channelId: string, localId: string) {
  const o = findOutgoing(channelId, localId);
  if (!o || o.status !== "uploading" || !o.file) return;
  const id = o.file.uploadId;
  patchOutgoing(channelId, localId, { status: "failed", error: "已取消", file: { ...o.file, uploadId: undefined, sent: 0 } });
  if (id) api.uploadCancel(id).catch(() => {});
}

export function send(target: Target, text: string, reply?: Reply): boolean {
  if (!canSend(target) || !text.trim()) return false;
  void post(msgKey(target), text, reply);
  return true;
}

export function post(channelId: string, text: string, reply?: Reply): Promise<void> {
  const o: Outgoing = {
    localId: `${Date.now().toString(36)}-${++seq}`, channelId, text, reply,
    createAt: Date.now(), status: "sending",
  };
  setSession("outbox", o.channelId, (l) => [...(l ?? []), o]);
  // 正在查看跳转片段时先回到最新，发出的消息才能显示在列表末尾。
  if (session.history[o.channelId]?.newer) void jumpLatest(o.channelId);
  return deliver(o);
}

// 仅在确定未送达时直接重发；结果不确定须先 verify。
export function retry(channelId: string, localId: string) {
  const o = findOutgoing(channelId, localId);
  if (!o || o.status !== "failed" || !session.me) return;
  void deliver(o);
}

// 拉取最新历史，查找本账号在发送时间之后、内容相同的消息。找到则视为已送达，否则转为可重发。
export async function verify(channelId: string, localId: string) {
  const o = findOutgoing(channelId, localId);
  const me = session.me;
  if (!o || o.status !== "uncertain" || !me) return;
  const gen = st.account;
  patchOutgoing(channelId, localId, { status: "sending", error: undefined });
  try {
    const list = (await (isDmKey(channelId) ? api.dmMessages(channelId.slice(3)) : api.messages(channelId))).items.map(toMessage);
    if (gen !== st.account) return;
    // 文件消息按资源地址比对：图片内容即地址，文件卡片中含地址。
    const url = o.file?.url;
    const hit = list.find((m) => m.author === me.id && m.createAt >= o.createAt - 60_000
      && (url ? (m.type === Kind.IMAGE ? m.content === url : m.type === Kind.CARD && cardFileSrcs(m.content).includes(url)) : m.content === o.text));
    if (hit) {
      append(channelId, hit);
      patchOutgoing(channelId, localId, null);
    } else {
      patchOutgoing(channelId, localId, { status: "failed", error: isDmKey(channelId) ? "未在私信中找到该消息，可重新发送" : "未在频道中找到该消息，可重新发送" });
    }
  } catch (e) {
    if (gen === st.account) patchOutgoing(channelId, localId, { status: "uncertain", error: errorText(e) });
  }
}

// 放弃待发消息，内容交回该频道的输入框；输入框已有内容时不覆盖。
export function discard(channelId: string, localId: string, restore: boolean) {
  const o = findOutgoing(channelId, localId);
  if (!o || o.status === "sending" || o.status === "uploading") return;
  patchOutgoing(channelId, localId, null);
  const key = isDmKey(channelId) ? channelId : `channel:${channelId}`;
  if (o.file) {
    // 文件放回附件栏（内容仍在则可再次发送），否则释放。
    if (restore && fileBodies.has(localId)) setUi("staged", key, (l) => [...(l ?? []), { id: localId, channelId, name: o.file!.name, size: o.file!.size, image: o.file!.image }]);
    else fileBodies.delete(localId);
    return;
  }
  if (restore && !ui.drafts[key]) {
    setUi("drafts", key, o.text);
    if (o.reply && !ui.replies[key]) setUi("replies", key, o.reply);
  }
}

// 取消在途上传并释放待发文件。须在清空 outbox 之前调用。
export function resetOutbox() {
  for (const list of Object.values(session.outbox)) for (const o of list) if (o.file?.uploadId) api.uploadCancel(o.file.uploadId).catch(() => {});
  fileBodies.clear();
}
