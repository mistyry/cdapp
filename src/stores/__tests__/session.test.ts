import { beforeEach, expect, mock, test } from "bun:test";

type Call = { cmd: string; args: any; options?: any };
const calls: Call[] = [];
const pending: Array<{ cmd: string; args: any; resolve: (v: unknown) => void }> = [];
let auto: Record<string, (args: any) => unknown> = {};

// 可控的 invoke：默认立即返回，注册到 hold 的命令须手动放行。
const hold = new Set<string>();
mock.module("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: any, options?: any) => {
    calls.push({ cmd, args, options });
    if (hold.has(cmd)) return new Promise((resolve) => pending.push({ cmd, args, resolve }));
    return Promise.resolve(auto[cmd]?.(args) ?? null);
  },
}));
mock.module("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

const { retryAccess, enterVoice, voiceBlock, visible, pwPrompt, cancelPassword, session, setSession, selectGuild, selectChannel, openDm, send, canSend, logout, enter, retry, verify, discard, boot, skipRestore, loadGuilds, loadOlder, loadHistory, onGatewayStatus, onEvent, reconnectNow, ensureMembers, loadMoreMembers, refreshMembers, sendBlock, loadAccess, jumpTo, loadNewer, jumpLatest, plainContent, editMessage, deleteMessage, toggleReaction, isReacting, reactFailed, canEdit, canDelete, canReact, stageFiles, submit, cancelUpload, unstage, onUploadProgress, cardFileSrcs, loadDms, selectDm, openDmWith, loadDmOlder, loadDmHistory, dmKey, dmUnreadTotal, readCurrent, msgKey } = await import("../session");
const { ui, setUi, currentKey } = await import("../ui");

const VIEW = 1 << 11, SEND = 1 << 12;
const KIND = ["category", "text", "voice"] as const;
const raw = (id: string, name: string, type = 1, parent = "", level = 0) =>
  ({ id, name, kind: KIND[type] ?? "unknown", rawType: type, parentId: parent || null, level, limit: 0 });
const pg = (items: unknown[], end = true) => ({ items, end });
const flush = () => new Promise((r) => setTimeout(r, 0));
// 按命令与参数放行挂起的请求，便于构造任意返回顺序。
const release = (cmd: string, v: unknown, match: (a: any) => boolean = () => true) => {
  const i = pending.findIndex((p) => p.cmd === cmd && match(p.args));
  if (i < 0) throw new Error(`没有挂起的 ${cmd}`);
  pending.splice(i, 1)[0].resolve(v);
};

beforeEach(async () => {
  calls.length = 0;
  hold.clear();
  pending.length = 0;
  auto = { guild_access: ({ guildId }) => ({ guildId, ownerId: "o", master: false, myRoles: [], roles: [{ id: 0, name: "@全体成员", color: 0, position: 999, hoist: false, permissions: VIEW | SEND }] }), unread: () => [], messages: () => pg([]), channels: ({ guildId }) => (guildId === "gEmpty" ? [] : [raw(`${guildId}-voice`, "v", 2), raw(`${guildId}-t1`, "t1"), raw(`${guildId}-t2`, "t2")]) };
  await logout();
  setSession({
    phase: "ready", me: { id: "me", name: "me", online: true },
    guilds: [{ id: "A", name: "A", groups: [] }, { id: "B", name: "B", groups: [] }, { id: "gEmpty", name: "E", groups: [] }],
  });
});

test("进入服务器选中首个文字频道", async () => {
  await selectGuild("A");
  expect(ui.target).toEqual({ kind: "channel", guildId: "A", channelId: "A-t1" });
});

test("快速切换 A、B，A 的迟到结果不改变 B", async () => {
  hold.add("channels");
  const a = selectGuild("A");
  const b = selectGuild("B");
  release("channels", [raw("B-t1", "b")], (a) => a.guildId === "B"); // B 先返回
  await b;
  release("channels", [raw("A-t1", "a")], (a) => a.guildId === "A"); // A 迟到
  await a;
  expect(ui.view).toBe("B");
  expect(ui.target).toEqual({ kind: "channel", guildId: "B", channelId: "B-t1" });
});

test("迟到的历史消息不切走当前频道", async () => {
  await selectGuild("A");
  hold.add("messages");
  const slow = selectChannel("A-t2");
  const b = selectGuild("B");
  await flush();
  release("messages", pg([]), (a) => a.channelId === "B-t1");
  await b;
  release("messages", pg([]), (a) => a.channelId === "A-t2");
  await slow;
  await flush();
  expect(ui.target).toMatchObject({ channelId: "B-t1" });
});

test("切到私信入口后不能向旧频道发送", async () => {
  await selectGuild("A");
  openDm();
  expect(ui.target.kind).toBe("none");
  expect(canSend()).toBe(false);
  expect(send(ui.target, "hi")).toBe(false);
  expect(calls.some((c) => c.cmd === "send_message")).toBe(false);
});

test("空服务器没有发送目标", async () => {
  await selectGuild("A");
  await selectGuild("gEmpty");
  expect(ui.target.kind).toBe("none");
  expect(canSend()).toBe(false);
});

test("语音频道和未知频道不能成为发送目标", async () => {
  await selectGuild("A");
  await selectChannel("A-voice");
  await selectChannel("nope");
  expect(ui.target).toMatchObject({ channelId: "A-t1" });
  expect(canSend({ kind: "channel", guildId: "A", channelId: "A-voice" })).toBe(false);
});

test("发送结果写回发起时的频道", async () => {
  await selectGuild("A");
  const target = ui.target;
  hold.add("send_message");
  expect(send(target, "hello")).toBe(true);
  await selectChannel("A-t2");
  release("send_message", { msg_id: "m1", msg_timestamp: 1 });
  await flush();
  expect(calls.find((c) => c.cmd === "send_message")!.args.channelId).toBe("A-t1");
  expect(session.messages["A-t1"].map((m) => m.id)).toEqual(["m1"]);
  expect(session.messages["A-t2"]).toEqual([]);
});

test("引用与草稿按频道隔离，退出登录后清空", async () => {
  await selectGuild("A");
  setUi("replies", currentKey(), { id: "q", author: "x", text: "y" });
  setUi("drafts", currentKey(), "draft");
  await selectChannel("A-t2");
  expect(ui.replies[currentKey()]).toBeUndefined();
  expect(ui.drafts[currentKey()]).toBeUndefined();
  await selectChannel("A-t1");
  expect(ui.replies[currentKey()]?.id).toBe("q");
  await logout();
  expect(ui.target.kind).toBe("none");
  expect(Object.keys(ui.drafts)).toEqual([]);
  expect(Object.keys(ui.replies)).toEqual([]);
});

test("回到服务器时恢复上次频道", async () => {
  await selectGuild("A");
  await selectChannel("A-t2");
  await selectGuild("B");
  await selectGuild("A");
  expect(ui.target).toMatchObject({ channelId: "A-t2" });
});

test("退出登录后旧登录的迟到结果不写回", async () => {
  await logout();
  hold.add("guilds");
  auto.me = () => ({ id: "me", username: "me" });
  const p = enter();
  await flush();
  await logout();
  release("guilds", [{ id: "A", name: "A" }]);
  await p;
  expect(session.phase).toBe("login");
  expect(session.guilds).toEqual([]);
  expect(ui.target.kind).toBe("none");
});

test("旧账号的历史消息不写入新账号", async () => {
  await selectGuild("A");
  hold.add("messages");
  const p = selectChannel("A-t2");
  await logout();
  setSession({ phase: "ready", me: { id: "other", name: "o", online: true } });
  release("messages", pg([{ id: "old", author: { id: "x" }, content: "c" }]));
  await p;
  expect(session.messages["A-t2"]).toBeUndefined();
});

test("拉取历史期间发出的消息不被覆盖", async () => {
  await selectGuild("A");
  hold.add("messages");
  const p = selectChannel("A-t2");
  await flush();
  auto.send_message = () => ({ msg_id: "s1", msg_timestamp: 1 });
  expect(send({ kind: "channel", guildId: "A", channelId: "A-t2" }, "hi")).toBe(true);
  await flush();
  release("messages", pg([{ id: "h1", author: { id: "x" }, content: "c", create_at: 0 }]));
  await p;
  expect(session.messages["A-t2"].map((m) => m.id)).toEqual(["h1", "s1"]);
});

test("频道加载失败可见且可重试", async () => {
  auto.channels = () => { throw new Error("x"); };
  await selectGuild("A").catch(() => {});
  expect(session.guilds.find((g) => g.id === "A")?.failed).toBe(true);
});

const T1 = { kind: "channel", guildId: "A", channelId: "A-t1" } as const;
const box = () => session.outbox["A-t1"] ?? [];
const fail = (kind: string) => () => { throw { kind, message: kind }; };

test("发送期间显示发送中，成功后只留一条消息", async () => {
  await selectGuild("A");
  hold.add("send_message");
  send(T1, "x");
  expect(box().map((o) => o.status)).toEqual(["sending"]);
  release("send_message", { msg_id: "m1", msg_timestamp: 5 });
  await flush();
  expect(box()).toEqual([]);
  expect(session.messages["A-t1"].map((m) => m.id)).toEqual(["m1"]);
});

test("确定未发出时为失败，可重试且不重复", async () => {
  await selectGuild("A");
  auto.send_message = fail("not_sent");
  send(T1, "x", { id: "q", author: "a", text: "t" });
  await flush();
  expect(box()[0]).toMatchObject({ status: "failed", text: "x", reply: { id: "q" } });
  auto.send_message = () => ({ msg_id: "m1", msg_timestamp: 5 });
  retry("A-t1", box()[0].localId);
  await flush();
  expect(box()).toEqual([]);
  expect(calls.filter((c) => c.cmd === "send_message").slice(-1)[0].args.quote).toBe("q");
  expect(session.messages["A-t1"].length).toBe(1);
});

test("结果不确定时不允许直接重试，核对到已送达则不重发", async () => {
  await selectGuild("A");
  auto.send_message = fail("uncertain");
  send(T1, "same");
  await flush();
  const o = box()[0];
  expect(o.status).toBe("uncertain");
  retry("A-t1", o.localId);
  await flush();
  expect(calls.filter((c) => c.cmd === "send_message").length).toBe(1);
  auto.messages = () => pg([{ id: "srv", author: { id: "me" }, content: "same", create_at: o.createAt + 10 }]);
  await verify("A-t1", o.localId);
  expect(box()).toEqual([]);
  expect(session.messages["A-t1"].map((m) => m.id)).toEqual(["srv"]);
  expect(calls.filter((c) => c.cmd === "send_message").length).toBe(1);
});

test("核对未找到时转为可重发", async () => {
  await selectGuild("A");
  auto.send_message = fail("uncertain");
  send(T1, "lost");
  await flush();
  await verify("A-t1", box()[0].localId);
  expect(box()[0]).toMatchObject({ status: "failed" });
});

test("无法识别的错误按不确定处理", async () => {
  await selectGuild("A");
  auto.send_message = () => { throw "boom"; };
  send(T1, "x");
  await flush();
  expect(box()[0].status).toBe("uncertain");
});

test("放弃失败消息时交回输入框且不覆盖新草稿", async () => {
  await selectGuild("A");
  auto.send_message = fail("not_sent");
  send(T1, "first", { id: "q", author: "a", text: "t" });
  send(T1, "second");
  await flush();
  discard("A-t1", box()[0].localId, true);
  expect(ui.drafts["channel:A-t1"]).toBe("first");
  expect(ui.replies["channel:A-t1"]?.id).toBe("q");
  discard("A-t1", box()[0].localId, true);
  expect(ui.drafts["channel:A-t1"]).toBe("first");
});

test("发送中切换频道，结果仍在原频道", async () => {
  await selectGuild("A");
  auto.send_message = fail("not_sent");
  send(T1, "x");
  await selectChannel("A-t2");
  await flush();
  expect(session.outbox["A-t2"] ?? []).toEqual([]);
  expect(box()[0].status).toBe("failed");
});

test("退出登录清空待发列表，迟到响应不写回", async () => {
  await selectGuild("A");
  hold.add("send_message");
  send(T1, "x");
  await logout();
  release("send_message", { msg_id: "m1" });
  await flush();
  expect(session.outbox).toEqual({});
  expect(session.messages).toEqual({});
});

test("历史消息以 rong_id 为标识，与发送响应和推送一致", async () => {
  await selectGuild("A");
  auto.messages = () => pg([{ id: "internal24", rong_id: "uuid-1", author: { id: "x" }, content: "c", create_at: 1 }]);
  await selectChannel("A-t2");
  expect(session.messages["A-t2"].map((m) => m.id)).toEqual(["uuid-1"]);
});

// M1-03 登录与会话生命周期

const reject = (kind: string, message = "x") => Promise.reject({ kind, message });

test("无保存凭据时进入登录页", async () => {
  auto.restore_session = () => ({ status: "none" });
  await boot();
  expect(session.phase).toBe("login");
  expect(session.notice).toBeNull();
});

test("重启恢复成功后进入并加载服务器", async () => {
  auto.restore_session = () => ({ status: "ready", me: { id: "u1", username: "u1" } });
  auto.guilds = () => [{ id: "A", name: "A" }];
  await boot();
  expect(session.phase).toBe("ready");
  expect(session.me?.id).toBe("u1");
  expect(ui.target).toEqual({ kind: "channel", guildId: "A", channelId: "A-t1" });
});

test("无网络启动保留凭据，可重试或改为扫码", async () => {
  auto.restore_session = () => reject("not_sent", "连接失败");
  calls.length = 0;
  await boot();
  expect(session.phase).toBe("restore_failed");
  expect(calls.some((c) => c.cmd === "logout")).toBe(false);
  auto.restore_session = () => ({ status: "ready", me: { id: "u1" } });
  auto.guilds = () => [];
  await boot();
  expect(session.phase).toBe("ready");
  await logout();
  auto.restore_session = () => reject("uncertain");
  await boot();
  skipRestore();
  expect(session.phase).toBe("login");
});

test("恢复时凭据失效进入登录页并提示原因", async () => {
  auto.restore_session = () => ({ status: "expired", message: "请重新登录" });
  await boot();
  expect(session.phase).toBe("login");
  expect(session.notice).toContain("请重新登录");
});

test("恢复期间放弃恢复，迟到结果不进入旧会话", async () => {
  hold.add("restore_session");
  const b = boot();
  skipRestore();
  release("restore_session", { status: "ready", me: { id: "u1" } });
  await b;
  expect(session.phase).toBe("login");
  expect(session.me).toBeNull();
});

test("任一请求返回凭据失效时回到登录页并清空账号数据", async () => {
  await selectGuild("A");
  auto.messages = () => reject("unauthorized", "请重新登录");
  setSession("messages", {});
  await selectChannel("A-t2");
  expect(session.phase).toBe("login");
  expect(session.me).toBeNull();
  expect(session.guilds).toEqual([]);
  expect(ui.target).toEqual({ kind: "none" });
  expect(session.notice).toContain("登录已失效");
});

test("凭据失效后迟到的结果不写回", async () => {
  await selectGuild("A");
  hold.add("send_message");
  send(ui.target, "hi");
  auto.messages = () => reject("unauthorized");
  setSession("messages", {});
  await selectChannel("A-t2");
  release("send_message", { msg_id: "m1", msg_timestamp: 1 });
  await flush();
  expect(session.messages).toEqual({});
  expect(session.outbox).toEqual({});
});

test("其他接口错误不结束登录", async () => {
  await selectGuild("A");
  auto.messages = () => reject("rejected", "无权限");
  setSession("messages", {});
  await selectChannel("A-t2");
  expect(session.phase).toBe("ready");
});

test("登录后取用户失败回到登录页并说明原因", async () => {
  await logout();
  auto.me = () => reject("not_sent", "连接失败");
  await enter();
  expect(session.phase).toBe("login");
  expect(session.notice).toContain("连接失败");
});

test("服务器列表加载失败可见且可重试", async () => {
  await logout();
  auto.guilds = () => reject("not_sent");
  await enter({ id: "u1" });
  expect(session.phase).toBe("ready");
  expect(session.loadFailed).toBe(true);
  auto.guilds = () => [{ id: "A", name: "A" }];
  await loadGuilds();
  expect(session.loadFailed).toBe(false);
  expect(ui.view).toBe("A");
});

test("退出后重新登录，旧账号的进入流程不写回", async () => {
  await logout();
  hold.add("me");
  const old = enter();
  await logout();
  auto.guilds = () => [];
  await enter({ id: "u2" });
  release("me", { id: "u1" });
  await old;
  expect(session.me?.id).toBe("u2");
});

test("旧登录的迟到 401 不结束新登录", async () => {
  await logout();
  auto.guilds = () => [];
  await enter({ id: "u1" });
  hold.add("reaction");
  const { api } = await import("../../lib/api");
  const late = api.reaction("m", "e", true).catch(() => {});
  await logout();
  await enter({ id: "u2" });
  const i = pending.findIndex((p) => p.cmd === "reaction");
  pending.splice(i, 1)[0].resolve(Promise.reject({ kind: "unauthorized", message: "x" }));
  await late;
  expect(session.phase).toBe("ready");
  expect(session.me?.id).toBe("u2");
});

test("凭据失效时退出语音", async () => {
  await selectGuild("A");
  auto.messages = () => reject("unauthorized");
  setSession("messages", {});
  await selectChannel("A-t2");
  expect(calls.some((c) => c.cmd === "voice_leave")).toBe(true);
});

test("退出登录关闭设置页", async () => {
  setUi("settings", "account");
  await logout();
  expect(ui.settings).toBeNull();
  expect(calls.some((c) => c.cmd === "logout")).toBe(true);
});

test("频道分组按 level 排序，顶层频道与分组交错，孤立频道归入顶层", async () => {
  const { toGroups } = await import("../session");
  const g = toGroups([
    raw("c2", "分组2", 0, "", 5), raw("x", "x", 1, "c2", 2), raw("w", "w", 1, "c2", 1),
    raw("top1", "top1", 1, "", 1), raw("c1", "分组1", 0, "", 3), raw("v", "v", 2, "c1", 0),
    raw("orphan", "o", 1, "gone", 4), raw("top0", "top0", 1, "", 0), raw("odd", "", 7, "c1", 1),
  ]);
  expect(g.map((x) => [x.loose ? "_" : x.name, x.channels.map((c) => c.id)])).toEqual([
    ["_", ["top0", "top1"]], ["分组1", ["v", "odd"]], ["_", ["orphan"]], ["分组2", ["w", "x"]],
  ]);
  expect(g[1].channels[1]).toMatchObject({ type: "unknown", name: "未知频道（类型 7）" });
  expect(toGroups([])).toEqual([]);
});

test("未知类型频道不会被选为发送目标", async () => {
  auto.channels = () => [raw("A-odd", "odd", 7), raw("A-t", "t")];
  await selectGuild("A");
  expect(ui.target).toEqual({ kind: "channel", guildId: "A", channelId: "A-t" });
  await selectChannel("A-odd");
  expect(ui.target).toEqual({ kind: "channel", guildId: "A", channelId: "A-t" });
});

test("被限流的发送标为失败而非不确定", async () => {
  await selectGuild("A");
  auto.send_message = () => { throw { kind: "rate_limited", message: "请求过于频繁" }; };
  send(ui.target, "hi");
  await flush(); await flush();
  expect(session.outbox["A-t1"]?.[0]).toMatchObject({ status: "failed" });
});

// M2-02 历史翻页

const m = (id: string, t: number, author = "x") => ({ id: `i${id}`, rong_id: id, author: { id: author }, content: id, create_at: t });
const ids = (c: string) => session.messages[c].map((x) => x.id);

test("上翻以最早消息为游标，前插去重且不覆盖期间的新消息", async () => {
  auto.messages = () => pg([m("c", 30), m("d", 40)], false);
  await selectGuild("A");
  expect(ids("A-t1")).toEqual(["c", "d"]);
  expect(session.history["A-t1"]).toEqual({ end: false, loading: false, failed: false });
  hold.add("messages");
  const p = loadOlder("A-t1");
  expect(session.history["A-t1"].loading).toBe(true);
  expect(calls[calls.length - 1].args).toEqual({ channelId: "A-t1", before: "c" });
  setSession("messages", "A-t1", (l) => [...l, { ...l[1], id: "e", createAt: 50 }]); // 期间到达的新消息
  release("messages", pg([m("a", 10), m("b", 20), m("c", 30)], false));
  await p;
  expect(ids("A-t1")).toEqual(["a", "b", "c", "d", "e"]);
  expect(session.history["A-t1"].end).toBe(false);
});

test("历史结束后不再请求；空历史直接为结束", async () => {
  auto.messages = () => pg([m("c", 30)], true);
  await selectGuild("A");
  expect(session.history["A-t1"].end).toBe(true);
  const n = calls.length;
  await loadOlder("A-t1");
  expect(calls.length).toBe(n);
  auto.messages = () => pg([], true);
  await selectChannel("A-t2");
  expect(session.messages["A-t2"]).toEqual([]);
  expect(session.history["A-t2"].end).toBe(true);
});

test("上翻失败可重试，期间不重复请求", async () => {
  auto.messages = () => pg([m("c", 30)], false);
  await selectGuild("A");
  auto.messages = () => Promise.reject({ kind: "not_sent", message: "网络错误" });
  await loadOlder("A-t1");
  expect(session.history["A-t1"]).toEqual({ end: false, loading: false, failed: true });
  expect(ids("A-t1")).toEqual(["c"]);
  hold.add("messages");
  const p = loadOlder("A-t1");
  const n = calls.length;
  void loadOlder("A-t1");
  expect(calls.length).toBe(n);
  release("messages", pg([m("b", 20)], true));
  await p;
  expect(ids("A-t1")).toEqual(["b", "c"]);
  expect(session.history["A-t1"]).toEqual({ end: true, loading: false, failed: false });
});

test("首次历史加载失败标记 failed，重试后恢复", async () => {
  auto.messages = () => Promise.reject({ kind: "not_sent", message: "网络错误" });
  await selectGuild("A");
  expect(session.history["A-t1"].failed).toBe(true);
  expect(session.messages["A-t1"]).toBeUndefined();
  auto.messages = () => pg([m("c", 30)]);
  await loadHistory("A-t1");
  expect(ids("A-t1")).toEqual(["c"]);
  expect(session.history["A-t1"].failed).toBe(false);
});

test("退出登录后迟到的上翻结果被丢弃", async () => {
  auto.messages = () => pg([m("c", 30)], false);
  await selectGuild("A");
  hold.add("messages");
  const p = loadOlder("A-t1");
  await logout();
  release("messages", pg([m("b", 20)]));
  await p;
  expect(session.messages["A-t1"]).toBeUndefined();
  expect(session.history["A-t1"]).toBeUndefined();
});

test("补拉最新页不清除进行中的上翻状态，不产生重复上翻", async () => {
  auto.messages = () => pg([m("b", 20), m("c", 30)], false);
  await selectGuild("A");
  hold.add("messages");
  const older = loadOlder("A-t1");
  const latest = loadHistory("A-t1");
  expect(session.history["A-t1"].loading).toBe(true);
  const n = calls.length;
  void loadOlder("A-t1");
  expect(calls.length).toBe(n);
  release("messages", pg([m("c", 30), m("d", 40)], false), (a) => !a.before);
  await latest;
  expect(session.history["A-t1"].loading).toBe(true);
  release("messages", pg([m("a", 10)], true));
  await older;
  expect(ids("A-t1")).toEqual(["a", "b", "c", "d"]);
});

test("断线期间超过一页的新消息：向前补齐到衔接处，不留缺口", async () => {
  auto.messages = () => pg([m("a", 10), m("b", 20)], false);
  await selectGuild("A");
  const pages: Record<string, unknown> = { "": pg([m("e", 50), m("f", 60)], false), e: pg([m("c", 30), m("d", 40)], false), c: pg([m("a", 10), m("b", 20)], false) };
  auto.messages = ({ before }) => pages[before ?? ""];
  await loadHistory("A-t1");
  expect(ids("A-t1")).toEqual(["a", "b", "c", "d", "e", "f"]);
});

test("断档超过上限时以最新页重建列表，上翻可继续", async () => {
  auto.messages = () => pg([m("a", 10)], false);
  await selectGuild("A");
  let k = 1000;
  auto.messages = () => pg([m(`n${k}`, k--)], false);
  await loadHistory("A-t1");
  expect(session.messages["A-t1"].some((x) => x.id === "a")).toBe(false);
  expect(session.history["A-t1"].end).toBe(false);
});

test("首次加载未返回时重复进入频道，不丢失暂存的实时消息", async () => {
  hold.add("messages");
  const p1 = loadHistory("A-t1");
  setSession("guilds", 0, { loaded: true, groups: [{ id: "g", name: "", channels: [{ id: "A-t1", name: "t", type: "text" }] }] });
  const p2 = loadHistory("A-t1");
  expect(pending.filter((x) => x.cmd === "messages").length).toBe(1);
  release("messages", pg([m("a", 10)]));
  await Promise.all([p1, p2]);
  expect(ids("A-t1")).toEqual(["a"]);
});

test("已有列表时重新拉取最新页只合并，保留已加载的更早消息与结束状态", async () => {
  auto.messages = () => pg([m("b", 20), m("c", 30)], false);
  await selectGuild("A");
  auto.messages = () => pg([m("a", 10)], true);
  await loadOlder("A-t1");
  auto.messages = () => pg([m("c", 30), m("d", 40)], false);
  await loadHistory("A-t1");
  expect(ids("A-t1")).toEqual(["a", "b", "c", "d"]);
  expect(session.history["A-t1"].end).toBe(true);
});

// 网关恢复与补偿

const ready = (kind: "new" | "resumed" | "rebuilt") => onGatewayStatus({ state: "ready", kind });
const down = () => onGatewayStatus({ state: "disconnected", reason: "x", retry_in_ms: 1000 });
const push = (channelId: string, msgId: string, at = 99) =>
  onEvent({ type: "CHANNEL_MSG", channelType: "GROUP", toUserId: channelId, msgId, msgTimestamp: at, fromUserId: "u", content: JSON.stringify({ content: msgId }), extra: { author: { id: "u" } } });
const activations = () => calls.filter((c) => c.cmd === "activate_guild").map((c) => c.args.guildId);
const fetchesOf = (from: number) => calls.slice(from).filter((c) => c.cmd === "messages").map((c) => c.args.channelId);

test("续传成功不重新激活也不补拉，状态回到在线", async () => {
  ready("new");
  await selectGuild("A");
  down();
  expect(session.gateway).toBe("reconnecting");
  const n = calls.length;
  ready("resumed");
  expect(session.gateway).toBe("online");
  expect(session.recovery).toBe("resumed");
  expect(calls.slice(n).filter((c) => c.cmd !== "client_log")).toEqual([]);
});

test("续传失败重建后：重新激活所有进入过的服务器，补拉全部已缓存频道且当前频道优先", async () => {
  ready("new");
  await selectGuild("A");
  await selectChannel("A-t2");
  await selectGuild("B");
  expect(activations()).toEqual(["A", "B"]);
  auto.messages = ({ channelId, before }) => (before ? pg([]) : pg([m(`${channelId}-new`, 500)], false));
  const n = calls.length;
  down();
  ready("rebuilt");
  expect(session.gateway).toBe("syncing");
  await flush();
  await flush();
  expect(activations().slice(2).sort()).toEqual(["A", "B"]);
  const order = fetchesOf(n);
  expect(order[0]).toBe("B-t1");
  expect(new Set(order)).toEqual(new Set(["A-t1", "A-t2", "B-t1"]));
  expect(session.gateway).toBe("online");
  expect(session.recovery).toBe("rebuilt");
  expect(ids("A-t2")).toContain("A-t2-new");
});

test("补偿时单个频道失败不影响其余频道，结果为部分失败", async () => {
  ready("new");
  await selectGuild("A");
  await selectChannel("A-t2");
  auto.messages = ({ channelId }) => {
    if (channelId === "A-t1") throw { kind: "not_sent", message: "net" };
    return pg([m(`${channelId}-new`, 500)], false);
  };
  ready("rebuilt");
  for (let i = 0; i < 5; i++) await flush();
  expect(ids("A-t2")).toContain("A-t2-new");
  expect(session.recovery).toBe("partial");
  expect(session.gateway).toBe("online");
});

test("补偿期间再次断线则停止补偿，不误报已同步", async () => {
  ready("new");
  await selectGuild("A");
  await selectChannel("A-t2");
  hold.add("messages");
  ready("rebuilt");
  await flush();
  down();
  release("messages", pg([]));
  for (let i = 0; i < 5; i++) await flush();
  expect(session.gateway).toBe("reconnecting");
  expect(pending.filter((p) => p.cmd === "messages").length).toBe(0);
});

test("补偿期间退出登录，迟到结果不写回", async () => {
  ready("new");
  await selectGuild("A");
  hold.add("messages");
  ready("rebuilt");
  await flush();
  await logout();
  release("messages", pg([m("late", 1)]));
  await flush();
  expect(session.messages).toEqual({});
  expect(session.gateway).toBe("offline");
});

test("重复下发的消息不重复显示，也不重复计未读", async () => {
  await selectGuild("A");
  push("A-t1", "x");
  push("A-t1", "x");
  expect(ids("A-t1").filter((i) => i === "x").length).toBe(1);
  push("A-t2", "y");
  push("A-t2", "y");
  expect(session.unread["A-t2"]?.unread).toBe(true);
});

test("首次就绪激活就绪前已进入的服务器；仅在等待重连时可立即重连", async () => {
  await selectGuild("A");
  expect(activations()).toEqual([]);
  ready("new");
  await flush();
  expect(activations()).toEqual(["A"]);
  reconnectNow();
  expect(calls.some((c) => c.cmd === "gateway_reconnect")).toBe(false);
  down();
  reconnectNow();
  expect(calls.some((c) => c.cmd === "gateway_reconnect")).toBe(true);
});

test("首个会话补拉就绪前已打开的全部频道，不显示恢复提示", async () => {
  await selectGuild("A");
  await selectChannel("A-t2");
  const n = calls.length;
  ready("new");
  for (let i = 0; i < 5; i++) await flush();
  expect(new Set(fetchesOf(n))).toEqual(new Set(["A-t1", "A-t2"]));
  expect(session.gateway).toBe("online");
  expect(session.recovery).toBe(null);
});

test("补偿被断线打断后，即使续传成功也重新补偿", async () => {
  ready("new");
  await selectGuild("A");
  await selectChannel("A-t2");
  for (let i = 0; i < 5; i++) await flush();
  hold.add("messages");
  ready("rebuilt");
  await flush();
  down();
  release("messages", pg([]));
  hold.clear();
  const n = calls.length;
  ready("resumed");
  for (let i = 0; i < 5; i++) await flush();
  expect(new Set(fetchesOf(n))).toEqual(new Set(["A-t1", "A-t2"]));
  expect(session.recovery).toBe("rebuilt");
});

test("服务器重新激活失败计为部分失败", async () => {
  ready("new");
  await selectGuild("A");
  for (let i = 0; i < 5; i++) await flush();
  auto.activate_guild = () => { throw { kind: "not_sent", message: "net" }; };
  ready("rebuilt");
  for (let i = 0; i < 5; i++) await flush();
  expect(session.recovery).toBe("partial");
});

test("断档重建列表时保留拉取期间到达的实时消息", async () => {
  auto.messages = () => pg([m("a", 10)], false);
  await selectGuild("A");
  hold.add("messages");
  const p = loadHistory("A-t1");
  await flush();
  push("A-t1", "live", 5000);
  hold.clear();
  let k = 1000;
  auto.messages = () => pg([m(`n${k}`, k--)], false);
  release("messages", pg([m(`n${k}`, k--)], false));
  await p;
  expect(ids("A-t1")).toContain("live");
  expect(ids("A-t1")).not.toContain("a");
});

// 未读、提及与跨端已读

const sys = (type: string, body: unknown) => onEvent({ type: "SYS_MSG", content: JSON.stringify({ type, body }) });
const pushFrom = (channelId: string, msgId: string, author: string, at = 99, guild = "A") =>
  onEvent({ type: "CHANNEL_MSG", channelType: "GROUP", toUserId: channelId, msgId, msgTimestamp: at, fromUserId: author,
    content: JSON.stringify({ content: msgId, extra: JSON.stringify({ type: 9, guild_id: guild, author: { id: author }, mention: [] }) }) });
const marks = () => calls.filter((c) => c.cmd === "mark_read");

test("登录后拉取全部服务器未读，未进入过的服务器也有汇总", async () => {
  await logout();
  auto.me = () => ({ id: "me", username: "me" });
  auto.guilds = () => [{ id: "A", name: "A" }, { id: "B", name: "B" }];
  auto.unread = () => [
    { guildId: "B", channelId: "B-t1", unread: true, mentions: 0, readAt: 1 },
    { guildId: "B", channelId: "B-t2", unread: true, mentions: 2, readAt: 1 },
  ];
  await enter();
  const { guildUnread } = await import("../session");
  expect(guildUnread("B")).toEqual({ unread: true, mentions: 2 });
  expect(guildUnread("A")).toEqual({ unread: false, mentions: 0 });
});

test("后台频道收到消息记未读；提及数来自服务端事件；extra 为 JSON 字符串时可解析", async () => {
  await selectGuild("A");
  pushFrom("A-t2", "p1", "u");
  expect(session.unread["A-t2"]).toMatchObject({ guildId: "A", unread: true, mentions: 0 });
  sys("unread_count_changed", { type: 2, guild_id: "A", channel_id: "A-t2", mention_count: 1, msg_time: 100 });
  expect(session.unread["A-t2"].mentions).toBe(1);
});

test("本账号其他设备发送的消息不计未读", async () => {
  await selectGuild("A");
  pushFrom("A-t2", "own", "me");
  expect(session.unread["A-t2"]).toBeUndefined();
});

test("未激活服务器的未读事件同样生效", async () => {
  sys("unread_count_changed", { type: 2, guild_id: "B", channel_id: "B-t9", message_count: 1, msg_time: 5 });
  const { guildUnread } = await import("../session");
  expect(guildUnread("B").unread).toBe(true);
});

test("另一设备已读清除未读；已读时间早于最新消息则保留", async () => {
  await selectGuild("A");
  pushFrom("A-t2", "a1", "u", 100);
  pushFrom("A-t2", "a2", "u", 200);
  sys("mark_as_batchread", { channel: [{ guild_id: "A", channel_id: "A-t2", time: 150 }] });
  expect(session.unread["A-t2"]?.unread).toBe(true);
  sys("mark_as_batchread", { channel: [{ guild_id: "A", channel_id: "A-t2", time: 200 }] });
  expect(session.unread["A-t2"]).toBeUndefined();
});

test("切到频道阅读后标记已读，成功后才清除；失败保留未读", async () => {
  await selectGuild("A");
  pushFrom("A-t2", "b1", "u", 100);
  auto.messages = () => pg([m("b1", 100, "u")]);
  auto.mark_read = () => { throw { kind: "not_sent", message: "net" }; };
  await selectChannel("A-t2");
  await flush();
  expect(marks().slice(-1)[0]?.args).toMatchObject({ channelId: "A-t2", msgId: "b1", time: 100 });
  expect(session.unread["A-t2"]?.unread).toBe(true);
  auto.mark_read = () => ({});
  const { readCurrent } = await import("../session");
  readCurrent();
  await flush();
  expect(session.unread["A-t2"]).toBeUndefined();
});

test("阅读旧消息（不在底部）时不标记已读，回到底部后标记", async () => {
  await selectGuild("A");
  setUi("atBottom", false);
  auto.messages = () => pg([m("c1", 100, "u")]);
  pushFrom("A-t2", "c1", "u", 100);
  await selectChannel("A-t2");
  expect(marks().length).toBe(0);
  setUi("atBottom", true);
  const { readCurrent } = await import("../session");
  readCurrent();
  await flush();
  expect(marks().length).toBe(1);
  expect(session.unread["A-t2"]).toBeUndefined();
});

test("当前频道正在阅读时收到消息直接标记已读", async () => {
  await selectGuild("A");
  pushFrom("A-t1", "d1", "u", 300);
  await flush();
  expect(marks().slice(-1)[0]?.args).toMatchObject({ channelId: "A-t1", msgId: "d1" });
  expect(session.unread["A-t1"]).toBeUndefined();
});

test("新会话补偿重新拉取未读；拉取期间到达的未读不被旧快照覆盖", async () => {
  ready("new");
  await selectGuild("A");
  await flush();
  hold.add("unread");
  ready("rebuilt");
  for (let i = 0; i < 5; i++) await flush();
  pushFrom("A-t2", "e1", "u", 400);
  release("unread", [{ guildId: "B", channelId: "B-t1", unread: true, mentions: 1, readAt: 0 }]);
  for (let i = 0; i < 5; i++) await flush();
  expect(session.unread["A-t2"]?.unread).toBe(true);
  expect(session.unread["B-t1"]?.mentions).toBe(1);
  expect(session.gateway).toBe("online");
});

test("未读拉取失败保留本地未读并计为部分失败", async () => {
  ready("new");
  await selectGuild("A");
  pushFrom("A-t2", "f1", "u", 500);
  auto.unread = () => { throw { kind: "not_sent", message: "net" }; };
  ready("rebuilt");
  for (let i = 0; i < 8; i++) await flush();
  expect(session.unread["A-t2"]?.unread).toBe(true);
  expect(session.recovery).toBe("partial");
});

test("重复下发的提及事件只计一次", async () => {
  await selectGuild("A");
  const ev = { type: 2, guild_id: "A", channel_id: "A-t2", mention_count: 1, msg_time: 700 };
  sys("unread_count_changed", ev);
  sys("unread_count_changed", ev);
  expect(session.unread["A-t2"].mentions).toBe(1);
});

test("已读后迟到的旧未读事件不恢复未读", async () => {
  await selectGuild("A");
  const ev = { type: 2, guild_id: "A", channel_id: "A-t2", message_count: 1, msg_time: 800 };
  sys("unread_count_changed", ev);
  sys("mark_as_batchread", { channel: [{ guild_id: "A", channel_id: "A-t2", time: 800 }] });
  expect(session.unread["A-t2"]).toBeUndefined();
  sys("unread_count_changed", ev);
  expect(session.unread["A-t2"]).toBeUndefined();
});

test("阅读中收到消息但标记已读失败，离开后仍显示未读与提及", async () => {
  await selectGuild("A");
  auto.mark_read = () => { throw { kind: "not_sent", message: "net" }; };
  pushFrom("A-t1", "g1", "u", 900);
  sys("unread_count_changed", { type: 2, guild_id: "A", channel_id: "A-t1", mention_count: 1, msg_time: 900 });
  await flush();
  await selectChannel("A-t2");
  expect(session.unread["A-t1"]).toMatchObject({ unread: true, mentions: 1 });
});

// M3-01 成员与权限

const access = (guildId: string, p: Partial<{ myRoles: number[]; everyone: number; roles: any[]; master: boolean }> = {}) => ({
  guildId, ownerId: "o", master: p.master ?? false, myRoles: p.myRoles ?? [],
  roles: [{ id: 0, name: "", color: 0, position: 999, hoist: false, permissions: p.everyone ?? (VIEW | SEND) }, ...(p.roles ?? [])],
});
const withOw = (id: string, name: string, overwrites: any[]) => ({ ...raw(id, name), overwrites, userOverwrites: [] });
const mem = (id: string, online = true, nickname = "") => ({ id, username: `u${id}`, nickname, identifyNum: "0001", avatar: "", online, bot: false, roles: [], color: 0 });
const mpage = (items: any[], page = 1, pageTotal = 1, online = 1, offline = 1) => ({ items, page, pageTotal, total: online + offline, online, offline });

test("隐藏无查看权限的频道，进入服务器跳过它", async () => {
  auto.channels = () => [withOw("A-hidden", "h", [{ roleId: 0, allow: 0, deny: VIEW }]), raw("A-t1", "t1")];
  await selectGuild("A");
  expect(ui.target).toMatchObject({ channelId: "A-t1" });
  await selectChannel("A-hidden");
  expect(ui.target).toMatchObject({ channelId: "A-t1" });
});

test("角色覆盖允许查看时可见", async () => {
  auto.guild_access = ({ guildId }) => access(guildId, { myRoles: [5], roles: [{ id: 5, name: "r", color: 0, position: 1, hoist: false, permissions: 0 }] });
  auto.channels = () => [withOw("A-priv", "p", [{ roleId: 0, allow: 0, deny: VIEW }, { roleId: 5, allow: VIEW, deny: 0 }])];
  await selectGuild("A");
  expect(ui.target).toMatchObject({ channelId: "A-priv" });
  expect(canSend()).toBe(true);
});

test("无发送权限时不能发送且给出原因", async () => {
  auto.channels = () => [withOw("A-ro", "ro", [{ roleId: 0, allow: 0, deny: SEND }])];
  await selectGuild("A");
  expect(ui.target).toMatchObject({ channelId: "A-ro" });
  expect(sendBlock()).toBe("denied");
  expect(send(ui.target, "hi")).toBe(false);
  expect(calls.some((c) => c.cmd === "send_message")).toBe(false);
});

test("权限依据加载中或失败时不视为已授权，重试后恢复", async () => {
  hold.add("guild_access");
  const p = selectGuild("A");
  await flush();
  expect(sendBlock({ kind: "channel", guildId: "A", channelId: "A-t1" })).toBe("loading");
  release("guild_access", Promise.reject("网络错误"));
  await p;
  expect(session.access.A.status).toBe("failed");
  // 依据失败时不选中频道，也不读取历史。
  expect(ui.target).toEqual({ kind: "none" });
  expect(sendBlock({ kind: "channel", guildId: "A", channelId: "A-t1" })).toBe("failed");
  expect(canSend()).toBe(false);
  expect(calls.some((c) => c.cmd === "messages")).toBe(false);
  hold.clear();
  await retryAccess("A");
  expect(ui.target).toMatchObject({ channelId: "A-t1" });
  expect(sendBlock()).toBeNull();
});

test("刷新权限失败保留已有依据", async () => {
  await selectGuild("A");
  auto.guild_access = () => Promise.reject("x");
  await loadAccess("A");
  expect(session.access.A.status).toBe("ready");
  expect(canSend()).toBe(true);
});

test("退出登录后迟到的权限结果被丢弃", async () => {
  hold.add("guild_access");
  const p = selectGuild("A");
  await flush();
  await logout();
  release("guild_access", access("A"));
  await p;
  expect(session.access).toEqual({});
});

test("成员列表来自成员接口，包含未发言成员，按在线分组计数", async () => {
  auto.members = () => mpage([mem("1"), mem("2", false, "昵称")], 1, 1, 7, 9);
  await selectGuild("A");
  await ensureMembers();
  expect(session.members?.items.map((m) => m.id)).toEqual(["1", "2"]);
  expect([session.members?.online, session.members?.offline]).toEqual([7, 9]);
  expect(calls.find((c) => c.cmd === "members")?.args).toEqual({ guildId: "A", channelId: "A-t1", page: 1 });
});

test("成员翻页去重，到末页停止", async () => {
  auto.members = ({ page }) => (page === 1 ? mpage([mem("1"), mem("2")], 1, 2) : mpage([mem("2"), mem("3")], 2, 2));
  await selectGuild("A");
  await ensureMembers();
  await loadMoreMembers();
  await loadMoreMembers();
  expect(session.members?.items.map((m) => m.id)).toEqual(["1", "2", "3"]);
  expect(calls.filter((c) => c.cmd === "members").length).toBe(2);
});

test("切换频道丢弃旧成员列表与迟到结果", async () => {
  await selectGuild("A");
  hold.add("members");
  const a = ensureMembers();
  await selectGuild("B");
  const b = ensureMembers();
  expect(session.members?.guildId).toBe("B");
  expect(session.members?.items).toEqual([]);
  release("members", mpage([mem("a")]), (x) => x.guildId === "A");
  await a;
  release("members", mpage([mem("b")]), (x) => x.guildId === "B");
  await b;
  expect(session.members?.items.map((m) => m.id)).toEqual(["b"]);
});

test("成员加载失败可重试", async () => {
  auto.members = () => Promise.reject("x");
  await selectGuild("A");
  await ensureMembers();
  expect(session.members?.failed).toBe(true);
  auto.members = () => mpage([mem("1")]);
  await refreshMembers();
  expect(session.members?.items.length).toBe(1);
});

const sysTo = (target: string, type: string, body: any) =>
  onEvent({ type: "SYS_MSG", channelType: "GROUP", toUserId: target, msgId: `${type}-${Math.random()}`, content: JSON.stringify({ type, body }) });

test("服务器昵称变化更新成员；其他服务器的事件不影响", async () => {
  auto.members = () => mpage([mem("42", true, "旧")]);
  await selectGuild("A");
  await ensureMembers();
  sysTo("B", "user_state_change", { user_id: 42, nickname: "错" });
  expect(session.members?.items[0].nickname).toBe("旧");
  sysTo("A", "user_state_change", { user_id: 42, nickname: "新" });
  expect(session.members?.items[0].nickname).toBe("新");
});

test("成员加入退出与角色变化重新拉取；本人角色变化刷新权限", async () => {
  let n = 0;
  auto.members = () => mpage([mem(String(++n))]);
  await selectGuild("A");
  await ensureMembers();
  sysTo("A", "joined_guild", { user_id: "9" });
  await flush();
  expect(session.members?.items[0].id).toBe("2");
  const before = calls.filter((c) => c.cmd === "guild_access").length;
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: VIEW });
  sysTo("A", "user_role_change", { guild_id: "A", user_id: "me", roles: [] });
  await flush();
  expect(calls.filter((c) => c.cmd === "guild_access").length).toBe(before + 1);
  expect(sendBlock()).toBe("denied");
});

test("频道覆盖变化后重新拉取频道列表", async () => {
  await selectGuild("A");
  auto.channels = () => [withOw("A-t1", "t1", [{ roleId: 0, allow: 0, deny: SEND }])];
  sysTo("A", "updated_channel", { id: "A-t1", guild_id: "A" });
  await flush();
  expect(sendBlock()).toBe("denied");
});

// M3-01 审查回归

test("同频道失败后不自动重复请求，重试由调用方发起", async () => {
  auto.members = () => Promise.reject("x");
  await selectGuild("A");
  await ensureMembers();
  await ensureMembers();
  await ensureMembers();
  expect(calls.filter((c) => c.cmd === "members").length).toBe(1);
  expect(session.members?.failed).toBe(true);
});

test("强制刷新取代进行中的请求，旧结果不覆盖新结果", async () => {
  await selectGuild("A");
  hold.add("members");
  const old = ensureMembers();
  await flush();
  const fresh = ensureMembers(true);
  await flush();
  release("members", mpage([mem("new")]), () => true);
  // 先放行的是旧请求：应被丢弃。
  release("members", mpage([mem("new2")]), () => true);
  await Promise.all([old, fresh]);
  expect(session.members?.items.map((m) => m.id)).toEqual(["new2"]);
  expect(session.members?.loading).toBe(false);
});

test("翻页期间刷新：迟到的下一页不并入刷新后的列表", async () => {
  auto.members = ({ page }) => mpage([mem(`p${page}`)], page, 3);
  await selectGuild("A");
  await ensureMembers();
  hold.add("members");
  const more = loadMoreMembers();
  await flush();
  const fresh = ensureMembers(true);
  await flush();
  release("members", mpage([mem("r1")], 1, 3), (a) => a.page === 1);
  await fresh;
  release("members", mpage([mem("p2")], 2, 3), (a) => a.page === 2);
  await more;
  expect(session.members?.items.map((m) => m.id)).toEqual(["r1"]);
  expect(session.members?.page).toBe(1);
});

test("权限请求进行中收到角色变化：另发请求并采用新结果", async () => {
  await selectGuild("A");
  hold.add("guild_access");
  const first = loadAccess("A");
  sysTo("A", "user_role_change", { guild_id: "A", user_id: "me", roles: [] });
  await flush();
  expect(pending.filter((p) => p.cmd === "guild_access").length).toBe(2);
  release("guild_access", access("A", { everyone: VIEW }), () => true); // 旧请求（变化前）
  release("guild_access", access("A", { everyone: VIEW | SEND }), () => true);
  await first;
  await flush();
  expect(sendBlock()).toBeNull();
});

test("当前频道失去查看权限后切到其他可见频道，全部不可见时清空会话", async () => {
  await selectGuild("A");
  expect(ui.target).toMatchObject({ channelId: "A-t1" });
  auto.channels = () => [withOw("A-t1", "t1", [{ roleId: 0, allow: 0, deny: VIEW }]), raw("A-t2", "t2")];
  sysTo("A", "updated_channel", { guild_id: "A" });
  await flush();
  await flush();
  expect(ui.target).toMatchObject({ channelId: "A-t2" });
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: 0 });
  sysTo("A", "updated_role", { role_id: 0 });
  await flush();
  await flush();
  expect(ui.target.kind).toBe("none");
});

test("补偿时权限刷新失败计为部分失败", async () => {
  await selectGuild("A");
  auto.guild_access = () => Promise.reject("x");
  onGatewayStatus({ state: "ready", kind: "rebuilt" });
  for (let i = 0; i < 5; i++) await flush();
  expect(session.recovery).toBe("partial");
  expect(session.access.A.status).toBe("ready");
});

// M3-01 在线状态订阅

const subs = () => calls.filter((c) => c.cmd === "subscribe_members").map((c) => [c.args.channelId, c.args.start, c.args.end]);
const userList = (channelId: string, data: any[]) =>
  sysTo("A", "updated_user_list", { channel_id: channelId, start: 0, type: 2, user_list_data: { data } });

test("加载成员后订阅已加载区间，切换频道先取消旧订阅", async () => {
  auto.members = () => mpage([mem("1"), mem("2", false)]);
  setSession("gateway", "online");
  await selectGuild("A");
  await ensureMembers();
  expect(subs()).toEqual([["A-t1", 0, 1]]);
  await selectChannel("A-t2");
  await ensureMembers();
  expect(subs()).toEqual([["A-t1", 0, 1], ["A-t1", -1, -1], ["A-t2", 0, 1]]);
});

test("网关未就绪时不订阅", async () => {
  auto.members = () => mpage([mem("1")]);
  await selectGuild("A");
  await ensureMembers();
  expect(subs()).toEqual([]);
});

test("updated_user_list 更新在线状态与人数，其他频道的推送忽略", async () => {
  auto.members = () => mpage([mem("1"), mem("2", false)], 1, 1, 1, 1);
  setSession("gateway", "online");
  await selectGuild("A");
  await ensureMembers();
  userList("A-t2", [{ id: 2, online: true }]);
  expect(session.members?.items.find((m) => m.id === "2")?.online).toBe(false);
  userList("A-t1", [{ id: 2, online: true, nickname: "新" }]);
  expect(session.members?.items.map((m) => [m.id, m.online])).toEqual([["1", true], ["2", true]]);
  expect([session.members?.online, session.members?.offline]).toEqual([2, 0]);
  expect(session.members?.items[1].nickname).toBe("新");
  userList("A-t1", [{ id: 1, online: false }]);
  expect(session.members?.items.map((m) => m.id)).toEqual(["2", "1"]);
  expect([session.members?.online, session.members?.offline]).toEqual([1, 1]);
});

test("关闭成员栏与离开频道取消订阅", async () => {
  const { closeMembers } = await import("../session");
  auto.members = () => mpage([mem("1")]);
  setSession("gateway", "online");
  await selectGuild("A");
  await ensureMembers();
  closeMembers();
  expect(subs()[subs().length - 1]).toEqual(["A-t1", -1, -1]);
  expect(session.members).toBeNull();
});

test("新网关会话后重新订阅", async () => {
  auto.members = () => mpage([mem("1")]);
  setSession("gateway", "online");
  await selectGuild("A");
  await ensureMembers();
  onGatewayStatus({ state: "ready", kind: "rebuilt" });
  for (let i = 0; i < 6; i++) await flush();
  expect(subs().filter((s) => s[1] === 0).length).toBe(2);
});

// M3-02 消息内容与引用

const hist = (id: string, at: number, extra: Record<string, unknown> = {}) => ({ id: `i-${id}`, rong_id: id, create_at: at, type: 9, content: id, author: { id: "u", username: "U" }, ...extra });
const listIds = (ch: string) => (session.messages[ch] ?? []).map((m) => m.id);

test("引用保留被引用消息 ID；历史、网关与本地回显一致，网关引用内容同样解析", async () => {
  auto.messages = () => pg([hist("m1", 10, { quote: { id: "i-q", rong_id: "q", type: 9, content: "(met)u2(met) hi", author: { id: "u2", username: "小明" } } })]);
  await selectGuild("A");
  expect(session.messages["A-t1"][0].quote).toMatchObject({ id: "q", authorId: "u2", author: "小明", text: "@小明 hi" });
  onEvent({ type: "CHANNEL_MSG", channelType: "GROUP", toUserId: "A-t1", msgId: "g1", msgTimestamp: 20, fromUserId: "u",
    content: JSON.stringify({ content: "x", extra: JSON.stringify({ type: 9, author: { id: "u" }, quote: { rong_id: "q", type: 9, content: "(met)u2(met)", author: { id: "u2", username: "小明" } } }) }) });
  expect(session.messages["A-t1"].find((m) => m.id === "g1")!.quote).toMatchObject({ id: "q", text: "@小明" });
  auto.send_message = () => ({ msg_id: "s1", msg_timestamp: 30 });
  send(ui.target, "re", { id: "g1", authorId: "u", author: "U", type: 9, content: "x", text: "x" });
  await flush();
  expect(session.messages["A-t1"].find((m) => m.id === "s1")!.quote).toMatchObject({ id: "g1", authorId: "u" });
});

test("同名用户按 ID 解析，未知类型给出可读说明", () => {
  setSession("users", { a: { id: "a", name: "同名", online: true }, b: { id: "b", name: "同名", online: true } });
  expect(plainContent(9, "(met)a(met)(met)zz(met)")).toBe("@同名@未知用户");
  expect(plainContent(2, "https://x")).toBe("[图片]");
  expect(plainContent(4, "u", { name: "a.zip", url: null, size: 1 })).toBe("[文件] a.zip");
  expect(plainContent(77, "")).toBe("[不支持的消息类型 77]");
  expect(plainContent(10, JSON.stringify([{ type: "card", modules: [{ type: "header", text: { type: "plain-text", content: "标题" } }] }]))).toBe("[卡片消息] 标题");
  expect(plainContent(10, "{坏")).toBe("[卡片消息]");
});

test("跳转：已加载的消息直接定位，不发请求", async () => {
  auto.messages = () => pg([hist("m1", 10), hist("m2", 20)]);
  await selectGuild("A");
  calls.length = 0;
  expect(await jumpTo("A-t1", "m1")).toBe("shown");
  expect(session.focus).toMatchObject({ channelId: "A-t1", id: "m1" });
  expect(calls.length).toBe(0);
});

test("跳转：消息已删除或无权限给出明确提示并标记引用", async () => {
  auto.messages = () => pg([hist("m1", 10, { quote: { rong_id: "q", type: 9, content: "x", author: { id: "u" } } })]);
  await selectGuild("A");
  auto.locate_message = () => null;
  expect(await jumpTo("A-t1", "q")).toBe("gone");
  expect(session.error).toContain("已被删除");
  expect(session.messages["A-t1"][0].quote!.gone).toBe(true);
});

test("跳转：其他频道的消息不加载", async () => {
  await selectGuild("A");
  auto.locate_message = () => ({ id: "q", channelId: "B-t1", createAt: 1 });
  expect(await jumpTo("A-t1", "q")).toBe("elsewhere");
  expect(calls.some((c) => c.cmd === "messages_after")).toBe(false);
});

test("跳转：加载目标前后一页作为片段，实时消息暂存，加载到最新后合并", async () => {
  auto.messages = ({ before }) => (before ? pg([hist("t", 5), hist("p", 4)], false) : pg([hist("m9", 90)], false));
  await selectGuild("A");
  auto.locate_message = () => ({ id: "t", channelId: "A-t1", createAt: 5 });
  auto.messages_after = ({ after }) => (after === "t" ? pg([hist("n1", 6)], false) : pg([hist("n2", 7)], true));
  expect(await jumpTo("A-t1", "t")).toBe("shown");
  expect(listIds("A-t1")).toEqual(["p", "t", "n1"]);
  expect(session.history["A-t1"].newer).toBe(true);
  expect(calls.find((c) => c.cmd === "messages" && c.args.before)!.args.before).toBe("n1");
  // 片段期间的实时消息不追加，避免留下缺口。
  onEvent({ type: "CHANNEL_MSG", channelType: "GROUP", toUserId: "A-t1", msgId: "live", msgTimestamp: 100, fromUserId: "u", content: JSON.stringify({ content: "l", extra: { author: { id: "u" } } }) });
  expect(listIds("A-t1")).not.toContain("live");
  expect(calls.some((c) => c.cmd === "mark_read")).toBe(false);
  await loadNewer("A-t1");
  expect(listIds("A-t1")).toEqual(["p", "t", "n1", "n2", "live"]);
  expect(session.history["A-t1"].newer).toBe(false);
});

test("跳转：目标与已加载列表衔接时直接合并", async () => {
  auto.messages = ({ before }) => (before ? pg([hist("t", 5)], true) : pg([hist("n1", 6), hist("m9", 90)], false));
  await selectGuild("A");
  auto.locate_message = () => ({ id: "t", channelId: "A-t1", createAt: 5 });
  auto.messages_after = () => pg([hist("n1", 6)], false);
  expect(await jumpTo("A-t1", "t")).toBe("shown");
  expect(listIds("A-t1")).toEqual(["t", "n1", "m9"]);
  expect(session.history["A-t1"]).toMatchObject({ newer: false, end: true });
});

test("跳转：切换频道或再次跳转后，迟到结果不修改列表", async () => {
  auto.messages = () => pg([hist("m9", 90)], false);
  await selectGuild("A");
  hold.add("locate_message");
  const p = jumpTo("A-t1", "t");
  await selectChannel("A-t2");
  release("locate_message", { id: "t", channelId: "A-t1", createAt: 5 });
  expect(await p).toBe("stale");
  expect(listIds("A-t1")).toEqual(["m9"]);
  await selectChannel("A-t1");
  const first = jumpTo("A-t1", "t");
  const second = jumpTo("A-t1", "t2");
  release("locate_message", null, (a) => a.msgId === "t");
  expect(await first).toBe("stale");
  release("locate_message", null, (a) => a.msgId === "t2");
  expect(await second).toBe("gone");
});

test("跳转：再次点击已加载的引用，使进行中的跳转失效", async () => {
  auto.messages = () => pg([hist("m1", 10), hist("m9", 90)], false);
  await selectGuild("A");
  hold.add("locate_message");
  const first = jumpTo("A-t1", "t");
  expect(await jumpTo("A-t1", "m1")).toBe("shown");
  release("locate_message", { id: "t", channelId: "A-t1", createAt: 5 });
  expect(await first).toBe("stale");
  expect(listIds("A-t1")).toEqual(["m1", "m9"]);
  expect(session.focus).toMatchObject({ id: "m1" });
  expect(session.history["A-t1"].newer).toBeFalsy();
});

test("跳转替换列表后，替换前发出的上翻结果作废", async () => {
  auto.messages = () => pg([hist("m9", 90)], false);
  await selectGuild("A");
  hold.add("messages");
  const older = loadOlder("A-t1");
  auto.locate_message = () => ({ id: "t", channelId: "A-t1", createAt: 5 });
  auto.messages_after = () => pg([hist("n1", 6)], false);
  const j = jumpTo("A-t1", "t");
  await flush();
  release("messages", pg([hist("t", 5)], false), (a) => a.before === "n1");
  expect(await j).toBe("shown");
  release("messages", pg([hist("m8", 80)], false), (a) => a.before === "m9");
  await older;
  expect(listIds("A-t1")).toEqual(["t", "n1"]);
  expect(session.history["A-t1"]).toMatchObject({ loading: false, failed: false });
  hold.clear();
  auto.messages = () => pg([hist("m9", 90)], false);
  await jumpLatest("A-t1");
  expect(listIds("A-t1")).toEqual(["m9"]);
});

test("跳转：在片段中再跳到最新消息附近时替换片段，不拼接不连续的区间", async () => {
  auto.messages = ({ before }) => (before === "n1" ? pg([hist("p", 4), hist("t", 5)], false) : before ? pg([], true) : pg([hist("z", 200), hist("y", 201)], false));
  await selectGuild("A");
  auto.locate_message = ({ msgId }) => ({ id: msgId, channelId: "A-t1", createAt: 1 });
  auto.messages_after = ({ after }) => (after === "t" ? pg([hist("n1", 6)], false) : pg([], true));
  await jumpTo("A-t1", "t");
  expect(session.history["A-t1"].newer).toBe(true);
  auto.messages = () => pg([hist("x", 199), hist("w", 300)], false);
  expect(await jumpTo("A-t1", "w")).toBe("shown");
  expect(listIds("A-t1")).toEqual(["x", "w"]);
  expect(session.history["A-t1"].newer).toBe(false);
});

// M3-03 编辑、删除与回应

const MANAGE = 1 << 13, REACT = 1 << 18;
const msgOf = (id: string) => session.messages["A-t1"].find((m) => m.id === id);
const setupMsgs = async (perm = VIEW | SEND | REACT) => {
  auto.guild_access = ({ guildId }) => ({ guildId, ownerId: "o", master: false, myRoles: [], roles: [{ id: 0, name: "@全体成员", color: 0, position: 999, hoist: false, permissions: perm }] });
  auto.messages = () => pg([
    hist("mine", 10, { author: { id: "me", username: "me" } }),
    hist("other", 20, { reactions: [{ emoji: { id: "👍", name: "thumbsup" }, count: 2, me: true }], updated_at: 0 }),
  ]);
  await selectGuild("A");
  await flush();
};

test("历史解析回应与编辑时间", async () => {
  auto.messages = () => pg([hist("x", 1, { updated_at: 99, reactions: [{ emoji: { id: "😂", name: "joy" }, count: 1, me: false }, { emoji: { id: "a", name: "a" }, count: 0, me: false }] })]);
  await selectGuild("A");
  expect(msgOf("x")!.updatedAt).toBe(99);
  expect(msgOf("x")!.reactions!.map((r) => [r.id, r.count, r.me])).toEqual([["😂", 1, false]]);
});

test("编辑与删除权限：仅本人消息可编辑；他人消息需管理消息权限；权限未知不授权", async () => {
  await setupMsgs();
  expect(canEdit("A-t1", msgOf("mine")!)).toBe(true);
  expect(canEdit("A-t1", msgOf("other")!)).toBe(false);
  expect(canDelete("A-t1", msgOf("mine")!)).toBe(true);
  expect(canDelete("A-t1", msgOf("other")!)).toBe(false);
  expect(canReact("A-t1")).toBe(true);
  await logout();
  setSession({ phase: "ready", me: { id: "me", name: "me", online: true }, guilds: [{ id: "A", name: "A", groups: [] }] });
  await setupMsgs(VIEW | SEND | MANAGE);
  expect(canDelete("A-t1", msgOf("other")!)).toBe(true);
  expect(canReact("A-t1")).toBe(false);
  setSession("access", "A", { status: "failed" });
  expect(canDelete("A-t1", msgOf("other")!)).toBe(false);
  expect(canDelete("A-t1", msgOf("mine")!)).toBe(true);
});

test("编辑：进行中显示新内容，成功后标记已编辑，重复的编辑事件结果不变", async () => {
  await setupMsgs();
  hold.add("update_message");
  const p = editMessage("A-t1", "mine", "新内容");
  expect(msgOf("mine")).toMatchObject({ content: "新内容", pending: "editing" });
  expect(await editMessage("A-t1", "mine", "再改")).toBe(false); // 进行中不重复提交
  release("update_message", null);
  expect(await p).toBe(true);
  expect(msgOf("mine")!.pending).toBeUndefined();
  expect(msgOf("mine")!.updatedAt).toBeGreaterThan(0);
  const ev = { rong_id: "mine", channel_id: "A-t1", content: "新内容", updated_at: 500, last_msg_content: "旧" };
  sys("message_updated", ev);
  sys("message_updated", ev);
  expect(msgOf("mine")).toMatchObject({ content: "新内容", updatedAt: 500 });
  expect(calls.filter((c) => c.cmd === "update_message").length).toBe(1);
});

test("编辑失败恢复原内容；不能编辑他人消息", async () => {
  await setupMsgs();
  auto.update_message = () => { throw { kind: "rejected", message: "40012" }; };
  expect(await editMessage("A-t1", "mine", "x")).toBe(false);
  expect(msgOf("mine")).toMatchObject({ content: "mine" });
  expect(msgOf("mine")!.pending).toBeUndefined();
  expect(msgOf("mine")!.updatedAt).toBeUndefined();
  expect(session.error).toContain("40012");
  calls.length = 0;
  expect(await editMessage("A-t1", "other", "x")).toBe(false);
  expect(calls.length).toBe(0);
});

test("删除：进行中标记，失败（403）恢复；成功后移除，重复删除事件忽略", async () => {
  await setupMsgs(VIEW | SEND | REACT | MANAGE);
  hold.add("delete_message");
  const p = deleteMessage("A-t1", "other");
  expect(msgOf("other")!.pending).toBe("deleting");
  release("delete_message", null);
  expect(await p).toBe(true);
  expect(msgOf("other")).toBeUndefined();
  sys("deleted_message", { rong_id: "other", id: "i-other", channel_id: "A-t1" });
  sys("deleted_message", { rong_id: "zzz", id: "mine", channel_id: "A-t1" }); // id 字段不是消息 ID
  expect(listIds("A-t1")).toEqual(["mine"]);
  hold.clear();
  auto.delete_message = () => { throw { kind: "rejected", message: "403" }; };
  expect(await deleteMessage("A-t1", "mine")).toBe(false);
  expect(msgOf("mine")!.pending).toBeUndefined();
});

test("删除事件按 rong_id 移除并标记引用", async () => {
  auto.messages = () => pg([hist("a", 1), hist("b", 2, { quote: { rong_id: "a", type: 9, content: "x", author: { id: "u" } } })]);
  await selectGuild("A");
  sys("deleted_message", { rong_id: "a", id: "i-a", channel_id: "A-t1" });
  expect(listIds("A-t1")).toEqual(["b"]);
  expect(msgOf("b")!.quote!.gone).toBe(true);
});

test("回应：本端添加后重复事件不重复计数；他人事件按用户去重；降到 0 移除", async () => {
  await setupMsgs();
  expect(await toggleReaction("A-t1", "mine", { id: "🎉", name: "tada" })).toBe(true);
  expect(calls.find((c) => c.cmd === "reaction")!.args).toEqual({ msgId: "mine", emoji: "🎉", add: true });
  const ev = (type: string, user: string) => sys(type, { rong_id: "mine", channel_id: "A-t1", emoji: { id: "🎉", name: "tada" }, user_id: user });
  ev("add_reaction", "me");
  ev("add_reaction", "me");
  expect(msgOf("mine")!.reactions).toMatchObject([{ id: "🎉", count: 1, me: true }]);
  ev("add_reaction", "u2");
  ev("add_reaction", "u2");
  expect(msgOf("mine")!.reactions![0].count).toBe(2);
  ev("delete_reaction", "u2");
  ev("delete_reaction", "u2");
  expect(msgOf("mine")!.reactions![0].count).toBe(1);
  expect(await toggleReaction("A-t1", "mine", { id: "🎉", name: "tada" })).toBe(true);
  expect(calls.filter((c) => c.cmd === "reaction").slice(-1)[0].args.add).toBe(false);
  ev("delete_reaction", "me");
  expect(msgOf("mine")!.reactions).toEqual([]);
});

test("回应：历史已含本人回应时，重复的本人事件不改变计数；取消后计数减一", async () => {
  await setupMsgs();
  const ev = (type: string) => sys(type, { rong_id: "other", channel_id: "A-t1", emoji: { id: "👍", name: "thumbsup" }, user_id: "me" });
  ev("add_reaction");
  expect(msgOf("other")!.reactions![0]).toMatchObject({ count: 2, me: true });
  ev("delete_reaction");
  ev("delete_reaction");
  expect(msgOf("other")!.reactions![0]).toMatchObject({ count: 1, me: false });
});

test("回应失败回滚计数与 me；进行中不重复提交", async () => {
  await setupMsgs();
  hold.add("reaction");
  auto.reaction = () => { throw { kind: "rejected", message: "表情无效" }; };
  const p = toggleReaction("A-t1", "other", { id: "👍", name: "thumbsup" });
  expect(msgOf("other")!.reactions![0]).toMatchObject({ count: 1, me: false });
  expect(await toggleReaction("A-t1", "other", { id: "👍", name: "thumbsup" })).toBe(false);
  hold.clear();
  const i = pending.findIndex((x) => x.cmd === "reaction");
  pending.splice(i, 1)[0].resolve(Promise.reject({ kind: "rejected", message: "表情无效" }));
  expect(await p).toBe(false);
  expect(msgOf("other")!.reactions![0]).toMatchObject({ count: 2, me: true });
  expect(await toggleReaction("A-t1", "other", { id: "👍", name: "thumbsup" })).toBe(false);
  expect(msgOf("other")!.reactions![0]).toMatchObject({ count: 2, me: true });
  expect(await toggleReaction("A-t1", "other", { id: "🙃", name: "x" })).toBe(false);
  expect(msgOf("other")!.reactions!.map((r) => r.id)).toEqual(["👍"]);
});

test("回应请求期间可见进行中，失败后标记失败，成功后清除", async () => {
  await setupMsgs();
  hold.add("reaction");
  const p = toggleReaction("A-t1", "other", { id: "👍", name: "thumbsup" });
  expect(isReacting("A-t1", "other", "👍")).toBe(true);
  release("reaction", Promise.reject({ kind: "rejected", message: "x" }));
  expect(await p).toBe(false);
  expect(isReacting("A-t1", "other", "👍")).toBe(false);
  expect(reactFailed("A-t1", "other", "👍")).toBe(true);
  hold.clear();
  auto.reaction = () => ({});
  expect(await toggleReaction("A-t1", "other", { id: "👍", name: "thumbsup" })).toBe(true);
  expect(reactFailed("A-t1", "other", "👍")).toBe(false);
  expect(isReacting("A-t1", "other", "👍")).toBe(false);
});

test("退出登录后迟到的编辑、删除、回应结果不修改新会话", async () => {
  await setupMsgs(VIEW | SEND | REACT | MANAGE);
  hold.add("update_message");
  hold.add("delete_message");
  hold.add("reaction");
  const e = editMessage("A-t1", "mine", "旧会话");
  const d = deleteMessage("A-t1", "other");
  const r = toggleReaction("A-t1", "mine", { id: "🎉", name: "tada" });
  expect(await toggleReaction("A-t1", "other", { id: "👍", name: "thumbsup" })).toBe(false); // 删除中不可回应
  await logout();
  setSession({ phase: "ready", me: { id: "me", name: "me", online: true }, guilds: [{ id: "A", name: "A", groups: [] }] });
  hold.clear();
  await setupMsgs(VIEW | SEND | REACT | MANAGE);
  for (const c of ["update_message", "delete_message", "reaction"]) {
    const i = pending.findIndex((x) => x.cmd === c);
    pending.splice(i, 1)[0].resolve(null);
  }
  expect([await e, await d, await r]).toEqual([false, false, false]);
  expect(listIds("A-t1")).toEqual(["mine", "other"]);
  expect(msgOf("mine")).toMatchObject({ content: "mine" });
  expect(msgOf("mine")!.reactions).toBeUndefined();
  expect(msgOf("mine")!.pending).toBeUndefined();
  expect(await toggleReaction("A-t1", "mine", { id: "🎉", name: "tada" })).toBe(true); // 旧会话的进行中标记不阻塞
});

// M3-03 审查回归：拉取期间的删除、编辑、回应不被旧页覆盖；补拉以服务端副本为准。
test("首次加载期间的删除、编辑与回应不被迟到的历史页覆盖", async () => {
  hold.add("messages");
  const p = selectGuild("A");
  await flush();
  await flush();
  sys("deleted_message", { rong_id: "a", id: "i-a", channel_id: "A-t1" });
  sys("updated_message", { rong_id: "b", channel_id: "A-t1", content: "b2", updated_at: 50 });
  sys("added_reaction", { rong_id: "b", channel_id: "A-t1", emoji: { id: "🎉", name: "tada" }, user_id: "u2" });
  release("messages", pg([hist("a", 1), hist("b", 2)]));
  await p;
  await flush();
  expect(listIds("A-t1")).toEqual(["b"]);
  expect(msgOf("b")).toMatchObject({ content: "b2", updatedAt: 50 });
  expect(msgOf("b")!.reactions).toMatchObject([{ id: "🎉", count: 1 }]);
});

test("补拉期间删除的消息不被旧页带回", async () => {
  auto.messages = () => pg([hist("a", 1), hist("b", 2)]);
  await selectGuild("A");
  await flush();
  hold.add("messages");
  const p = loadHistory("A-t1");
  await flush();
  sys("deleted_message", { rong_id: "a", id: "i-a", channel_id: "A-t1" });
  release("messages", pg([hist("a", 1), hist("b", 2), hist("c", 3)]));
  await p;
  expect(listIds("A-t1")).toEqual(["b", "c"]);
});

test("重连补拉以服务端副本更新已缓存消息的内容与回应", async () => {
  auto.messages = () => pg([hist("a", 1), hist("b", 2)]);
  await selectGuild("A");
  await flush();
  auto.messages = () => pg([hist("a", 1, { content: "a2", updated_at: 77, reactions: [{ emoji: { id: "👍", name: "t" }, count: 3, me: true }] }), hist("b", 2)]);
  await loadHistory("A-t1");
  expect(msgOf("a")).toMatchObject({ content: "a2", updatedAt: 77 });
  expect(msgOf("a")!.reactions).toMatchObject([{ id: "👍", count: 3, me: true }]);
});

// M3-04 图片与文件发送

const file = (name: string, size: number, type = "") => new File([new Uint8Array(size)], name, { type });
const settle = async () => { for (let i = 0; i < 5; i++) await flush(); };
const uploads = () => calls.filter((c) => c.cmd === "upload");
const sends = () => calls.filter((c) => c.cmd === "send_message");

test("附件检查：空文件与超过 5 MB 被拒绝，图片按类型识别", async () => {
  await selectGuild("A");
  const errs = stageFiles(ui.target, [file("e.txt", 0), file("big.bin", 5 * 1024 * 1024 + 1), file("ok.png", 5 * 1024 * 1024, "image/png"), file("a.exe", 3)]);
  expect(errs.length).toBe(2);
  expect(ui.staged[currentKey()].map((f) => [f.name, f.image])).toEqual([["ok.png", true], ["a.exe", false]]);
});

test("文字与图片混合：文字在前、引用附在首条，图片上传后以类型 2 发送", async () => {
  await selectGuild("A");
  auto.upload = () => ({ url: "https://img/a.png" });
  auto.send_message = ({ kind }) => ({ msg_id: `m${kind}`, msg_timestamp: 1 });
  stageFiles(ui.target, [file("a.png", 4, "image/png")]);
  expect(submit(ui.target, "hi", { id: "q", author: "x", text: "t" })).toBe(true);
  expect(ui.staged[currentKey()]).toEqual([]);
  await settle();
  const s = sends();
  expect(s.map((c) => [c.args.kind, c.args.content, c.args.quote])).toEqual([[9, "hi", "q"], [2, "https://img/a.png", undefined]]);
  expect(uploads()[0].options.headers["x-file-name"]).toBe("a.png");
  expect(session.messages["A-t1"].map((m) => m.type)).toEqual([9, 2]);
  expect(box()).toEqual([]);
});

test("文件以类型 4 发送，本地回显为文件卡片，网关回显不重复", async () => {
  await selectGuild("A");
  auto.upload = () => ({ url: "https://f/x.zip" });
  auto.send_message = () => ({ msg_id: "mf", msg_timestamp: 1 });
  stageFiles(ui.target, [file("x.zip", 10)]);
  submit(ui.target, "");
  await settle();
  expect(sends()[0].args.kind).toBe(4);
  const m = session.messages["A-t1"][0];
  expect(m.type).toBe(10);
  expect(plainContent(m.type, m.content)).toBe("[文件] x.zip");
  onEvent({ type: "CHANNEL_MSG", msgId: "mf", toUserId: "A-t1", content: JSON.stringify({ content: m.content }), extra: JSON.stringify({ type: 4, author: { id: "me" } }) });
  expect(session.messages["A-t1"].length).toBe(1);
});

test("附件绑定选择时的会话：切换频道后发送仍发往原频道", async () => {
  await selectGuild("A");
  const t1 = ui.target;
  stageFiles(t1, [file("a.png", 4, "image/png")]);
  await selectChannel("A-t2");
  expect(ui.staged[currentKey()] ?? []).toEqual([]);
  hold.add("upload");
  auto.send_message = () => ({ msg_id: "m1", msg_timestamp: 1 });
  submit(t1, "");
  await settle();
  release("upload", { url: "https://img/a.png" });
  await settle();
  expect(sends()[0].args.channelId).toBe("A-t1");
  expect(session.outbox["A-t2"] ?? []).toEqual([]);
});

test("上传进度与取消：取消后迟到结果被丢弃，重试重新上传", async () => {
  await selectGuild("A");
  hold.add("upload");
  stageFiles(ui.target, [file("a.png", 100, "image/png")]);
  submit(ui.target, "");
  await settle();
  const id = uploads()[0].options.headers["x-upload-id"];
  onUploadProgress({ id, sent: 40, total: 100 });
  expect(box()[0]).toMatchObject({ status: "uploading", file: { sent: 40 } });
  cancelUpload("A-t1", box()[0].localId);
  expect(calls.find((c) => c.cmd === "upload_cancel")!.args.id).toBe(id);
  expect(box()[0]).toMatchObject({ status: "failed", error: "已取消" });
  release("upload", { url: "https://img/late.png" }); // 迟到结果
  await settle();
  expect(sends()).toEqual([]);
  expect(box()[0].file!.url).toBeUndefined();
  hold.clear();
  auto.upload = () => ({ url: "https://img/a.png" });
  auto.send_message = () => ({ msg_id: "m1", msg_timestamp: 1 });
  retry("A-t1", box()[0].localId);
  await settle();
  expect(uploads().length).toBe(2);
  expect(uploads()[1].options.headers["x-upload-id"]).not.toBe(id);
  expect(sends().map((c) => c.args.content)).toEqual(["https://img/a.png"]);
  expect(box()).toEqual([]);
});

test("上传失败可重试；上传成功发送失败后重试不重新上传", async () => {
  await selectGuild("A");
  auto.upload = fail("rejected");
  stageFiles(ui.target, [file("a.png", 4, "image/png")]);
  submit(ui.target, "");
  await settle();
  expect(box()[0]).toMatchObject({ status: "failed" });
  expect(sends()).toEqual([]);
  auto.upload = () => ({ url: "https://img/a.png" });
  auto.send_message = fail("not_sent");
  retry("A-t1", box()[0].localId);
  await settle();
  expect(box()[0]).toMatchObject({ status: "failed", file: { url: "https://img/a.png" } });
  auto.send_message = () => ({ msg_id: "m1", msg_timestamp: 1 });
  retry("A-t1", box()[0].localId);
  await settle();
  expect(uploads().length).toBe(2);
  expect(sends().length).toBe(2);
  expect(box()).toEqual([]);
});

test("文件发送结果不确定时按资源地址核对，不重复发送", async () => {
  await selectGuild("A");
  auto.upload = () => ({ url: "https://img/a.png" });
  auto.send_message = fail("uncertain");
  stageFiles(ui.target, [file("a.png", 4, "image/png")]);
  submit(ui.target, "");
  await settle();
  expect(box()[0].status).toBe("uncertain");
  retry("A-t1", box()[0].localId); // 不确定时不允许直接重试
  await settle();
  expect(sends().length).toBe(1);
  auto.messages = () => pg([{ rong_id: "m1", type: 2, content: "https://img/a.png", create_at: Date.now(), author: { id: "me" } }]);
  await verify("A-t1", box()[0].localId);
  expect(box()).toEqual([]);
  expect(sends().length).toBe(1);
  expect(session.messages["A-t1"].map((m) => m.id)).toContain("m1");
});

test("放弃失败的附件后放回附件栏，不丢失；移除附件释放", async () => {
  await selectGuild("A");
  auto.upload = fail("not_sent");
  stageFiles(ui.target, [file("a.png", 4, "image/png")]);
  submit(ui.target, "");
  await settle();
  discard("A-t1", box()[0].localId, true);
  expect(box()).toEqual([]);
  expect(ui.staged[currentKey()].map((f) => f.name)).toEqual(["a.png"]);
  unstage(currentKey(), ui.staged[currentKey()][0].id);
  expect(ui.staged[currentKey()]).toEqual([]);
});

test("退出登录后迟到的上传结果不发送、不写入新会话", async () => {
  await selectGuild("A");
  hold.add("upload");
  stageFiles(ui.target, [file("a.png", 4, "image/png")]);
  submit(ui.target, "");
  await settle();
  const id = uploads()[0].options.headers["x-upload-id"];
  await logout();
  expect(calls.some((c) => c.cmd === "upload_cancel" && c.args.id === id)).toBe(true);
  setSession({ phase: "ready", me: { id: "me", name: "me", online: true }, guilds: [{ id: "A", name: "A", groups: [] }] });
  release("upload", { url: "https://img/a.png" });
  await settle();
  expect(sends()).toEqual([]);
  expect(session.outbox).toEqual({});
});

test("多个附件按选择顺序发送：文字先发，后选的小文件先传完也等前一条发出", async () => {
  await selectGuild("A");
  hold.add("upload");
  hold.add("send_message");
  stageFiles(ui.target, [file("big.zip", 400), file("s.png", 4, "image/png")]);
  submit(ui.target, "hi", { id: "q", author: "x", text: "t" });
  await settle();
  expect(sends().map((c) => c.args.content)).toEqual(["hi"]);
  // 第二个文件先上传完成，第一条文字仍未返回
  release("upload", { url: "https://img/s.png" }, (b) => b.length === 4);
  await settle();
  expect(sends().length).toBe(1);
  release("send_message", { msg_id: "t", msg_timestamp: 1 });
  await settle();
  expect(sends().length).toBe(1);
  release("upload", { url: "https://f/big.zip" }, (b) => b.length === 400);
  await settle();
  expect(sends().map((c) => c.args.content)).toEqual(["hi", "https://f/big.zip"]);
  release("send_message", { msg_id: "b", msg_timestamp: 2 });
  await settle();
  expect(sends().map((c) => [c.args.content, c.args.quote])).toEqual([["hi", "q"], ["https://f/big.zip", undefined], ["https://img/s.png", undefined]]);
  release("send_message", { msg_id: "s", msg_timestamp: 3 });
  await settle();
  expect(box()).toEqual([]);
});

test("文件卡片核对按解析后的地址比对，兼容斜杠转义", async () => {
  await selectGuild("A");
  auto.upload = () => ({ url: "https://f/x.zip" });
  auto.send_message = fail("uncertain");
  stageFiles(ui.target, [file("x.zip", 10)]);
  submit(ui.target, "");
  await settle();
  const escaped = '[{"type":"card","modules":[{"type":"file","title":"x.zip","src":"https:\\/\\/f\\/x.zip"}]}]';
  expect(cardFileSrcs(escaped)).toEqual(["https://f/x.zip"]);
  auto.messages = () => pg([{ rong_id: "m1", type: 10, content: escaped, create_at: Date.now(), author: { id: "me" } }]);
  await verify("A-t1", box()[0].localId);
  expect(box()).toEqual([]);
  expect(sends().length).toBe(1);
});

// 私信（M4-01）

const CODE_A = "a".repeat(32), CODE_B = "b".repeat(32);
const chat = (code: string, uid: string, latestAt: number, unread = 0, preview = "") =>
  ({ code, target: { id: uid, username: `u${uid}`, nickname: "", identifyNum: "0001", avatar: "", online: true, bot: false, system: false }, preview, latestAt, readAt: 0, unread });
const dmMsg = (id: string, at: number, author = "u1") => ({ id, type: 1, content: id, create_at: at, author: { id: author, username: `u${author}` } });
const pm = (code: string, id: string, at: number, from = "u1") =>
  ({ type: 1, channelType: "PERSON", fromUserId: from, toUserId: "x", msgId: id, msgTimestamp: at, content: JSON.stringify({ content: id, extra: { code, type: 1, author: { id: from, username: `u${from}` } } }) });

test("私信入口加载会话列表，映射 chat_code 与用户 ID，翻页去重并保持降序", async () => {
  auto.dm_chats = ({ page }) => (page === 1
    ? { items: [chat(CODE_A, "u1", 30, 2, "hi"), chat(CODE_B, "u2", 20)], page: 1, pageTotal: 2, total: 3 }
    : { items: [chat(CODE_B, "u2", 20), chat("c".repeat(32), "u3", 10)], page: 2, pageTotal: 2, total: 3 });
  openDm();
  await flush();
  expect(ui.target).toEqual({ kind: "none" });
  expect(session.dms.map((c) => c.code)).toEqual([CODE_A, CODE_B]);
  expect(session.dmByUser).toEqual({ u1: CODE_A, u2: CODE_B });
  expect(dmUnreadTotal()).toBe(2);
  await loadDms(true);
  expect(session.dms.map((c) => c.target.id)).toEqual(["u1", "u2", "u3"]);
  expect(session.dmList.page).toBe(2);
  await loadDms(true); // 已到最后一页，不再请求
  expect(calls.filter((c) => c.cmd === "dm_chats").length).toBe(2);
});

test("私信列表为空与失败状态", async () => {
  auto.dm_chats = () => ({ items: [], page: 1, pageTotal: 1, total: 0 });
  openDm();
  await flush();
  expect(session.dmList).toMatchObject({ loaded: true, failed: false });
  expect(session.dms).toEqual([]);
  await logout();
  setSession({ phase: "ready", me: { id: "me", name: "me", online: true } });
  auto.dm_chats = () => Promise.reject({ kind: "not_sent", message: "x" });
  openDm();
  await flush();
  expect(session.dmList).toMatchObject({ loaded: false, failed: true, loading: false });
});

test("私信历史以 chat_code 请求，存放在独立键下，不与同名频道 ID 混用", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 30)], page: 1, pageTotal: 1, total: 1 });
  auto.dm_messages = ({ chatCode }) => (chatCode === CODE_A ? pg([dmMsg("d1", 1), dmMsg("d2", 2)], false) : pg([]));
  openDm();
  await flush();
  await selectDm(CODE_A);
  expect(ui.target).toEqual({ kind: "dm", chatId: CODE_A });
  expect(session.messages[dmKey(CODE_A)].map((m) => m.id)).toEqual(["d1", "d2"]);
  expect(session.messages[CODE_A]).toBeUndefined();
  expect(calls.some((c) => c.cmd === "messages")).toBe(false);
  // 上翻以最早一条的 id 为游标
  auto.dm_messages = () => pg([dmMsg("d0", 0)]);
  await loadDmOlder(CODE_A);
  expect(calls.filter((c) => c.cmd === "dm_messages").pop()!.args).toEqual({ chatCode: CODE_A, before: "d1" });
  expect(session.messages[dmKey(CODE_A)].map((m) => m.id)).toEqual(["d0", "d1", "d2"]);
  expect(session.history[dmKey(CODE_A)].end).toBe(true);
});

test("上翻在途时最新页替换私信列表，迟到的旧页被丢弃，不形成空洞", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 30)], page: 1, pageTotal: 1, total: 1 });
  auto.dm_messages = () => pg([dmMsg("d10", 10), dmMsg("d11", 11)], false);
  openDm();
  await flush();
  await selectDm(CODE_A);
  hold.add("dm_messages");
  const older = loadDmOlder(CODE_A);
  const refresh = loadDmHistory(CODE_A); // 已有列表 → refreshDm
  release("dm_messages", pg([dmMsg("d50", 50), dmMsg("d51", 51)], false), (x) => !x.before);
  await refresh;
  expect(session.messages[dmKey(CODE_A)].map((m) => m.id)).toEqual(["d50", "d51"]);
  release("dm_messages", pg([dmMsg("d1", 1)], false), (x) => x.before === "d10");
  await older;
  expect(session.messages[dmKey(CODE_A)].map((m) => m.id)).toEqual(["d50", "d51"]);
  expect(session.history[dmKey(CODE_A)]).toMatchObject({ end: false, loading: false });
  hold.delete("dm_messages");
  auto.dm_messages = () => pg([dmMsg("d49", 49)], false);
  await loadDmOlder(CODE_A);
  expect(calls.filter((c) => c.cmd === "dm_messages").pop()!.args).toEqual({ chatCode: CODE_A, before: "d50" });
});

test("频道与多个私信快速切换，迟到的私信历史不改变当前会话，草稿与引用互不残留", async () => {
  await selectGuild("A");
  setUi("drafts", currentKey(), "频道草稿");
  setUi("replies", currentKey(), { id: "r", author: "x", text: "q" });
  hold.add("dm_messages");
  const a = selectDm(CODE_A);
  const b = selectDm(CODE_B);
  expect(currentKey()).toBe(`dm:${CODE_B}`);
  expect(ui.drafts[currentKey()]).toBeUndefined();
  expect(ui.replies[currentKey()]).toBeUndefined();
  setUi("drafts", currentKey(), "私信B草稿");
  release("dm_messages", pg([dmMsg("b1", 1)]), (x) => x.chatCode === CODE_B);
  await b;
  const back = selectChannel("A-t1");
  await back;
  release("dm_messages", pg([dmMsg("a1", 1)]), (x) => x.chatCode === CODE_A); // A 迟到
  await a;
  await flush();
  expect(ui.target).toMatchObject({ kind: "channel", channelId: "A-t1" });
  expect(ui.drafts["channel:A-t1"]).toBe("频道草稿");
  expect(ui.replies["channel:A-t1"]).toMatchObject({ id: "r" });
  expect(ui.drafts[`dm:${CODE_B}`]).toBe("私信B草稿");
  expect(session.messages[dmKey(CODE_A)].map((m) => m.id)).toEqual(["a1"]);
  expect(session.messages[dmKey(CODE_B)].map((m) => m.id)).toEqual(["b1"]);
  expect(session.messages["A-t1"].some((m) => m.id === "a1" || m.id === "b1")).toBe(false);
  expect(canSend({ kind: "dm", chatId: CODE_A })).toBe(false);
});

test("从成员打开私信：已知映射直接打开，未知时请求一次并合并并发调用", async () => {
  auto.dm_messages = () => pg([]);
  auto.open_dm = ({ userId }) => chat(CODE_B, userId, 5);
  hold.add("open_dm");
  const p1 = openDmWith("u2");
  const p2 = openDmWith("u2");
  release("open_dm", chat(CODE_B, "u2", 5));
  expect(await p1).toBe(CODE_B);
  expect(await p2).toBe(CODE_B);
  expect(calls.filter((c) => c.cmd === "open_dm").length).toBe(1);
  expect(ui.target).toEqual({ kind: "dm", chatId: CODE_B });
  expect(session.dmByUser.u2).toBe(CODE_B);
  hold.clear();
  await openDmWith("u2");
  expect(calls.filter((c) => c.cmd === "open_dm").length).toBe(1);
  expect(await openDmWith("me")).toBeNull();
});

test("打开私信期间切到频道则不跳转；退出登录后迟到结果不写入新会话", async () => {
  await selectGuild("A");
  hold.add("open_dm");
  const p = openDmWith("u9");
  await selectChannel("A-t2");
  release("open_dm", chat(CODE_A, "u9", 1));
  await p;
  expect(ui.target).toMatchObject({ kind: "channel", channelId: "A-t2" });
  expect(session.dmByUser.u9).toBe(CODE_A);

  hold.add("dm_chats");
  openDm();
  await logout();
  setSession({ phase: "ready", me: { id: "me", name: "me", online: true } });
  release("dm_chats", { items: [chat(CODE_A, "u1", 1, 3)], page: 1, pageTotal: 1, total: 1 });
  await flush();
  expect(session.dms).toEqual([]);
  expect(session.dmByUser).toEqual({});
});

test("私信推送按 extra.code 路由：追加历史、更新摘要排序与未读，重复推送不重复计数", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 30), chat(CODE_B, "u2", 20)], page: 1, pageTotal: 1, total: 2 });
  auto.dm_messages = () => pg([dmMsg("b1", 20, "u2")]);
  openDm();
  await flush();
  await selectDm(CODE_B);
  await selectGuild("A");
  const ev = pm(CODE_B, "b2", 40, "u2");
  onEvent(ev);
  onEvent(ev);
  expect(session.dms.map((c) => c.code)).toEqual([CODE_B, CODE_A]);
  expect(session.dms[0]).toMatchObject({ unread: 1, preview: "b2", latestAt: 40 });
  expect(session.messages[dmKey(CODE_B)].map((m) => m.id)).toEqual(["b1", "b2"]);
  expect(session.messages["A-t1"].some((m) => m.id === "b2")).toBe(false);
  // 本人另一端发出的私信不计未读
  onEvent(pm(CODE_A, "a9", 50, "me"));
  expect(session.dms.find((c) => c.code === CODE_A)!.unread).toBe(0);
});

test("打开有未读的私信时按最后一条标记已读，成功后清零；失败保留", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 2, 2)], page: 1, pageTotal: 1, total: 1 });
  auto.dm_messages = () => pg([dmMsg("d1", 1), dmMsg("d2", 2)]);
  auto.mark_dm_read = () => Promise.reject({ kind: "not_sent", message: "x" });
  openDm();
  await flush();
  await selectDm(CODE_A);
  await flush();
  expect(calls.find((c) => c.cmd === "mark_dm_read")!.args).toEqual({ chatCode: CODE_A, msgId: "d2", time: 2 });
  expect(session.dms[0].unread).toBe(2);
  auto.mark_dm_read = () => null;
  readCurrent();
  await flush();
  expect(session.dms[0].unread).toBe(0);
  const n = calls.filter((c) => c.cmd === "mark_dm_read").length;
  readCurrent();
  await flush();
  expect(calls.filter((c) => c.cmd === "mark_dm_read").length).toBe(n);
});

test("私信删除事件移除消息；跨端已读事件清零未读", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 2, 1)], page: 1, pageTotal: 1, total: 1 });
  auto.dm_messages = () => pg([dmMsg("d1", 1), dmMsg("d2", 2)]);
  hold.add("mark_dm_read"); // 本端标记未完成，未读只能由跨端事件清零
  openDm();
  await flush();
  await selectDm(CODE_A);
  await selectGuild("A");
  onEvent({ type: "SYS_MSG", content: JSON.stringify({ type: "private_message_deleted", body: { code: CODE_A, id: "9001", rong_id: "d2", author_id: 1 } }) });
  expect(session.messages[dmKey(CODE_A)].map((m) => m.id)).toEqual(["d1"]);
  onEvent({ type: "SYS_MSG", content: JSON.stringify({ type: "mark_as_batchread", body: { user_chat: [{ chat_code: CODE_A, time: 5 }] } }) });
  expect(session.dms[0].unread).toBe(0);
  // 删除事件触发的列表刷新迟到且仍带旧未读时，不使已读的会话回升。
  await flush();
  expect(session.dms[0].unread).toBe(0);
});

test("登录后即加载私信列表：未打开私信入口也显示未读，新会话推送刷新列表", async () => {
  await logout();
  auto.me = () => ({ id: "me", username: "me" });
  auto.guilds = () => [];
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 30, 2)], page: 1, pageTotal: 1, total: 1 });
  await enter();
  await flush();
  expect(dmUnreadTotal()).toBe(2);
  auto.dm_chats = () => ({ items: [chat(CODE_B, "u2", 40, 1), chat(CODE_A, "u1", 30, 2)], page: 1, pageTotal: 1, total: 2 });
  onEvent(pm(CODE_B, "b1", 40, "u2"));
  await flush();
  expect(session.dms.map((c) => c.code)).toEqual([CODE_B, CODE_A]);
  expect(dmUnreadTotal()).toBe(3);
});

test("列表尚未加载时收到私信也加载列表", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 40, 1)], page: 1, pageTotal: 1, total: 1 });
  onEvent(pm(CODE_A, "a1", 40));
  await flush();
  expect(dmUnreadTotal()).toBe(1);
});

test("刷新私信列表移除服务端已不返回的会话", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 30), chat(CODE_B, "u2", 20, 4)], page: 1, pageTotal: 1, total: 2 });
  openDm();
  await flush();
  expect(dmUnreadTotal()).toBe(4);
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 30)], page: 1, pageTotal: 1, total: 1 });
  await loadDms();
  expect(session.dms.map((c) => c.code)).toEqual([CODE_A]);
  expect(dmUnreadTotal()).toBe(0);
});

test("迟到的列表结果不回退请求期间推送更新的摘要、排序与未读", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 30), chat(CODE_B, "u2", 20)], page: 1, pageTotal: 1, total: 2 });
  openDm();
  await flush();
  hold.add("dm_chats");
  const p = loadDms();
  onEvent(pm(CODE_B, "b2", 40, "u2"));
  release("dm_chats", { items: [chat(CODE_A, "u1", 30), chat(CODE_B, "u2", 20, 0, "old")], page: 1, pageTotal: 1, total: 2 });
  await p;
  expect(session.dms.map((c) => c.code)).toEqual([CODE_B, CODE_A]);
  expect(session.dms[0]).toMatchObject({ preview: "b2", latestAt: 40, unread: 1 });
});

test("退出登录清除私信标记中状态，重新登录后可再次标记已读", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 2, 2)], page: 1, pageTotal: 1, total: 1 });
  auto.dm_messages = () => pg([dmMsg("d1", 1), dmMsg("d2", 2)]);
  hold.add("mark_dm_read");
  openDm();
  await flush();
  await selectDm(CODE_A);
  await flush();
  expect(calls.filter((c) => c.cmd === "mark_dm_read").length).toBe(1);
  await logout();
  release("mark_dm_read", null);
  await flush();
  hold.delete("mark_dm_read");
  auto.mark_dm_read = () => null;
  setSession({ phase: "ready", me: { id: "me", name: "me", online: true } });
  openDm();
  await flush();
  await selectDm(CODE_A);
  await flush();
  expect(calls.filter((c) => c.cmd === "mark_dm_read").length).toBe(2);
  expect(session.dms[0].unread).toBe(0);
});

// 私信（M4-02）

const sysEv = (type: string, body: unknown) => ({ type: "SYS_MSG", content: JSON.stringify({ type, body }) });
// 真实推送：d.content 解析后 extra 仍是 JSON 字符串。
const pm2 = (code: string, id: string, at: number, from = "u1", extra: Record<string, unknown> = {}) =>
  ({ type: 1, channelType: "PERSON", fromUserId: from, toUserId: "x", msgId: id, msgTimestamp: at, content: JSON.stringify({ content: id, extra: JSON.stringify({ code, type: 1, author: { id: from, username: `u${from}` }, ...extra }) }) });
const DM = { kind: "dm" as const, chatId: CODE_A };

async function openA(history = [dmMsg("d1", 1, "u1"), dmMsg("m1", 2, "me")]) {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 2)], page: 1, pageTotal: 1, total: 1 });
  auto.dm_messages = () => pg(history);
  openDm();
  await flush();
  await selectDm(CODE_A);
}

test("私信发送只走私信接口，以对方用户 ID 发送并带引用；本端回显与网关回显去重，不计未读", async () => {
  await openA();
  auto.send_direct = () => ({ id: "m2", code: CODE_A, createAt: 10 });
  expect(canSend()).toBe(true);
  expect(submit(DM, "你好", { id: "d1", author: "u1", text: "d1" })).toBe(true);
  await flush();
  const c = calls.find((x) => x.cmd === "send_direct")!;
  expect(c.args).toEqual({ userId: "u1", kind: 9, content: "你好", quote: "d1" });
  expect(calls.some((x) => x.cmd === "send_message")).toBe(false);
  expect(session.messages[dmKey(CODE_A)].map((m) => m.id)).toEqual(["d1", "m1", "m2"]);
  expect(session.outbox[dmKey(CODE_A)]).toEqual([]);
  expect(session.dms[0]).toMatchObject({ preview: "你好", latestAt: 10, unread: 0 });
  onEvent(pm2(CODE_A, "m2", 10, "me"));
  expect(session.messages[dmKey(CODE_A)].filter((m) => m.id === "m2").length).toBe(1);
  expect(session.dms[0].unread).toBe(0);
  // 频道 ID 与 chat_code 相同也不会收到私信内容
  expect(session.messages[CODE_A]).toBeUndefined();
});

test("私信附件以类型 2/4 经私信接口发送；系统会话与未知会话不能发送", async () => {
  await openA();
  auto.upload = () => ({ url: "https://img.example/a.png" });
  auto.send_direct = ({ kind }) => ({ id: `f${kind}`, code: CODE_A, createAt: 20 });
  expect(stageFiles(DM, [new File(["x"], "a.png", { type: "image/png" })])).toEqual([]);
  expect(ui.staged[`dm:${CODE_A}`][0].channelId).toBe(dmKey(CODE_A));
  expect(submit(DM, "", undefined)).toBe(true);
  for (let i = 0; i < 5; i++) await flush();
  expect(calls.filter((x) => x.cmd === "send_direct").map((x) => x.args.kind)).toEqual([2]);
  expect(calls.some((x) => x.cmd === "send_message")).toBe(false);
  expect(session.messages[dmKey(CODE_A)].some((m) => m.id === "f2")).toBe(true);
  expect(canSend({ kind: "dm", chatId: "z".repeat(32) })).toBe(false);
  setSession("dms", 0, "target", "system", true);
  expect(canSend()).toBe(false);
  expect(submit(DM, "x")).toBe(false);
});

test("私信发送结果不确定时按私信历史核对，不重发也不查频道", async () => {
  await openA();
  auto.send_direct = () => Promise.reject({ kind: "uncertain", message: "超时" });
  submit(DM, "嗨");
  await flush();
  const o = session.outbox[dmKey(CODE_A)][0];
  expect(o.status).toBe("uncertain");
  auto.dm_messages = () => pg([dmMsg("d1", 1), { ...dmMsg("m9", Date.now() + 1, "me"), content: "嗨" }]);
  await verify(dmKey(CODE_A), o.localId);
  expect(calls.filter((x) => x.cmd === "send_direct").length).toBe(1);
  expect(calls.some((x) => x.cmd === "messages")).toBe(false);
  expect(session.messages[dmKey(CODE_A)].some((m) => m.id === "m9")).toBe(true);
  expect(session.outbox[dmKey(CODE_A)]).toEqual([]);
});

test("私信编辑删除只对本人消息开放，走私信接口；对方消息给出明确提示且不请求", async () => {
  await openA();
  const k = dmKey(CODE_A);
  const [other, mine] = session.messages[k];
  expect(canEdit(k, mine)).toBe(true);
  expect(canDelete(k, mine)).toBe(true);
  expect(canEdit(k, other)).toBe(false);
  expect(canDelete(k, other)).toBe(false);
  expect(canReact(k)).toBe(true);
  expect(await editMessage(k, "d1", "改")).toBe(false);
  expect(session.error).toContain("不能编辑对方");
  expect(await deleteMessage(k, "d1")).toBe(false);
  expect(session.error).toContain("不能删除对方");
  expect(await editMessage(k, "m1", "新内容")).toBe(true);
  expect(calls.find((x) => x.cmd === "update_dm_message")!.args).toEqual({ msgId: "m1", content: "新内容" });
  expect(await deleteMessage(k, "m1")).toBe(true);
  expect(calls.find((x) => x.cmd === "delete_dm_message")!.args).toEqual({ msgId: "m1" });
  expect(calls.some((x) => ["update_message", "delete_message"].includes(x.cmd))).toBe(false);
  expect(session.messages[k].map((m) => m.id)).toEqual(["d1"]);
  // 删除后的重复删除事件与迟到推送都不会带回
  onEvent(sysEv("private_message_deleted", { id: "1", code: CODE_A, rong_id: "m1", author_id: 1 }));
  onEvent(pm2(CODE_A, "m1", 2, "me"));
  expect(session.messages[k].map((m) => m.id)).toEqual(["d1"]);
});

test("私信回应走私信接口；本端回应后的重复事件与他人事件按用户去重", async () => {
  await openA();
  const k = dmKey(CODE_A);
  expect(await toggleReaction(k, "d1", { id: "👍", name: "👍" })).toBe(true);
  expect(calls.find((x) => x.cmd === "dm_reaction")!.args).toEqual({ msgId: "d1", emoji: "👍", add: true });
  expect(calls.some((x) => x.cmd === "reaction")).toBe(false);
  const ev = (type: string, user: number) => sysEv(type, { code: CODE_A, rong_id: "d1", emoji: { id: "👍", name: "👍" }, user_id: user });
  onEvent({ ...ev("private_add_reaction", 0), content: JSON.stringify({ type: "private_add_reaction", body: { code: CODE_A, rong_id: "d1", emoji: { id: "👍", name: "👍" }, user_id: "me" } }) });
  onEvent(ev("private_add_reaction", 7));
  onEvent(ev("private_add_reaction", 7));
  expect(session.messages[k][0].reactions).toMatchObject([{ id: "👍", count: 2, me: true }]);
  onEvent(ev("private_delete_reaction", 7));
  onEvent(ev("private_delete_reaction", 7));
  expect(session.messages[k][0].reactions).toMatchObject([{ count: 1 }]);
  // 回应失败回滚
  auto.dm_reaction = () => Promise.reject({ kind: "rejected", message: "消息已删除" });
  expect(await toggleReaction(k, "d1", { id: "❤", name: "❤" })).toBe(false);
  expect(session.messages[k][0].reactions!.some((r) => r.id === "❤")).toBe(false);
});

test("对方编辑与删除事件按 code 与 rong_id 生效，重复事件幂等，不影响同 ID 频道", async () => {
  await openA();
  const k = dmKey(CODE_A);
  const up = sysEv("private_message_updated", { code: CODE_A, rong_id: "d1", type: 9, author_id: "u1", target_id: "me", content: "改过 **了**", updated_at: 99 });
  onEvent(up);
  onEvent(up);
  expect(session.messages[k][0]).toMatchObject({ content: "改过 **了**", updatedAt: 99 });
  onEvent(sysEv("private_message_updated", { code: CODE_A, rong_id: "m1", type: 9, content: "最后一条", updated_at: 100 }));
  expect(session.dms[0].preview).toBe("最后一条");
  onEvent(sysEv("private_message_deleted", { id: "1", code: CODE_A, rong_id: "d1", author_id: 1 }));
  onEvent(sysEv("private_message_deleted", { id: "1", code: CODE_A, rong_id: "d1", author_id: 1 }));
  expect(session.messages[k].map((m) => m.id)).toEqual(["m1"]);
});

test("推送 extra 为 JSON 字符串时正确路由并解析引用", async () => {
  await openA();
  onEvent(pm2(CODE_A, "d3", 30, "u1", { quote: { id: "x", rong_id: "m1", type: 1, content: "m1", author: { id: "me", username: "me" } } }));
  const m = session.messages[dmKey(CODE_A)].find((x) => x.id === "d3")!;
  expect(m.quote).toMatchObject({ id: "m1", authorId: "me" });
  expect(session.dms[0].latestAt).toBe(30);
});

test("首次加载私信历史期间到达的删除、编辑与新消息不被迟到的旧页覆盖", async () => {
  auto.dm_chats = () => ({ items: [chat(CODE_A, "u1", 2)], page: 1, pageTotal: 1, total: 1 });
  openDm();
  await flush();
  hold.add("dm_messages");
  const p = selectDm(CODE_A);
  await flush();
  onEvent(sysEv("private_message_deleted", { id: "1", code: CODE_A, rong_id: "d1" }));
  onEvent(sysEv("private_message_updated", { code: CODE_A, rong_id: "d2", content: "新", updated_at: 5 }));
  onEvent(pm2(CODE_A, "d4", 40));
  release("dm_messages", pg([dmMsg("d1", 1), dmMsg("d2", 2)]));
  await p;
  const list = session.messages[dmKey(CODE_A)];
  expect(list.map((m) => m.id)).toEqual(["d2", "d4"]);
  expect(list[0]).toMatchObject({ content: "新", updatedAt: 5 });
});

test("重连补拉：断线期间对方删除与编辑的消息以服务端为准", async () => {
  await openA([dmMsg("d1", 1), dmMsg("d2", 2), dmMsg("d3", 3)]);
  auto.dm_messages = () => pg([dmMsg("d1", 1), { ...dmMsg("d3", 3), content: "改", updated_at: 9 }, dmMsg("d5", 5)]);
  onGatewayStatus({ state: "ready", kind: "rebuilt" });
  for (let i = 0; i < 6; i++) await flush();
  const list = session.messages[dmKey(CODE_A)];
  expect(list.map((m) => m.id)).toEqual(["d1", "d3", "d5"]);
  expect(list[1]).toMatchObject({ content: "改", updatedAt: 9 });
});

test("私信操作在退出登录后迟到返回不改变新会话；私信与频道的待发消息互不串", async () => {
  await openA();
  hold.add("send_direct");
  submit(DM, "旧账号");
  await flush();
  await logout();
  setSession({ phase: "ready", me: { id: "me", name: "me", online: true } });
  release("send_direct", { id: "late", code: CODE_A, createAt: 50 });
  await flush();
  expect(session.messages[dmKey(CODE_A)]).toBeUndefined();
  expect(session.outbox[dmKey(CODE_A)] ?? []).toEqual([]);
  expect(msgKey({ kind: "dm", chatId: CODE_A })).toBe(dmKey(CODE_A));
  expect(msgKey({ kind: "channel", guildId: "A", channelId: CODE_A })).toBe(CODE_A);
});

// // 语音会话

const vs = await import("../voice");
const peer = (id: string, extra: Partial<import("../voice").VoicePeer> = {}) =>
  ({ id, name: id, avatar: "", device: "pc", micOff: false, headsetOff: false, permissionSay: true, serverMute: false, ...extra });
const ev = (generation: number, event: import("../voice").VoiceEvent, channelId = "V1") => vs.onVoiceEvent({ generation, channelId, event });

test("语音会话：加入后按事件更新成员，已在该频道时不重复加入", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  expect(vs.voice).toMatchObject({ channelId: "V1", generation: 1, phase: "connecting" });
  ev(1, { type: "joined", me: peer("me"), peers: [peer("u1")] });
  ev(1, { type: "state", phase: "connected" });
  expect(vs.voice.phase).toBe("connected");
  ev(1, { type: "peer_joined", peer: peer("u2") });
  ev(1, { type: "peer_joined", peer: peer("u2") });
  ev(1, { type: "peer_changed", patch: { id: "u1", micOff: true } });
  ev(1, { type: "peer_left", id: "u2" });
  expect(vs.voice.peers.map((p) => [p.id, p.micOff])).toEqual([["u1", true]]);
  // // 已在该频道时不重复加入。
  await vs.joinVoice("V1");
  expect(calls.filter((c) => c.cmd === "voice_join").length).toBe(1);
});

test("加入后记录本人的 permissionSay=false 状态", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "joined", me: peer("me", { permissionSay: false }), peers: [] });
  expect(vs.voice.me?.permissionSay).toBe(false);
});

test("语音会话：切换频道后旧会话的迟到事件被忽略", async () => {
  auto.voice_join = ({ channelId }) => (channelId === "V1" ? 1 : 2);
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  await vs.joinVoice("V2");
  expect(vs.voice).toMatchObject({ channelId: "V2", generation: 2, phase: "connecting" });
  // // 旧会话的退出与成员事件不影响新频道。
  ev(1, { type: "state", phase: "disconnected", reason: "left" });
  ev(1, { type: "joined", me: peer("me"), peers: [peer("old")] });
  expect(vs.voice).toMatchObject({ channelId: "V2", phase: "connecting", peers: [] });
  ev(2, { type: "state", phase: "connected" }, "V2");
  expect(vs.voice.phase).toBe("connected");
});

test("语音会话：加入返回前的事件暂存，按返回的代次过滤", async () => {
  hold.add("voice_join");
  const p = vs.joinVoice("V1");
  await flush();
  ev(4, { type: "state", phase: "disconnected", reason: "replaced" }); // // 代次不符，丢弃。
  ev(5, { type: "joined", me: peer("me"), peers: [] });
  ev(5, { type: "state", phase: "connected" });
  release("voice_join", 5);
  await p;
  expect(vs.voice).toMatchObject({ generation: 5, phase: "connected", reason: null });
  expect(vs.voice.me?.id).toBe("me");
});

test("语音会话：连续加入时先返回的较新结果不被较早的结果覆盖", async () => {
  hold.add("voice_join");
  const a = vs.joinVoice("V1");
  const b = vs.joinVoice("V2");
  await flush();
  release("voice_join", 2, (x) => x.channelId === "V2");
  await b;
  release("voice_join", 1, (x) => x.channelId === "V1");
  await a;
  expect(vs.voice).toMatchObject({ channelId: "V2", generation: 2 });
});

test("语音会话：被替换后显示原因且不自动重新加入", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "state", phase: "disconnected", reason: "replaced" });
  await flush();
  expect(vs.voice).toMatchObject({ phase: "disconnected", reason: "replaced", channelId: "V1" });
  expect(vs.voiceStatusText()).toBe("已在其他设备加入语音");
  expect(calls.filter((c) => c.cmd === "voice_join").length).toBe(1);
  vs.dismissVoice();
  expect(vs.voice.phase).toBe("idle");
});

test("语音会话：重连与断开时显示原因并清空成员", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "reconnecting", message: "网络中断" });
  expect(vs.voice).toMatchObject({ phase: "reconnecting", message: "网络中断" });
  ev(1, { type: "state", phase: "disconnected", reason: "lost", message: "语音连接已断开" });
  expect(vs.voice).toMatchObject({ phase: "disconnected", reason: "lost", peers: [] });
});

test("语音会话：退出后不再处理旧会话事件", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  await vs.leaveVoice();
  expect(calls.some((c) => c.cmd === "voice_leave")).toBe(true);
  expect(vs.voice.phase).toBe("idle");
  ev(1, { type: "state", phase: "connected" });
  expect(vs.voice.phase).toBe("idle");
});

test("加入在途时退出：请求序号让后端拒绝迟到的加入", async () => {
  hold.add("voice_join");
  const p = vs.joinVoice("V1");
  await vs.leaveVoice();
  await flush();
  const join = calls.find((c) => c.cmd === "voice_join")!;
  const leave = calls.find((c) => c.cmd === "voice_leave")!;
  // // 退出的序号大于加入，后端据此拒绝迟到的加入。
  expect(typeof join.args.request).toBe("number");
  expect(leave.args.request).toBeGreaterThan(join.args.request);
  pending.splice(0).forEach((x) => x.resolve(Promise.reject({ kind: "cancelled", message: "已取消" })));
  await p;
  expect(vs.voice.phase).toBe("idle");
});

test("语音会话：快速切换时被取代的加入返回取消，不显示失败", async () => {
  hold.add("voice_join");
  const a = vs.joinVoice("V1");
  const b = vs.joinVoice("V2");
  await flush();
  const [ra, rb] = calls.filter((c) => c.cmd === "voice_join").map((c) => c.args.request);
  expect(rb).toBeGreaterThan(ra);
  release("voice_join", 2, (x) => x.channelId === "V2");
  release("voice_join", Promise.reject({ kind: "cancelled", message: "已取消" }), (x) => x.channelId === "V1");
  await Promise.all([a, b]);
  expect(vs.voice).toMatchObject({ channelId: "V2", generation: 2 });
});

test("语音会话：加入被拒绝时显示失败原因", async () => {
  auto.voice_join = () => Promise.reject({ kind: "rejected", message: "没有权限" });
  await vs.joinVoice("V1");
  expect(vs.voice).toMatchObject({ phase: "disconnected", reason: "join_failed", message: "没有权限" });
});

test("语音会话：加入在途时退出登录，迟到的结果不恢复语音", async () => {
  hold.add("voice_join");
  const p = vs.joinVoice("V1");
  await flush();
  await logout();
  release("voice_join", 1);
  await p;
  expect(vs.voice.phase).toBe("idle");
});

// // 麦克风

test("麦克风：默认开启，静音后电平归零，操作失败时提示", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  expect(calls.find((c) => c.cmd === "voice_join")!.args.audio.muted).toBe(false);
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "mic", status: "starting", muted: false });
  ev(1, { type: "mic", status: "active", muted: false, device: "内置麦克风" });
  ev(1, { type: "mic_level", level: 40 });
  expect(vs.mic).toMatchObject({ status: "active", device: "内置麦克风", level: 40, muted: false });
  await vs.toggleMic();
  expect(calls.find((c) => c.cmd === "voice_set_mic")!.args).toEqual({ generation: 1, paused: true });
  expect(vs.mic.muted).toBe(true);
  ev(1, { type: "mic", status: "muted", muted: true });
  expect(vs.mic).toMatchObject({ status: "muted", level: 0 });
  // // 静音后的迟到电平被忽略。
  ev(1, { type: "mic_level", level: 80 });
  expect(vs.mic.level).toBe(0);
  // 后端拒绝时给出提示，静音状态以后端随后的事件为准。
  auto.voice_set_mic = () => Promise.reject({ kind: "rejected", message: "服务端拒绝" });
  await vs.toggleMic();
  expect(vs.mic.busy).toBe(false);
  expect(vs.mic.message).toBeTruthy();
});

test("麦克风：静音失败时以后端事件为准并显示原因", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "mic", status: "active", muted: false, device: "d" });
  // // 后端先推送静音状态，再返回错误。
  auto.voice_set_mic = () => {
    ev(1, { type: "mic", status: "muted", muted: true });
    return Promise.reject({ kind: "rejected", message: "暂停推流失败" });
  };
  await vs.setMuted(true);
  expect(vs.mic).toMatchObject({ status: "muted", muted: true, busy: false });
  expect(vs.mic.message).toBe("暂停推流失败");
});

test("麦克风：取消静音失败时保持静音", async () => {
  auto.voice_join = () => 1;
  await vs.setMuted(true);
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "mic", status: "muted", muted: true });
  auto.voice_set_mic = () => Promise.reject({ kind: "rejected", message: "恢复推流失败" });
  const p = vs.setMuted(false);
  // // 后端恢复失败后回到静音。
  ev(1, { type: "mic", status: "muted", muted: true, message: "恢复推流失败" });
  await p;
  expect(vs.mic).toMatchObject({ status: "muted", muted: true });
});

test("麦克风：加入期间静音，加入完成后发送一次", async () => {
  hold.add("voice_join");
  const p = vs.joinVoice("V1");
  await flush();
  await vs.setMuted(true);
  expect(calls.find((c) => c.cmd === "voice_join")!.args.audio.muted).toBe(false);
  release("voice_join", 1);
  await p;
  expect(calls.filter((c) => c.cmd === "voice_set_mic").map((c) => c.args)).toEqual([{ generation: 1, paused: true }]);
  expect(vs.mic.muted).toBe(true);
});

test("麦克风：加入期间切换后与加入时一致，不发送", async () => {
  hold.add("voice_join");
  const p = vs.joinVoice("V1");
  await flush();
  await vs.setMuted(true);
  await vs.setMuted(false);
  release("voice_join", 1);
  await p;
  expect(calls.some((c) => c.cmd === "voice_set_mic")).toBe(false);
});

test("麦克风：退出登录时释放语音并重置偏好", async () => {
  auto.voice_join = () => 1;
  await vs.setMuted(true);
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "mic", status: "active", muted: false, device: "d" });
  ev(1, { type: "mic_level", level: 50 });
  calls.length = 0;
  await logout();
  // // 退出登录时同时退出语音。
  expect(calls.some((c) => c.cmd === "logout")).toBe(true);
  expect(vs.voice.phase).toBe("idle");
  expect(vs.mic).toMatchObject({ muted: false, status: "off", device: null, level: 0, message: null, busy: false });
});

test("麦克风：发言权限被收回后停止采集，恢复后重新开始", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "joined", me: peer("me"), peers: [] });
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "mic", status: "active", muted: false, device: "d" });
  ev(1, { type: "peer_changed", patch: { id: "me", permissionSay: false } });
  ev(1, { type: "mic", status: "off", muted: false, message: "没有发言权限" });
  expect(vs.voice.me?.permissionSay).toBe(false);
  expect(vs.mic).toMatchObject({ status: "off", device: null, level: 0 });
  expect(vs.micText()).toBe("麦克风未开启");
  ev(1, { type: "peer_changed", patch: { id: "me", permissionSay: true } });
  ev(1, { type: "mic", status: "starting", muted: false });
  expect(vs.voice.me?.permissionSay).toBe(true);
  expect(vs.mic.status).toBe("starting");
});

test("麦克风：权限被拒绝时给出处理建议，取消静音即重试", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "mic", status: "denied", muted: false, message: "权限被拒绝" });
  expect(vs.micFailed()).toBe(true);
  expect(vs.micText()).toBe("麦克风权限被拒绝");
  expect(vs.micHint()).toContain("系统设置");
  await vs.toggleMic();
  expect(calls.filter((c) => c.cmd === "voice_set_mic").map((c) => c.args.paused)).toEqual([false]);
  ev(1, { type: "mic", status: "active", muted: false, device: "d" });
  expect(vs.micFailed()).toBe(false);
  ev(1, { type: "mic", status: "no_device", muted: false, message: "没有可用的麦克风" });
  expect(vs.micHint()).toContain("连接麦克风");
});

test("麦克风：加入前静音随加入请求发送，重连与断开时清除设备状态", async () => {
  await vs.setMuted(true);
  expect(calls.some((c) => c.cmd === "voice_set_mic")).toBe(false);
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  expect(calls.find((c) => c.cmd === "voice_join")!.args.audio.muted).toBe(true);
  ev(1, { type: "mic", status: "active", muted: false, device: "d" });
  ev(1, { type: "state", phase: "reconnecting", message: "网络中断" });
  expect(vs.mic.status).toBe("off");
  ev(1, { type: "state", phase: "disconnected", reason: "replaced" });
  expect(vs.mic).toMatchObject({ status: "off", device: null, level: 0 });
  // // 新会话忽略旧代次的事件。
  auto.voice_join = () => 2;
  await vs.joinVoice("V2");
  ev(1, { type: "mic", status: "active", muted: false, device: "old" });
  expect(vs.mic.device).toBeNull();
  vs.resetVoice();
  expect(vs.mic.muted).toBe(false);
});

test("麦克风：选择输入设备，失败时恢复之前的选择", async () => {
  auto.audio_inputs = () => ({ devices: [{ id: "coreaudio:a", name: "A", default: true }], selected: null });
  await vs.loadInputs();
  expect(vs.inputs.devices.length).toBe(1);
  await vs.selectInput("coreaudio:a");
  expect(calls.find((c) => c.cmd === "set_audio_input")!.args).toEqual({ id: "coreaudio:a" });
  expect(vs.inputs.selected).toBe("coreaudio:a");
  auto.set_audio_input = () => Promise.reject({ kind: "local", message: "x" });
  await vs.selectInput(null);
  expect(vs.inputs.selected).toBe("coreaudio:a");
  expect(vs.inputs.error).toBeTruthy();
});

// // 耳机与播放

test("耳机：未连接时关闭耳机同时静音，打开后恢复之前的状态", async () => {
  await vs.setDeafened(true);
  expect(vs.output.deafened).toBe(true);
  expect(vs.mic.muted).toBe(true);
  await vs.setDeafened(false);
  expect(vs.mic.muted).toBe(false);
  // // 关闭前已静音，打开后保持静音。
  await vs.setMuted(true);
  await vs.setDeafened(true);
  await vs.setDeafened(false);
  expect(vs.mic.muted).toBe(true);
  // // 关闭耳机期间取消静音会同时打开耳机。
  await vs.setDeafened(true);
  await vs.setMuted(false);
  expect(vs.output.deafened).toBe(false);
  expect(calls.some((c) => c.cmd === "voice_set_deafen")).toBe(false);
});

test("耳机：离线先静音再关闭耳机，加入时带上关闭前的静音状态", async () => {
  await vs.setMuted(true);
  await vs.setDeafened(true);
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  expect(calls.find((c) => c.cmd === "voice_join")!.args.audio).toEqual({ muted: true, deafened: true, micBeforeDeaf: true });
});

test("耳机：未关闭时不发送关闭前的静音状态", async () => {
  await vs.setMuted(true);
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  expect(calls.find((c) => c.cmd === "voice_join")!.args.audio).toEqual({ muted: true, deafened: false, micBeforeDeaf: null });
});

test("耳机：关闭状态随加入请求发送，连接后可以切换", async () => {
  await vs.setDeafened(true);
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  expect(calls.find((c) => c.cmd === "voice_join")!.args).toMatchObject({ audio: { muted: true, deafened: true } });
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "output", deafened: true, active: false });
  await vs.toggleDeafen();
  expect(calls.find((c) => c.cmd === "voice_set_deafen")!.args).toEqual({ generation: 1, deafened: false });
  ev(1, { type: "output", deafened: false, active: true, device: "内置扬声器" });
  ev(1, { type: "mic", status: "active", muted: false });
  expect(vs.output).toMatchObject({ deafened: false, active: true, device: "内置扬声器", busy: false });
  expect(vs.mic.muted).toBe(false);
  // // 操作失败时显示原因。
  auto.voice_set_deafen = () => Promise.reject({ kind: "rejected", message: "服务端拒绝" });
  await vs.setDeafened(true);
  expect(vs.output.message).toBe("服务端拒绝");
  await vs.leaveVoice();
  expect(vs.output.active).toBe(false);
});

test("耳机：加入期间关闭，加入完成后发送一次", async () => {
  hold.add("voice_join");
  const j = vs.joinVoice("V1");
  await flush();
  await vs.setDeafened(true);
  release("voice_join", 1);
  await j;
  expect(calls.filter((c) => c.cmd === "voice_set_deafen").map((c) => c.args)).toEqual([{ generation: 1, deafened: true }]);
});

test("耳机：说话状态随成员离开、关闭耳机与旧代次事件更新", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "joined", me: peer("me"), peers: [peer("u1"), peer("u2")] });
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "speaking", ids: ["u1", "u2"] });
  expect(vs.isSpeaking("u1") && vs.isSpeaking("u2")).toBe(true);
  ev(1, { type: "peer_left", id: "u2" });
  expect(vs.output.speaking).toEqual(["u1"]);
  ev(2, { type: "speaking", ids: ["u9"] });
  expect(vs.isSpeaking("u9")).toBe(false);
  ev(1, { type: "output", deafened: true, active: false });
  expect(vs.output.speaking).toEqual([]);
  ev(1, { type: "speaking", ids: ["u1"] });
  expect(vs.isSpeaking("u1")).toBe(false);
  ev(1, { type: "state", phase: "disconnected", reason: "lost" });
  expect(vs.output.active).toBe(false);
});

test("音量：成员音量限制在 0 至 200，退出登录时重置", async () => {
  await vs.setPeerVolume("u1", 250);
  expect(vs.volumeOf("u1")).toBe(200);
  expect(calls.find((c) => c.cmd === "voice_set_volume")!.args).toEqual({ userId: "u1", percent: 200 });
  await vs.setPeerVolume("u1", -5);
  expect(vs.volumeOf("u1")).toBe(0);
  expect(vs.volumeOf("u2")).toBe(100);
  await vs.setMasterVolume(80);
  expect(calls.find((c) => c.cmd === "voice_set_master")!.args).toEqual({ percent: 80 });
  vs.resetVoice();
  expect(vs.volumeOf("u1")).toBe(100);
  // 总音量属于本机偏好，退出登录时保留。
  expect(vs.output.master).toBe(80);
  await vs.setMasterVolume(100);
});

test("音量：选择输出设备，失败时恢复之前的选择", async () => {
  auto.audio_outputs = () => ({ devices: [{ id: "coreaudio:s", name: "S", default: true }], selected: null });
  await vs.loadOutputs();
  expect(vs.outputs.devices.length).toBe(1);
  await vs.selectOutput("coreaudio:s");
  expect(calls.find((c) => c.cmd === "set_audio_output")!.args).toEqual({ id: "coreaudio:s" });
  auto.set_audio_output = () => Promise.reject({ kind: "local", message: "x" });
  await vs.selectOutput(null);
  expect(vs.outputs.selected).toBe("coreaudio:s");
});

test("音量：麦克风增益限制范围，连续调节以最新值为准，失败时恢复", async () => {
  await vs.setMicGain(100);
  auto.voice_set_gain = ({ percent }) => percent;
  await vs.setMicGain(250);
  expect(vs.output.gain).toBe(200);
  expect(calls[calls.length - 1].args).toEqual({ percent: 200 });
  // 先发的请求晚返回，不覆盖后来的值。
  hold.add("voice_set_gain");
  const a = vs.setMicGain(50);
  const b = vs.setMicGain(150);
  release("voice_set_gain", 150, (x) => x.percent === 150);
  await b;
  release("voice_set_gain", 50, (x) => x.percent === 50);
  await a;
  expect(vs.output.gain).toBe(150);
  hold.clear();
  auto.voice_set_gain = () => Promise.reject({ kind: "local", message: "x" });
  await vs.setMicGain(20);
  expect(vs.output.gain).toBe(150);
  expect(vs.mic.message).toContain("未生效");
  auto.voice_set_gain = ({ percent }) => percent;
  await vs.setMicGain(100);
});

test("音量：调节音量不改变静音与耳机状态", async () => {
  vs.resetVoice();
  await vs.setMuted(true);
  await vs.setDeafened(true);
  auto.voice_set_gain = ({ percent }) => percent;
  await vs.setMicGain(200);
  await vs.setMasterVolume(200);
  expect(vs.mic.muted).toBe(true);
  expect(vs.output.deafened).toBe(true);
  expect(calls.some((c) => c.cmd === "voice_set_mic" || c.cmd === "voice_set_deafen")).toBe(false);
  await vs.setMicGain(100);
  await vs.setMasterVolume(100);
  vs.resetVoice();
});

test("音量：较早请求成功、较新请求失败时恢复为较早的值", async () => {
  await vs.setMasterVolume(100);
  hold.add("voice_set_master");
  const a = vs.setMasterVolume(120);
  const b = vs.setMasterVolume(130);
  release("voice_set_master", null, (x) => x.percent === 120);
  await a;
  const i = pending.findIndex((x) => x.cmd === "voice_set_master");
  pending.splice(i, 1)[0].resolve(Promise.reject({ kind: "local", message: "x" }));
  await b;
  expect(vs.output.master).toBe(120);
  hold.clear();
  await vs.setMasterVolume(100);
});

test("总音量失败时恢复上次生效的值", async () => {
  await vs.setMasterVolume(60);
  auto.voice_set_master = () => Promise.reject({ kind: "local", message: "x" });
  await vs.setMasterVolume(90);
  expect(vs.output.master).toBe(60);
  expect(vs.output.message).toContain("未生效");
  delete auto.voice_set_master;
  await vs.setMasterVolume(100);
});

test("设备：读取保存的偏好，读取期间的选择不被覆盖", async () => {
  hold.add("audio_prefs");
  const p = vs.loadAudioPrefs();
  await flush();
  await vs.selectOutput("o2");
  release("audio_prefs", { input: "i1", output: "o1", gain: 130, master: 70 });
  await p;
  expect(vs.inputs.selected).toBe("i1");
  expect(vs.outputs.selected).toBe("o2");
  expect(vs.output.gain).toBe(130);
  expect(vs.output.master).toBe(70);
  hold.clear();
  await vs.setMicGain(100);
  await vs.setMasterVolume(100);
  await vs.selectInput(null);
  await vs.selectOutput(null);
});

test("设备：连续切换以最后一次为准，迟到的失败不回滚新选择", async () => {
  await vs.selectInput(null);
  hold.add("set_audio_input");
  const a = vs.selectInput("a");
  const b = vs.selectInput("b");
  release("set_audio_input", null, (x) => x.id === "b");
  await b;
  // 先发的请求迟到失败。
  const i = pending.findIndex((x) => x.cmd === "set_audio_input");
  pending.splice(i, 1)[0].resolve(Promise.reject({ kind: "local", message: "x" }));
  await a;
  expect(vs.inputs.selected).toBe("b");
  expect(vs.inputs.error).toBeNull();
  hold.clear();
  await vs.selectInput(null);
});

test("设备：同名设备按序号区分，所选设备缺失时提示回退", async () => {
  auto.audio_outputs = () => ({
    devices: [{ id: "x1", name: "USB", default: false }, { id: "x2", name: "USB", default: true }, { id: "y", name: "内置", default: false }],
    selected: "gone",
  });
  await vs.loadOutputs();
  const opts = vs.deviceOptions(vs.outputs);
  expect(opts.map((o) => o.label)).toEqual(["USB 1", "USB 2", "内置"]);
  expect(opts[1].isDefault).toBe(true);
  expect(opts[1].title).toBe("USB 2");
  expect(vs.defaultName(vs.outputs)).toBe("USB");
  expect(vs.selectionMissing(vs.outputs)).toBe(true);
  await vs.selectOutput("y");
  expect(vs.selectionMissing(vs.outputs)).toBe(false);
  // 列表在途期间的选择不被返回的旧选择覆盖。
  hold.add("audio_outputs");
  const l = vs.loadOutputs();
  await flush();
  await vs.selectOutput("x1");
  release("audio_outputs", { devices: [], selected: "y" });
  await l;
  expect(vs.outputs.selected).toBe("x1");
  hold.clear();
  await vs.selectOutput(null);
});

// // 连接恢复

test("连接恢复：显示链路质量与重连次数", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "link", quality: "fair", down: { packets: 1, lost: 0, late: 0, overflow: 0, underruns: 0, depthMs: 80 } });
  expect(vs.qualityText()).toBe("一般");
  ev(1, { type: "state", phase: "reconnecting", message: "网络中断", attempt: 2, max: 3, retry_in_ms: 2000 });
  expect(vs.voice).toMatchObject({ phase: "reconnecting", attempt: 2, maxAttempts: 3, quality: null });
  expect(vs.voiceStatusText()).toBe("正在重连，第 2/3 次");
  expect(vs.voice.retryAt! - Date.now()).toBeGreaterThan(1000);
  ev(1, { type: "state", phase: "connected" });
  expect(vs.voice).toMatchObject({ phase: "connected", attempt: 0, retryAt: null });
});

test("连接恢复：重建次数用尽后可以重新加入，并沿用静音与耳机状态", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  await vs.setDeafened(true);
  ev(1, { type: "output", deafened: true, active: false });
  ev(1, { type: "mic", status: "muted", muted: true });
  ev(1, { type: "state", phase: "disconnected", reason: "lost", message: "语音连接已断开" });
  expect(vs.voiceStatusText()).toBe("语音连接已断开");
  expect(vs.canRejoin()).toBe(true);
  calls.length = 0;
  auto.voice_join = () => 2;
  await vs.rejoinVoice();
  const join = calls.find((c) => c.cmd === "voice_join")!;
  expect(join.args).toMatchObject({ channelId: "V1", audio: { muted: true, deafened: true } });
  expect(vs.voice).toMatchObject({ channelId: "V1", generation: 2, phase: "connecting" });
  expect(vs.canRejoin()).toBe(false);
});

test("连接恢复：被替换或登录失效后不重新加入，也不唤醒", async () => {
  for (const reason of ["replaced", "unauthorized"] as const) {
    auto.voice_join = () => 1;
    await vs.leaveVoice();
    await vs.joinVoice("V1");
    ev(1, { type: "state", phase: "connected" });
    calls.length = 0;
    ev(1, { type: "state", phase: "disconnected", reason });
    expect(vs.voice.phase).toBe("disconnected");
    expect(vs.canRejoin()).toBe(false);
    expect(vs.voice.attempt).toBe(0);
    await vs.rejoinVoice();
    vs.voiceNetworkBack();
    await flush();
    expect(calls.filter((c) => c.cmd === "voice_join" || c.cmd === "voice_wake")).toEqual([]);
  }
});

test("连接恢复：网络恢复与网关重连时唤醒语音会话", async () => {
  const { networkBack } = await import("../session");
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  calls.length = 0;
  // 已连接时也唤醒：媒体可能已断开而阶段仍为 connected，后端据此立即重建。
  networkBack();
  onGatewayStatus({ state: "ready", kind: "resumed" });
  await flush();
  expect(calls.filter((c) => c.cmd === "voice_wake").length).toBe(2);
  ev(1, { type: "state", phase: "reconnecting", attempt: 1, max: 3, retry_in_ms: 1000 });
  networkBack();
  await flush();
  expect(calls.filter((c) => c.cmd === "voice_wake").length).toBe(3);
  onGatewayStatus({ state: "ready", kind: "rebuilt" });
  await flush();
  expect(calls.filter((c) => c.cmd === "voice_wake").length).toBe(4);
  // // 首次连接网关不唤醒。
  onGatewayStatus({ state: "ready", kind: "new" });
  await flush();
  expect(calls.filter((c) => c.cmd === "voice_wake").length).toBe(4);
});

test("设备摘要：显示当前设备与设备提示", async () => {
  auto.voice_join = () => 1;
  await vs.leaveVoice();
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  ev(1, { type: "mic", status: "active", muted: false, device: "内置麦克风" });
  ev(1, { type: "output", deafened: false, active: true, device: "内置扬声器" });
  expect(vs.deviceSummary()).toMatchObject({ text: "麦克风：内置麦克风 扬声器：内置扬声器", ok: true });
  ev(1, { type: "output", deafened: false, active: true, device: "耳机", message: "已改用系统默认设备" });
  expect(vs.deviceSummary().title).toContain("已改用系统默认设备");
  ev(1, { type: "mic", status: "denied", muted: false, message: "x" });
  ev(1, { type: "output", deafened: false, active: false, message: "扬声器已断开" });
  expect(vs.deviceSummary()).toMatchObject({ text: "麦克风：无权限 扬声器：未开启", ok: false });
});

// M5-02a 频道权限与可见性

const CONNECT = 1 << 15;
const vraw = (id: string, overwrites: any[] = [], extra: any = {}) => ({ ...raw(id, id, 2), overwrites, userOverwrites: [], ...extra });

test("权限依据加载中时不显示频道，避免暴露未过滤的名称", async () => {
  hold.add("guild_access");
  const p = selectGuild("A");
  await flush();
  await flush();
  const c = session.guilds[0].groups.flatMap((g) => g.channels)[0];
  expect(c).toBeDefined();
  expect(visible(c)).toBe(false);
  release("guild_access", access("A"));
  await p;
  expect(visible(c)).toBe(true);
});

test("无 VIEW 的语音、无 CONNECT 的语音不申请令牌，可连接时才加入", async () => {
  auto.voice_join = () => 1;
  auto.channels = () => [raw("A-t1", "t1"), vraw("V-hide", [{ roleId: 0, allow: 0, deny: VIEW }]), vraw("V-noc", [{ roleId: 0, allow: 0, deny: CONNECT }]), vraw("V-ok")];
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: VIEW | SEND | CONNECT });
  await selectGuild("A");
  expect(voiceBlock("V-hide")).toBe("denied");
  expect(voiceBlock("V-noc")).toBe("denied");
  await enterVoice("V-hide");
  await enterVoice("V-noc");
  expect(calls.some((c) => c.cmd === "voice_join")).toBe(false);
  await enterVoice("V-ok");
  expect(calls.filter((c) => c.cmd === "voice_join").map((c) => c.args.channelId)).toEqual(["V-ok"]);
  await vs.leaveVoice();
});

test("依据未就绪时不加入语音", async () => {
  auto.channels = () => [raw("A-t1", "t1"), vraw("V-ok")];
  hold.add("guild_access");
  const p = selectGuild("A");
  await flush();
  const j = enterVoice("V-ok");
  await flush();
  // 首次读取与加入前的刷新均失败。
  while (pending.some((x) => x.cmd === "guild_access")) release("guild_access", Promise.reject("x"));
  await p;
  await j;
  expect(voiceBlock("V-ok")).toBe("failed");
  expect(calls.some((c) => c.cmd === "voice_join")).toBe(false);
});

test("无 SEND 的文字频道可阅读但不发送", async () => {
  auto.channels = () => [withOw("A-ro", "ro", [{ roleId: 0, allow: 0, deny: SEND }])];
  await selectGuild("A");
  expect(ui.target).toMatchObject({ channelId: "A-ro" });
  expect(calls.some((c) => c.cmd === "messages" && c.args.channelId === "A-ro")).toBe(true);
  expect(sendBlock()).toBe("denied");
  setUi("drafts", currentKey(), "hi");
  expect(canSend()).toBe(false);
  expect(submit(ui.target, "hi")).toBe(false);
  expect(calls.some((c) => c.cmd === "send_message")).toBe(false);
});

test("分组无可见子项时不读取，同步子频道不叠加父分组覆盖", async () => {
  auto.channels = () => [
    { ...raw("cat", "c", 0), overwrites: [{ roleId: 0, allow: 0, deny: VIEW }], userOverwrites: [] },
    { ...withOw("A-sync", "s", []), parentId: "cat", permissionSync: true },
  ];
  await selectGuild("A");
  // 子频道自身覆盖为空，按服务端复制的结果计算，不继承分组的拒绝。
  expect(ui.target).toMatchObject({ channelId: "A-sync" });
  expect(session.guilds[0].groups[0].channels[0].sync).toBe(true);
});

test("权限撤销后移除内容、未读，切离当前频道并丢弃迟到历史", async () => {
  const nav = await import("../session/nav");
  auto.channels = () => [raw("A-t1", "t1"), raw("A-t2", "t2"), raw("A-t3", "t3")];
  await selectGuild("A");
  await selectChannel("A-t2");
  setSession("unread", "A-t2", { guildId: "A", unread: true, mentions: 1, readAt: 0 });
  // 频道覆盖变更：A-t2 失去查看权限。
  auto.channels = () => [raw("A-t1", "t1"), withOw("A-t2", "t2", [{ roleId: 0, allow: 0, deny: VIEW }]), raw("A-t3", "t3")];
  await nav.reloadChannels("A");
  expect(session.messages["A-t2"]).toBeUndefined();
  expect(session.unread["A-t2"]).toBeUndefined();
  expect(ui.target).toMatchObject({ channelId: "A-t1" });
  // 撤销后的推送与未读不写回。
  sys("unread_count_changed", { channel_id: "A-t2", guild_id: "A", msg_time: 99, mention_count: 1 });
  expect(session.unread["A-t2"]).toBeUndefined();
  // A-t3 历史在途时被撤销：迟到结果不写回。
  hold.add("messages");
  const late = selectChannel("A-t3");
  await flush();
  hold.delete("messages");
  auto.channels = () => [raw("A-t1", "t1"), withOw("A-t3", "t3", [{ roleId: 0, allow: 0, deny: VIEW }])];
  await nav.reloadChannels("A");
  release("messages", pg([{ id: "m1", rong_id: "m1", content: "x", type: 1, author: { id: "u" }, create_at: 1 }]), (a) => a.channelId === "A-t3");
  await late;
  expect(session.messages["A-t3"]).toBeUndefined();
  expect(ui.target).toMatchObject({ channelId: "A-t1" });
});

test("已连接语音失去 CONNECT 后结束会话，不提供重新加入", async () => {
  auto.voice_join = () => 1;
  auto.channels = () => [raw("A-t1", "t1"), vraw("V-ok")];
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: VIEW | SEND | CONNECT });
  await selectGuild("A");
  await enterVoice("V-ok");
  ev(1, { type: "state", phase: "connected" }, "V-ok");
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: VIEW | SEND });
  await loadAccess("A", true);
  await flush();
  expect(calls.some((c) => c.cmd === "voice_leave")).toBe(true);
  expect(vs.voice.reason).toBe("revoked");
  expect(vs.canRejoin()).toBe(false);
});

test("密码频道：服务端记住时直接加入，40310 才提示，错误密码另行说明，取消不重试", async () => {
  let gen = 0;
  auto.voice_join = () => ++gen;
  auto.channels = () => [raw("A-t1", "t1"), vraw("V-pw", [], { hasPassword: true })];
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: VIEW | SEND | CONNECT });
  await selectGuild("A");
  await enterVoice("V-pw");
  expect(calls.find((c) => c.cmd === "voice_join")?.args.password).toBeNull();
  expect(pwPrompt.channelId).toBe("");
  ev(gen, { type: "state", phase: "disconnected", reason: "password" }, "V-pw");
  expect(pwPrompt).toMatchObject({ channelId: "V-pw", error: null });
  await enterVoice("V-pw", "bad");
  expect(calls.filter((c) => c.cmd === "voice_join").slice(-1)[0]?.args.password).toBe("bad");
  ev(gen, { type: "state", phase: "disconnected", reason: "password" }, "V-pw");
  expect(pwPrompt).toMatchObject({ channelId: "V-pw", error: "密码错误" });
  cancelPassword();
  ev(gen, { type: "state", phase: "disconnected", reason: "password" }, "V-pw");
  expect(pwPrompt.channelId).toBe("");
});

test("updated_guild 合并刷新角色依据与频道覆盖", async () => {
  await selectGuild("A");
  calls.length = 0;
  for (let i = 0; i < 3; i++) sys("updated_guild", { id: "A", name: "A" });
  await new Promise((r) => setTimeout(r, 350));
  expect(calls.filter((c) => c.cmd === "guild_access").length).toBe(1);
  expect(calls.filter((c) => c.cmd === "channels").length).toBe(1);
});

test("连续点击两个语音频道，只加入后点击的频道；取消密码后进行中的加入作废", async () => {
  auto.voice_join = () => 1;
  auto.channels = () => [raw("A-t1", "t1"), vraw("V1"), vraw("V2", [], { hasPassword: true })];
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: VIEW | SEND | CONNECT });
  await selectGuild("A");
  const { ACCESS_TTL } = await import("../session/access");
  const now = Date.now;
  Date.now = () => now() + ACCESS_TTL + 1;
  try {
    hold.add("guild_access");
    const a = enterVoice("V1");
    const b = enterVoice("V2");
    await flush();
    while (pending.some((x) => x.cmd === "guild_access")) release("guild_access", access("A", { everyone: VIEW | SEND | CONNECT }));
    await a;
    await b;
    expect(calls.filter((c) => c.cmd === "voice_join").map((c) => c.args.channelId)).toEqual(["V2"]);
  } finally {
    Date.now = now;
    hold.clear();
  }
  await vs.leaveVoice();
});

test("无 CONNECT 的密码频道不弹出密码框也不申请令牌；其他频道的 40310 不弹出", async () => {
  auto.voice_join = ({ channelId }) => (channelId === "V1" ? 1 : 2);
  auto.channels = () => [raw("A-t1", "t1"), vraw("Vno", [{ roleId: 0, allow: 0, deny: CONNECT }], { hasPassword: true }), vraw("V1", [], { hasPassword: true }), vraw("V2", [], { hasPassword: true })];
  auto.guild_access = ({ guildId }) => access(guildId, { everyone: VIEW | SEND | CONNECT });
  await selectGuild("A");
  await enterVoice("Vno");
  expect(calls.some((c) => c.cmd === "voice_join")).toBe(false);
  expect(pwPrompt.channelId).toBe("");
  await enterVoice("V1");
  await enterVoice("V2");
  ev(1, { type: "state", phase: "disconnected", reason: "password" }, "V1");
  expect(pwPrompt.channelId).toBe("");
  ev(2, { type: "state", phase: "disconnected", reason: "password" }, "V2");
  expect(pwPrompt.channelId).toBe("V2");
  cancelPassword();
});

test("撤销后迟到的频道消息推送不写回", async () => {
  auto.channels = () => [raw("A-t1", "t1"), raw("A-t2", "t2")];
  await selectGuild("A");
  await selectChannel("A-t2");
  auto.channels = () => [raw("A-t1", "t1"), withOw("A-t2", "t2", [{ roleId: 0, allow: 0, deny: VIEW }])];
  await (await import("../session/nav")).reloadChannels("A");
  onEvent({ type: "CHANNEL_MSG", channelType: "GROUP", toUserId: "A-t2", msgId: "late1", msgTimestamp: 5, fromUserId: "u", content: JSON.stringify({ content: "x" }), extra: { author: { id: "u" } } });
  expect(session.messages["A-t2"]).toBeUndefined();
  expect(session.unread["A-t2"]).toBeUndefined();
});

test("窗口恢复的刷新复用在途请求并节流", async () => {
  await selectGuild("A");
  const { refreshAccess: r, ACCESS_TTL } = await import("../session/access");
  const now = Date.now;
  Date.now = () => now() + ACCESS_TTL + 1;
  try {
    calls.length = 0;
    hold.add("guild_access");
    const p1 = r("A");
    const p2 = r("A");
    expect(calls.filter((c) => c.cmd === "guild_access").length).toBe(1);
    release("guild_access", access("A"));
    await Promise.all([p1, p2]);
    hold.clear();
    await r("A");
    expect(calls.filter((c) => c.cmd === "guild_access").length).toBe(1);
  } finally {
    Date.now = now;
  }
});

test("服务器文件夹：读取、失败降级与迟到结果", async () => {
  const { loadFolders } = await import("../session");
  auto.guild_folders = () => ({ bad: true });
  await loadFolders();
  expect(session.folders).toEqual([]);
  expect(session.foldersFailed).toBe(false);
  hold.add("guild_folders");
  const first = loadFolders();
  const second = loadFolders();
  release("guild_folders", [{ id: "f", name: "n", color: null, guild_ids: ["g1"] }]);
  release("guild_folders", [{ id: "", name: "", color: null, guild_ids: ["g1"] }]);
  await Promise.all([first, second]);
  expect(session.folders).toEqual([{ id: "", name: "", color: null, guildIds: ["g1"] }]);
  hold.clear();
  auto.guild_folders = () => { throw new Error("x"); };
  await loadFolders();
  expect(session.foldersFailed).toBe(true);
  expect(session.folders).toEqual([{ id: "", name: "", color: null, guildIds: ["g1"] }]);
  hold.add("guild_folders");
  const late = loadFolders();
  await logout();
  release("guild_folders", [{ id: "f", name: "n", color: null, guild_ids: ["g1"] }]);
  await late;
  expect(session.folders).toBeNull();
  expect(calls.some((c) => c.cmd === "guild_folders" && c.args)).toBe(false);
});

// 语音频道在线成员

const ids2 = (c: { users?: { id: string }[] }) => (c.users ?? []).map((u) => u.id);
const voiceOf = (gid: string) => session.guilds.find((g) => g.id === gid)!.groups.flatMap((g) => g.channels).find((c) => c.type === "voice")!;

test("频道列表中的语音成员显示在频道下并记入已知用户", async () => {
  auto.channels = ({ guildId }) => [{ ...raw(`${guildId}-voice`, "v", 2), users: [{ id: "u1", username: "a", nickname: "甲", identifyNum: "1", avatar: "" }] }, raw(`${guildId}-t1`, "t1")];
  await selectGuild("A");
  expect(voiceOf("A").users?.map((u) => [u.id, u.name])).toEqual([["u1", "甲"]]);
});

test("进出语音频道事件更新成员，加入其他频道时从原频道移出", async () => {
  auto.channels = ({ guildId }) => [raw(`${guildId}-voice`, "v", 2), raw(`${guildId}-voice2`, "v2", 2), raw(`${guildId}-t1`, "t1")];
  await selectGuild("A");
  const sysEv = (type: string, body: unknown) => onEvent({ type: "SYS_MSG", content: JSON.stringify({ type, body }) });
  const voices = () => session.guilds[0].groups.flatMap((g) => g.channels).filter((c) => c.type === "voice");
  sysEv("joined_channel", { id: "u2", username: "b", channel: { id: "A-voice", guild_id: "A" } });
  expect(ids2(voices()[0])).toEqual(["u2"]);
  sysEv("joined_channel", { id: "u2", username: "b", nickname: "乙", channel: { id: "A-voice", guild_id: "A" } });
  expect(voices()[0].users?.map((u) => u.name)).toEqual(["乙"]);
  sysEv("joined_channel", { id: "u2", username: "b", channel: { id: "A-voice2", guild_id: "A" } });
  expect(ids2(voices()[0])).toEqual([]);
  expect(ids2(voices()[1])).toEqual(["u2"]);
  sysEv("exited_channel", { id: "u2", channel: { id: "A-voice2", guild_id: "A" } });
  expect(ids2(voices()[1])).toEqual([]);
});

test("未加载服务器的进出事件被忽略", () => {
  const before = JSON.stringify(session.guilds);
  onEvent({ type: "SYS_MSG", content: JSON.stringify({ type: "joined_channel", body: { id: "u3", channel: { id: "x", guild_id: "zz" } } }) });
  expect(JSON.stringify(session.guilds)).toBe(before);
});

// 文字频道预览、作者名称与系统消息

const textOf = (gid: string, id: string) => session.guilds.find((g) => g.id === gid)!.groups.flatMap((g) => g.channels).find((c) => c.id === id)!;
const pushCh = (channelId: string, msgId: string, extra: Record<string, unknown>) =>
  onEvent({ type: "CHANNEL_MSG", channelType: "GROUP", toUserId: channelId, msgId, msgTimestamp: Date.now(), fromUserId: "u", content: JSON.stringify({ content: "x", extra: JSON.stringify(extra) }) });

test("频道预览取自频道列表并随推送更新", async () => {
  auto.channels = ({ guildId }) => [{ ...raw(`${guildId}-t1`, "t1"), preview: "甲：旧" }, raw(`${guildId}-t2`, "t2")];
  await selectGuild("A");
  expect(textOf("A", "A-t1").preview).toBe("甲：旧");
  expect(textOf("A", "A-t2").preview).toBeUndefined();
  pushCh("A-t2", "m-new", { author: { id: "u", username: "u", nickname: "服务器昵称" }, last_msg_content: "服务器昵称：新" });
  expect(textOf("A", "A-t2").preview).toBe("服务器昵称：新");
  pushCh("A-t1", "m-open", { author: { id: "u", username: "u", nickname: "服务器昵称" }, last_msg_content: "服务器昵称：x" });
  expect(session.messages["A-t1"]?.slice(-1)[0]?.name).toBe("服务器昵称");
});

test("删除最新消息后重新拉取频道列表", async () => {
  auto.messages = () => pg([m("a", 10), m("b", 20)]);
  await selectGuild("A");
  calls.length = 0;
  onEvent({ type: "SYS_MSG", content: JSON.stringify({ type: "deleted_message", body: { rong_id: "a", channel_id: "A-t1", guild_id: "A" } }) });
  await new Promise((r) => setTimeout(r, 350));
  expect(calls.some((c) => c.cmd === "channels")).toBe(false);
  onEvent({ type: "SYS_MSG", content: JSON.stringify({ type: "deleted_message", body: { rong_id: "b", channel_id: "A-t1", guild_id: "A" } }) });
  await new Promise((r) => setTimeout(r, 350));
  expect(calls.some((c) => c.cmd === "channels")).toBe(true);
});

test("历史消息带作者服务器昵称与系统标记", async () => {
  auto.messages = () => pg([{ id: "s1", rong_id: "s1", type: 9, content: "欢迎", create_at: 1, author: { id: "3900", username: "系统", is_sys: true } }, { id: "n1", rong_id: "n1", type: 9, content: "hi", create_at: 2, author: { id: "u9", username: "全局名", nickname: "服务器名" } }]);
  await selectGuild("A");
  const [s, n] = session.messages["A-t1"];
  expect(s.system).toBe(true);
  expect(n.system).toBeUndefined();
  expect(n.name).toBe("服务器名");
});

const effectIds = () => calls.filter((c) => c.cmd === "effect_play").map((c) => c.args.id);

test("提示音：连接、成员进出与主动断开", async () => {
  const realNow = Date.now;
  let now = 1_000_000;
  Date.now = () => now;
  try {
    auto.voice_join = () => 1;
    await vs.joinVoice("V1");
    ev(1, { type: "state", phase: "connected" });
    // 本人信息到达后才播放进入提示音。
    expect(effectIds()).toEqual([]);
    ev(1, { type: "joined", me: peer("me"), peers: [] });
    expect(effectIds()).toEqual([1]);
    // 连接后 1 秒内不播放成员进出。
    ev(1, { type: "peer_joined", peer: peer("u2") });
    expect(effectIds()).toEqual([1]);
    now += 1500;
    ev(1, { type: "peer_joined", peer: peer("u3") });
    ev(1, { type: "peer_joined", peer: peer("u3") });
    ev(1, { type: "peer_left", id: "u2" });
    ev(1, { type: "peer_left", id: "nobody" });
    expect(effectIds()).toEqual([1, 1, 2]);
    // 重连恢复不播放进入提示音。
    ev(1, { type: "state", phase: "reconnecting" });
    ev(1, { type: "state", phase: "connected" });
    expect(effectIds()).toEqual([1, 1, 2]);
    await vs.disconnectVoice();
    expect(effectIds()).toEqual([1, 1, 2, 3]);
  } finally {
    Date.now = realNow;
  }
});

test("提示音：开关麦与耳机", async () => {
  auto.voice_join = () => 1;
  await vs.joinVoice("V1");
  ev(1, { type: "state", phase: "connected" });
  calls.length = 0;
  await vs.setMuted(true);
  await vs.setMuted(false);
  await vs.setDeafened(true);
  await vs.setDeafened(false);
  expect(effectIds()).toEqual([4, 5, 8, 9]);
});

test("提示音：消息通知按会话与提及选择", async () => {
  setSession("me", { id: "me" } as any);
  const note = (extra: Record<string, unknown>) => onEvent({ type: "SYS_MSG", content: JSON.stringify({ type: "desktop_notification", body: { extra } }) });
  calls.length = 0;
  note({ msg_id: "m1", channel_id: "C9", author_id: 5 });
  note({ msg_id: "m1", channel_id: "C9", author_id: 5 });
  note({ msg_id: "m2", channel_id: "C9", author_id: 5, mention: true });
  note({ msg_id: "m3", chat_user_id: 5 });
  note({ msg_id: "m4", channel_id: "C9", author_id: "me" });
  note({ msg_id: "m5", channel_id: "C9", author_id: 5, gift_triggered: true });
  expect(effectIds()).toEqual([10, 13, 10]);
});

test("提示音：成员装扮音效", async () => {
  const realNow = Date.now;
  let now = 2_000_000;
  Date.now = () => now;
  try {
    auto.voice_join = () => 1;
    await vs.joinVoice("V1");
    ev(1, { type: "joined", me: peer("me", { joinVoice: 10148 }), peers: [] });
    ev(1, { type: "state", phase: "connected" });
    now += 1500;
    ev(1, { type: "peer_joined", peer: peer("u4", { joinVoice: 10011 }) });
    ev(1, { type: "peer_joined", peer: peer("u5") });
    const items = calls.filter((c) => c.cmd === "effect_play_item").map((c) => c.args.item);
    expect(items).toEqual([10148, 10011]);
    expect(effectIds()).toEqual([1]);
  } finally {
    Date.now = realNow;
  }
});
