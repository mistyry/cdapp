import { api, MsgType, type DmChatDto, type Raw } from "../../lib/api";
import type { Message } from "../types";
import { isText, parseCards, toAttachment, Kind } from "../../lib/content";
import { ui, setUi, DM_VIEW, currentChatCode } from "../ui";
import { parse, plainContent, remember, rememberRefs, stamp, toMessage, toQuote, toUser } from "./convert";
import { removeMessage } from "./edit";
import { firstSeen } from "./gateway";
import { append, deleted, freshen, genOf, listGen, loading, mergeMessages, patchHistory, settleFetched, trackFetch, untrackFetch } from "./history";
import { dmKey, report, session, setSession, st } from "./state";
import { readCurrent, reading } from "./unread";

let dmSeq = 0;

export function upsertDms(list: DmChatDto[], refresh?: { pageTotal: number }) {
  const byCode = new Map(session.dms.map((c) => [c.code, c]));
  if (refresh) {
    const got = new Set(list.map((c) => c.code));
    const cutoff = list.length ? Math.min(...list.map((c) => c.latestAt)) : Infinity;
    for (const [code, c] of byCode) if (!got.has(code) && (refresh.pageTotal <= 1 || c.latestAt >= cutoff)) byCode.delete(code);
  }
  for (const c of list) {
    const old = byCode.get(c.code);
    let next = c;
    if (old && old.latestAt > c.latestAt) next = { ...c, preview: old.preview, latestAt: old.latestAt, unread: old.unread, readAt: Math.max(old.readAt, c.readAt) };
    else if (old && old.readAt > c.readAt && old.unread === 0 && c.latestAt <= old.readAt) next = { ...c, unread: 0, readAt: old.readAt };
    byCode.set(c.code, next);
    if (c.target.id) setSession("dmByUser", c.target.id, c.code);
    remember({ id: c.target.id, name: c.target.nickname || c.target.username || "未知用户", tag: c.target.identifyNum || undefined, avatar: c.target.avatar || undefined, online: c.target.online });
  }
  setSession("dms", [...byCode.values()].sort((a, b) => b.latestAt - a.latestAt));
}

export async function loadDms(more = false): Promise<boolean> {
  const l = session.dmList;
  if (more && (l.loading || !l.loaded || l.page >= l.pageTotal)) return true;
  const my = ++dmSeq;
  const gen = st.account;
  const page = more ? l.page + 1 : 1;
  setSession("dmList", { loading: true, failed: false });
  try {
    const r = await api.dmChats(page);
    if (gen !== st.account || my !== dmSeq) return true;
    upsertDms(r.items, more ? undefined : { pageTotal: r.pageTotal });
    setSession("dmList", { loaded: true, loading: false, failed: false, page: more ? page : Math.min(Math.max(1, session.dmList.page), Math.max(1, r.pageTotal)), pageTotal: r.pageTotal });
    return true;
  } catch (e) {
    if (gen !== st.account || my !== dmSeq) return true;
    setSession("dmList", { loading: false, failed: true });
    if (ui.view === DM_VIEW) report(e);
    return false;
  }
}

export const dmOf = (code: string) => session.dms.find((c) => c.code === code);

export async function selectDm(code: string) {
  if (!code) return;
  const my = ++st.epoch;
  setUi({ view: DM_VIEW, target: { kind: "dm", chatId: code } });
  if (!session.messages[dmKey(code)]) await loadDmHistory(code);
  if (my === st.epoch) readCurrent();
}

export const dmOpens = new Map<string, Promise<string | null>>();
export async function openDmWith(userId: string): Promise<string | null> {
  if (!userId || userId === session.me?.id) return null;
  const known = session.dmByUser[userId];
  if (known) {
    await selectDm(known);
    return known;
  }
  const my = ++st.epoch;
  const gen = st.account;
  let p = dmOpens.get(userId);
  if (!p) {
    p = api.openDm(userId).then(
      (c) => {
        if (gen !== st.account) return null;
        upsertDms([c]);
        return c.code;
      },
      (e) => {
        if (gen === st.account) report(e);
        return null;
      },
    ).finally(() => dmOpens.delete(userId));
    dmOpens.set(userId, p);
  }
  const code = await p;
  if (!code || gen !== st.account || my !== st.epoch) return code;
  await selectDm(code);
  return code;
}

export const dmLoads = new Map<string, Promise<void>>();

export function loadDmHistory(code: string): Promise<void> {
  const key = dmKey(code);
  if (session.messages[key]) return refreshDm(code).then(() => {});
  const running = dmLoads.get(key);
  if (running) return running;
  const gen = st.account;
  const p = (async () => {
    patchHistory(key, { failed: false, loading: true });
    loading.set(key, []);
    const log = trackFetch(key);
    try {
      const page = await api.dmMessages(code);
      if (gen !== st.account) return;
      const fetched = settleFetched(key, page.items.map(toMessage), log);
      const buffered = loading.get(key) ?? [];
      setSession("messages", key, (l) => mergeMessages(l ?? [], fetched, buffered));
      patchHistory(key, { end: page.end, loading: false });
    } catch (e) {
      if (gen !== st.account) return;
      patchHistory(key, { failed: true, loading: false });
      if (currentChatCode() === code) report(e);
    } finally {
      untrackFetch(key, log);
      if (gen === st.account) loading.delete(key);
    }
  })().finally(() => {
    if (gen === st.account) dmLoads.delete(key);
  });
  dmLoads.set(key, p);
  return p;
}

export async function refreshDm(code: string): Promise<boolean> {
  const key = dmKey(code);
  const gen = st.account;
  const log = trackFetch(key);
  try {
    const page = await api.dmMessages(code);
    if (gen !== st.account || !session.messages[key]) return true;
    const fetched = settleFetched(key, page.items.map(toMessage), log);
    const known = new Set(session.messages[key].map((m) => m.id));
    if (!known.size || fetched.some((m) => known.has(m.id)) || page.end) {
      const ids = new Set(fetched.map((m) => m.id));
      const from = page.end ? -Infinity : fetched[0]?.createAt ?? Infinity;
      const to = fetched[fetched.length - 1]?.createAt ?? -Infinity;
      const gone = session.messages[key].filter((m) => !ids.has(m.id) && !m.pending && m.createAt >= from && m.createAt <= to).map((m) => m.id);
      for (const id of gone) removeMessage(key, id);
      setSession("messages", key, (l) => mergeMessages(freshen(l, fetched), fetched));
      if (page.end) patchHistory(key, { end: true });
    } else {
      listGen.set(key, genOf(key) + 1);
      setSession("messages", key, fetched);
      patchHistory(key, { end: false, failed: false, loading: false });
    }
    return true;
  } catch (e) {
    if (gen === st.account && currentChatCode() === code) report(e);
    return false;
  } finally {
    untrackFetch(key, log);
  }
}

export async function loadDmOlder(code: string) {
  const key = dmKey(code);
  const h = session.history[key];
  const list = session.messages[key];
  if (!h || !list || h.end || h.loading) return;
  const cursor = list[0]?.id;
  if (!cursor) return patchHistory(key, { end: true });
  const gen = st.account;
  const lg = genOf(key);
  patchHistory(key, { loading: true, failed: false });
  try {
    const page = await api.dmMessages(code, cursor);
    if (gen !== st.account || !session.messages[key]) return;
    if (lg !== genOf(key)) return;
    const fresh = settleFetched(key, page.items.map(toMessage)).filter((m) => !session.messages[key].some((x) => x.id === m.id));
    if (fresh.length) setSession("messages", key, (l) => mergeMessages(fresh, l));
    patchHistory(key, { end: page.end || fresh.length === 0, loading: false });
  } catch {
    if (gen === st.account && lg === genOf(key)) patchHistory(key, { failed: true, loading: false });
  }
}

export const dmMarking = new Map<string, number>();

export function readDm(code: string) {
  const c = dmOf(code);
  if (!c || c.unread === 0 || !reading()) return;
  const list = session.messages[dmKey(code)] ?? [];
  const last = list[list.length - 1];
  if (!last || last.id.startsWith("local-")) return;
  if ((dmMarking.get(code) ?? -1) >= last.createAt) return;
  dmMarking.set(code, last.createAt);
  const gen = st.account;
  api.markDmRead(code, last.id, last.createAt).then(
    () => {
      if (gen !== st.account) return;
      if (dmMarking.get(code) === last.createAt) dmMarking.delete(code);
      const i = session.dms.findIndex((x) => x.code === code);
      if (i >= 0 && session.dms[i].latestAt <= last.createAt) setSession("dms", i, { unread: 0, readAt: last.createAt });
    },
    () => {
      if (gen === st.account && dmMarking.get(code) === last.createAt) dmMarking.delete(code);
    },
  );
}

export const dmUnreadTotal = () => session.dms.reduce((n, c) => n + c.unread, 0);

export function onPrivate(d: Raw) {
  const id = String(d.msgId ?? "");
  if (!firstSeen(id)) return;
  const content = parse(d.content);
  const extra: Raw = parse(content?.extra ?? d.extra) ?? {};
  const code = String(extra.code ?? "");
  if (!code) return;
  const key = dmKey(code);
  if (deleted.get(key)?.has(id)) return;
  const author = toUser(extra.author ?? { id: d.fromUserId });
  if (author.id) remember(author);
  rememberRefs(extra.kmarkdown);
  const createAt = Number(d.msgTimestamp ?? Date.now());
  const type = Number(extra.type ?? content?.type ?? MsgType.TEXT);
  const msg: Message = {
    id, author: author.id, createAt, ...stamp(createAt), type,
    content: String(content?.content ?? ""), quote: toQuote(extra.quote),
  };
  if (msg.type === Kind.FILE && parseCards(msg.content)?.length) msg.type = Kind.CARD;
  if (!isText(msg.type) && msg.type !== Kind.IMAGE) msg.attachment = toAttachment(extra.attachments);
  append(key, msg);
  noteDmMessage(code, msg);
}

export function noteDmMessage(code: string, msg: Message) {
  const i = session.dms.findIndex((c) => c.code === code);
  if (i < 0) {
    void loadDms();
    return;
  }
  const mine = msg.author === session.me?.id;
  const c = session.dms[i];
  if (msg.createAt < c.latestAt) {
    if (!mine) setSession("dms", i, "unread", c.unread + 1);
  } else {
    setSession("dms", i, { preview: plainContent(msg.type, msg.content, msg.attachment), latestAt: msg.createAt, unread: mine ? c.unread : c.unread + 1 });
    setSession("dms", (l) => [...l].sort((a, b) => b.latestAt - a.latestAt));
  }
  if (currentChatCode() === code) readCurrent();
}

export function noteDmRead(c: Raw) {
  const code = String(c?.code ?? "");
  const time = Number(c?.last_read_time ?? 0);
  const i = code ? session.dms.findIndex((x) => x.code === code) : -1;
  if (i >= 0 && time > 0) {
    if (time >= session.dms[i].latestAt) setSession("dms", i, { unread: 0, readAt: Math.max(session.dms[i].readAt, time) });
  } else if (session.dmList.loaded) void loadDms();
}

export function resetDm() {
  dmOpens.clear();
  dmLoads.clear();
  dmSeq++;
  dmMarking.clear();
}
