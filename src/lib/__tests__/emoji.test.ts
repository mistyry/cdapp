import { expect, test } from "bun:test";
import { emojiMarkup, singleEmoji, fromCompact, gridMove, insertText, parseRecent, pushRecent, reactionOf, search, serialize, tokenize, unavailable, type Pick } from "../emoji";
import { parse } from "../kmarkdown";

const own: Pick = { kind: "custom", id: "100/abc", name: "笑", animated: false, guildId: "100" };
const other: Pick = { kind: "custom", id: "200/xyz", name: "哭", animated: true, guildId: "200" };
const mine: Pick = { kind: "custom", id: "1/22/pp", name: "我", animated: false, guildId: "" };
const uni: Pick = { kind: "unicode", char: "👍🏽", name: "拇指向上: 中等肤色" };

test("按用途区分可用范围", () => {
  const ch = { use: "send", guildId: "100" } as const;
  expect(unavailable(uni, ch)).toBeNull();
  expect(unavailable(own, ch)).toBeNull();
  expect(unavailable(mine, ch)).toBeNull();
  expect(unavailable(other, ch)).toBe("只能在所属服务器中使用");
  const dm = { use: "send", guildId: null } as const;
  expect(unavailable(own, dm)).toBe("私信中不能发送服务器表情");
  expect(unavailable(mine, dm)).toBeNull();
  const react = { use: "react", guildId: "100" } as const;
  expect(unavailable(own, react)).toBeNull();
  expect(unavailable(mine, react)).toBeNull();
  expect(unavailable(mine, { use: "react", guildId: null })).toBeNull();
  expect(unavailable(other, react)).not.toBeNull();
  expect(unavailable(own, { use: "react", guildId: null })).not.toBeNull();
  expect(unavailable(uni, { use: "react", guildId: null })).toBeNull();
});

test("回应参数：Unicode 传字符，服务器表情传 ID", () => {
  expect(reactionOf(uni)).toEqual({ id: "👍🏽", name: "👍🏽" });
  expect(reactionOf(own)).toEqual({ id: "100/abc", name: "笑" });
});

test("插入与序列化往返，并能被 KMarkdown 解析", () => {
  const draft = `你好${insertText(own)}${insertText(uni)}\n${insertText(mine)}末尾`;
  expect(serialize(tokenize(draft))).toBe(draft);
  const emj = parse(draft).flatMap((b) => (b.t === "p" ? b.c : [])).filter((n) => n.t === "emj");
  expect(emj).toEqual([{ t: "emj", name: "笑", id: "100/abc" }, { t: "emj", name: "我", id: "1/22/pp" }]);
});

test("同名表情以 ID 区分", () => {
  const t = tokenize(`${emojiMarkup("笑", "1/a")}${emojiMarkup("笑", "2/b")}`);
  expect(t).toEqual([{ t: "emj", name: "笑", id: "1/a" }, { t: "emj", name: "笑", id: "2/b" }]);
});

test("名称中破坏结构的字符被去掉，非法标记保持为文字", () => {
  expect(emojiMarkup("a(b)[c]\nd", "1/x")).toBe("(emj)abcd(emj)[1/x]");
  expect(emojiMarkup("()", "1/x")).toBe("(emj)表情(emj)[1/x]");
  const bad = "(emj)x(emj)[../evil] (emj)y(emj)[1/<b>]";
  expect(tokenize(bad)).toEqual([{ t: "text", v: bad }]);
});

test("单个表情才放大，含肤色、组合与旗帜；多个或混排不放大", () => {
  for (const x of ["👍🏽", " 👨‍👩‍👧 ", "🇨🇳", `  ${emojiMarkup("笑", "1/a")}\n`]) expect(singleEmoji(x)).toBe(true);
  for (const x of ["😀😀", `${emojiMarkup("笑", "1/a")}😀`, "好😀", "a", "1", "   ", "", "😀 😀"]) expect(singleEmoji(x)).toBe(false);
});

test("搜索：名称前缀优先，标签可命中，空查询无结果", () => {
  const u = fromCompact([
    { unicode: "😀", label: "笑脸", tags: ["开心"], group: 0, order: 1 },
    { unicode: "😂", label: "笑哭了", tags: [], group: 0, order: 2 },
    { unicode: "🦴", label: "骨头", tags: ["笑"], group: 3, order: 3 },
    { unicode: "🏻", label: "较浅肤色", tags: [], group: 2, order: 0 },
  ]);
  expect(u.map((x) => x.char)).toEqual(["😀", "😂", "🦴"]);
  const r = search(u, [{ id: "100/abc", name: "笑", animated: false, guildId: "100" }], "笑");
  expect(r.map((x) => (x.kind === "unicode" ? x.char : x.id))).toEqual(["100/abc", "😀", "😂", "🦴"]);
  expect(search(u, [], "开心").length).toBe(1);
  expect(search(u, [], " ")).toEqual([]);
});

test("最近使用去重、限量，损坏数据丢弃", () => {
  let r: Pick[] = [];
  for (let i = 0; i < 30; i++) r = pushRecent(r, { kind: "unicode", char: String(i), name: "" });
  r = pushRecent(r, { kind: "unicode", char: "5", name: "" });
  expect(r.length).toBe(24);
  expect(r[0]).toMatchObject({ char: "5" });
  expect(r.filter((x) => x.kind === "unicode" && x.char === "5").length).toBe(1);
  expect(parseRecent(JSON.stringify([own, { kind: "custom", id: "../x", name: "", guildId: "", animated: false }, { kind: "x" }, uni]))).toEqual([own, uni]);
  expect(parseRecent("not json")).toEqual([]);
  expect(parseRecent(null)).toEqual([]);
});

test("方向键按分区网格移动", () => {
  const s = [{ start: 0, len: 3 }, { start: 3, len: 10 }, { start: 13, len: 8 }];
  expect(gridMove(s, 0, 8, 8)).toBe(3);
  expect(gridMove(s, 2, 8, 8)).toBe(5);
  expect(gridMove(s, 5, 8, 8)).toBe(12);
  expect(gridMove(s, 11, 8, 8)).toBe(13 + 0);
  expect(gridMove(s, 13, -8, 8)).toBe(11);
  expect(gridMove(s, 20, -8, 8)).toBe(12);
  expect(gridMove(s, 3, -8, 8)).toBe(0);
  expect(gridMove(s, 0, -8, 8)).toBe(0);
  expect(gridMove(s, 20, 8, 8)).toBe(20);
  expect(gridMove(s, 4, 1, 8)).toBe(5);
  expect(gridMove([{ start: 0, len: 20 }], 15, 8, 8)).toBe(19);
});
