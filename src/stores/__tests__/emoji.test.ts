import { beforeEach, expect, mock, test } from "bun:test";

const accounts = new Map<string, Record<string, unknown>>();
const pending: Array<{ cmd: string; resolve: (v: unknown) => void; reject: (e: unknown) => void }> = [];
const calls: string[] = [];
mock.module("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: any) => {
    calls.push(cmd);
    if (cmd === "account_state") return Promise.resolve({ ...accounts.get(args.user) });
    if (cmd === "set_account_state") {
      accounts.set(args.user, { ...accounts.get(args.user), [args.key]: args.value });
      return Promise.resolve(null);
    }
    if (cmd === "emojis") return new Promise((resolve, reject) => pending.push({ cmd, resolve, reject }));
    return Promise.resolve(null);
  },
  Channel: class { onmessage: ((b: unknown) => void) | null = null; },
}));
mock.module("@tauri-apps/api/event", () => ({ listen: async () => () => {}, emit: async () => {} }));
const store = new Map<string, string>();
(globalThis as any).localStorage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v), removeItem: (k: string) => store.delete(k) };

const { emoji, ensureEmojis, noteRecent, customOf, shownGuilds } = await import("../emoji");
const { setSession, logout } = await import("../session");

const g = (gid: string, n = 1) => ({ guildId: gid, name: gid, icon: "", items: Array.from({ length: n }, (_, i) => ({ id: `${gid}/e${i}`, name: "a", animated: false })) });
const set = (gid: string) => ({ guilds: [{ guildId: gid, name: gid, icon: "", items: [{ id: `${gid}/a`, name: "a", animated: false }] }], personal: [], personalFailed: false, joined: [] as string[] | null });
const flush = () => new Promise((r) => setTimeout(r, 0));
const login = (id: string) => setSession({ phase: "ready", me: { id, name: id, online: true } });

beforeEach(async () => {
  pending.length = 0;
  calls.length = 0;
  await logout();
});

test("重复打开只请求一次，结果可查表", async () => {
  login("u1");
  const a = ensureEmojis();
  void ensureEmojis();
  expect(calls.filter((c) => c === "emojis").length).toBe(1);
  pending[0].resolve(set("G"));
  await a;
  expect(emoji.status).toBe("ready");
  expect(customOf("G/a")).toMatchObject({ guildId: "G" });
  expect(customOf("G/zz")).toBeNull();
});

test("退出后旧账号的迟到结果不写入新账号", async () => {
  login("u1");
  void ensureEmojis();
  const old = pending.shift()!;
  await logout();
  login("u2");
  void ensureEmojis();
  old.resolve(set("OLD"));
  await flush();
  expect(emoji.guilds).toEqual([]);
  pending.shift()!.resolve(set("NEW"));
  await flush();
  expect(emoji.guilds.map((g) => g.guildId)).toEqual(["NEW"]);
});

test("失败后不自动重试，手动重试可恢复", async () => {
  login("u1");
  void ensureEmojis();
  pending.shift()!.reject({ kind: "not_sent", message: "网络错误" });
  await flush();
  expect(emoji.status).toBe("failed");
  void ensureEmojis();
  expect(pending.length).toBe(0);
  void ensureEmojis(true);
  pending.shift()!.resolve(set("G"));
  await flush();
  expect(emoji.status).toBe("ready");
});

test("最近使用按账号保存，互不影响", async () => {
  login("u1");
  void ensureEmojis();
  noteRecent({ kind: "unicode", char: "😀", name: "笑脸" });
  expect(emoji.recent.length).toBe(1);
  await logout();
  login("u2");
  void ensureEmojis();
  expect(emoji.recent).toEqual([]);
  await logout();
  login("u1");
  void ensureEmojis();
  await flush();
  expect(emoji.recent).toEqual([{ kind: "unicode", char: "😀", name: "笑脸" }]);
});

test("旧版本地存储导入数据目录后删除", async () => {
  store.set("cdapp.emoji.recent.u9", JSON.stringify([{ kind: "unicode", char: "👍", name: "赞" }]));
  login("u9");
  void ensureEmojis();
  noteRecent({ kind: "unicode", char: "😀", name: "笑脸" });
  await flush();
  expect(emoji.recent.map((x) => (x as any).char)).toEqual(["😀", "👍"]);
  expect(store.has("cdapp.emoji.recent.u9")).toBe(false);
  expect((accounts.get("u9")!.emojiRecent as any[]).length).toBe(2);
});

test("服务器表情组按已添加顺序显示，当前服务器未添加时置顶，读取失败时显示全部", async () => {
  login("u1");
  void ensureEmojis();
  pending.shift()!.resolve({ guilds: [g("A"), g("B"), g("C"), g("E", 0)], personal: [], personalFailed: false, joined: ["C", "A", "X", "E"] });
  await flush();
  expect(shownGuilds(null).map((x) => x.guildId)).toEqual(["C", "A"]);
  expect(shownGuilds("A").map((x) => x.guildId)).toEqual(["C", "A"]);
  expect(shownGuilds("B").map((x) => x.guildId)).toEqual(["B", "C", "A"]);
  await logout();
  login("u1");
  void ensureEmojis();
  pending.shift()!.resolve({ guilds: [g("A"), g("B")], personal: [], personalFailed: false, joined: null });
  await flush();
  expect(shownGuilds(null).map((x) => x.guildId)).toEqual(["A", "B"]);
});
