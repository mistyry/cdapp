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

let seq = 0;

export function patchOutgoing(channelId: string, localId: string, patch: Partial<Outgoing> | null) {
  const list = session.outbox[channelId] ?? [];
  const next = patch ? list.map((o) => (o.localId === localId ? { ...o, ...patch } : o)) : list.filter((o) => o.localId !== localId);
  setSession("outbox", channelId, next);
}

export const findOutgoing = (channelId: string, localId: string) => session.outbox[channelId]?.find((o) => o.localId === localId);

export const fileCard = (f: OutgoingFile) =>
  JSON.stringify([{ type: "card", theme: "none", size: "lg", modules: [{ type: "file", title: f.name, src: f.url, size: f.size }] }]);

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

export const fileBodies = new Map<string, Blob>();
let progressInit: Promise<void> | null = null;

export function onUploadProgress(p: UploadProgress) {
  for (const [channelId, list] of Object.entries(session.outbox)) {
    const o = list.find((x) => x.file?.uploadId === p.id);
    if (o?.status === "uploading") patchOutgoing(channelId, o.localId, { file: { ...o.file!, sent: p.sent } });
  }
}

export function watchProgress(): Promise<void> {
  progressInit ??= listen<UploadProgress>("upload://progress", ({ payload }) => onUploadProgress(payload))
    .then(() => {})
    .catch(() => { progressInit = null; });
  return progressInit;
}

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
    if (current()) patchOutgoing(o.channelId, o.localId, { status: "failed", error: errorText(e), file: { ...findOutgoing(o.channelId, o.localId)!.file!, uploadId: undefined } });
    return null;
  }
}

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
    const mine = [...(session.messages[o.channelId] ?? [])].reverse().find((m) => m.author === me.id && m.name);
    sent.name = mine?.name ?? me.name;
    sent.avatar = mine?.avatar ?? me.avatar;
    append(o.channelId, sent);
    if (isDmKey(o.channelId)) noteDmMessage(o.channelId.slice(3), sent);
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

export function checkFile(size: number): string | null {
  if (size <= 0) return "不能发送空文件";
  if (size > UPLOAD_LIMIT) return "文件超过 5 MB 上限";
  return null;
}

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
    prev = deliver(o, prev).catch(() => {});
  }
  setUi("staged", key, []);
  const k = msgKey(target);
  if (session.history[k]?.newer) void jumpLatest(k);
  return true;
}

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
  if (session.history[o.channelId]?.newer) void jumpLatest(o.channelId);
  return deliver(o);
}

export function retry(channelId: string, localId: string) {
  const o = findOutgoing(channelId, localId);
  if (!o || o.status !== "failed" || !session.me) return;
  void deliver(o);
}

export async function verify(channelId: string, localId: string) {
  const o = findOutgoing(channelId, localId);
  const me = session.me;
  if (!o || o.status !== "uncertain" || !me) return;
  const gen = st.account;
  patchOutgoing(channelId, localId, { status: "sending", error: undefined });
  try {
    const list = (await (isDmKey(channelId) ? api.dmMessages(channelId.slice(3)) : api.messages(channelId))).items.map(toMessage);
    if (gen !== st.account) return;
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

export function discard(channelId: string, localId: string, restore: boolean) {
  const o = findOutgoing(channelId, localId);
  if (!o || o.status === "sending" || o.status === "uploading") return;
  patchOutgoing(channelId, localId, null);
  const key = isDmKey(channelId) ? channelId : `channel:${channelId}`;
  if (o.file) {
    if (restore && fileBodies.has(localId)) setUi("staged", key, (l) => [...(l ?? []), { id: localId, channelId, name: o.file!.name, size: o.file!.size, image: o.file!.image }]);
    else fileBodies.delete(localId);
    return;
  }
  if (restore && !ui.drafts[key]) {
    setUi("drafts", key, o.text);
    if (o.reply && !ui.replies[key]) setUi("replies", key, o.reply);
  }
}

export function resetOutbox() {
  for (const list of Object.values(session.outbox)) for (const o of list) if (o.file?.uploadId) api.uploadCancel(o.file.uploadId).catch(() => {});
  fileBodies.clear();
}
