import { expect, test } from "bun:test";
import { inline, parse, safeUrl, toPlain, MAX_PARSE, type Names } from "../kmarkdown";

const names: Names = { user: (id) => ({ u1: "小明", u2: "小明" } as Record<string, string>)[id], role: (id) => (id === "7" ? "管理" : undefined), channel: (id) => (id === "c1" ? "闲聊" : undefined) };

test("多行中文保留换行", () => {
  expect(parse("第一行\n第二行")).toEqual([{ t: "p", c: [{ t: "text", v: "第一行" }, { t: "br" }, { t: "text", v: "第二行" }] }]);
});

test("同名用户按 ID 区分", () => {
  const n = inline("(met)u1(met)(met)u2(met)(met)nobody(met)");
  expect(n).toEqual([{ t: "met", id: "u1" }, { t: "met", id: "u2" }, { t: "met", id: "nobody" }]);
  expect(toPlain(parse("(met)nobody(met)"), names)).toBe("@未知用户");
  // 普通 @文字不是提及。
  expect(inline("@小明 a@b.com")).toEqual([{ t: "text", v: "@小明 a@b.com" }]);
});

test("强调、删除线、下划线、剧透、角色与频道", () => {
  expect(inline("**粗***斜*~~删~~(ins)下(ins)(spl)秘(spl)(rol)7(rol)(chn)c1(chn)")).toEqual([
    { t: "b", c: [{ t: "text", v: "粗" }] }, { t: "i", c: [{ t: "text", v: "斜" }] }, { t: "s", c: [{ t: "text", v: "删" }] },
    { t: "u", c: [{ t: "text", v: "下" }] }, { t: "spl", c: [{ t: "text", v: "秘" }] }, { t: "rol", id: "7" }, { t: "chn", id: "c1" },
  ]);
  expect(inline("2 * 3 * 4")).toEqual([{ t: "text", v: "2 * 3 * 4" }]);
});

test("代码块内不解析标记", () => {
  const b = parse("前\n```js\n**x** <b>(met)u1(met)\n```\n后");
  expect(b[1]).toEqual({ t: "pre", lang: "js", v: "**x** <b>(met)u1(met)" });
  expect(inline("`**a**`")).toEqual([{ t: "code", v: "**a**" }]);
  expect(parse("```\n未闭合")[0].t).toBe("p");
});

test("特殊字符与 HTML 按文字保留", () => {
  expect(inline("<script>alert(1)</script> &amp;")).toEqual([{ t: "text", v: "<script>alert(1)</script> &amp;" }]);
  expect(inline("\\*不是斜体\\* \\(met\\)")).toEqual([{ t: "text", v: "*不是斜体* (met)" }]);
});

test("异常链接不可点击", () => {
  expect(safeUrl("javascript:alert(1)")).toBeNull();
  expect(safeUrl("JaVaScRiPt:x")).toBeNull();
  expect(safeUrl("data:text/html,x")).toBeNull();
  expect(safeUrl("https://")).toBeNull();
  expect(inline("[点我](javascript:alert(1))").some((n) => n.t === "link")).toBe(false);
  expect(inline("[官网](https://kookapp.cn)")).toEqual([{ t: "link", href: "https://kookapp.cn/", c: [{ t: "text", v: "官网" }] }]);
  expect(inline("看 https://a.com/x?y=1。")).toEqual([{ t: "text", v: "看 " }, { t: "link", href: "https://a.com/x?y=1", c: [{ t: "text", v: "https://a.com/x?y=1" }] }, { t: "text", v: "。" }]);
});

test("表情与引用块", () => {
  expect(inline("(emj)赞(emj)[123/abc]")).toEqual([{ t: "emj", name: "赞", id: "123/abc" }]);
  expect(inline("(emj)赞(emj)[bad/id!]")).toEqual([{ t: "text", v: "赞" }]);
  expect(parse("> 引用\n> 二行\n\n正文")[0]).toEqual({ t: "quote", c: [{ t: "p", c: [{ t: "text", v: "引用" }, { t: "br" }, { t: "text", v: "二行" }] }] });
});

test("未闭合标记与超长文本稳定", () => {
  expect(inline("**a (met)x [b](")).toEqual([{ t: "text", v: "**a (met)x [b](" }]);
  const big = "*(met)[**~~(spl)`".repeat(5000);
  const t0 = performance.now();
  const b = parse(big);
  expect(performance.now() - t0).toBeLessThan(2000);
  expect(b.length).toBeGreaterThan(0);
  const long = "啊".repeat(MAX_PARSE + 10);
  expect(toPlain(parse(long), names).length).toBe(MAX_PARSE + 10);
});

test("引用跨单个换行延续，空行结束", () => {
  expect(parse("> 第一行\n第二行\n\n正文")[0]).toEqual({ t: "quote", c: [{ t: "p", c: [{ t: "text", v: "第一行" }, { t: "br" }, { t: "text", v: "第二行" }] }] });
  expect(parse("> a\nb\n\nc")[0]).toEqual({ t: "quote", c: [{ t: "p", c: [{ t: "text", v: "a" }, { t: "br" }, { t: "text", v: "b" }] }] });
  expect(toPlain(parse("> a\nb\n\nc").slice(1), names)).toBe("c");
});

test("链接地址支持转义与配对括号", () => {
  expect(inline("[x](https://a.com/p\\(1\\))")).toEqual([{ t: "link", href: "https://a.com/p(1)", c: [{ t: "text", v: "x" }] }]);
  expect(inline("[x](https://a.com/wiki/Foo_(bar))")).toEqual([{ t: "link", href: "https://a.com/wiki/Foo_(bar)", c: [{ t: "text", v: "x" }] }]);
});

test("大量无效裸链接线性处理", () => {
  const s = "http://%".repeat(2500);
  const t = performance.now();
  const b = parse(s);
  expect(performance.now() - t).toBeLessThan(50);
  expect(toPlain(b, names)).toBe(s);
  const long = "https://a.com/" + "a".repeat(5000);
  expect(inline(long)[0]).toMatchObject({ t: "link" });
});

test("文件卡片解析为 file 部分，非 http 地址不可点击", async () => {
  const { parseCards } = await import("../content");
  const c = parseCards(JSON.stringify([{ type: "card", theme: "none", size: "lg", modules: [{ type: "file", title: "a.zip", src: "https://x/a.zip", size: 10, canDownload: true }] }]))!;
  expect(c[0].other).toBe(false);
  expect(c[0].parts[0]).toEqual({ t: "file", name: "a.zip", url: "https://x/a.zip", size: 10 });
  const bad = parseCards(JSON.stringify([{ modules: [{ type: "file", title: "b", src: "javascript:x" }] }]))!;
  expect(bad[0].parts[0]).toMatchObject({ t: "file", url: null });
});
