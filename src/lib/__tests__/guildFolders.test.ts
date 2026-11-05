import { expect, test } from "bun:test";
import { buildRail, folderTint, parseFolders } from "../guildFolders";

const raw = [
  { id: "", name: "", color: null, guild_ids: ["a"] },
  { id: "f1", name: "组一", color: 9408511, guild_ids: ["b", "c", "d", "e", "f"] },
  { id: "", name: "", color: null, guild_ids: ["g"] },
];

test("解析接口条目并保留顺序", () => {
  expect(parseFolders(raw)).toEqual([
    { id: "", name: "", color: null, guildIds: ["a"] },
    { id: "f1", name: "组一", color: 9408511, guildIds: ["b", "c", "d", "e", "f"] },
    { id: "", name: "", color: null, guildIds: ["g"] },
  ]);
});

test("格式错误的条目跳过", () => {
  expect(parseFolders(null)).toEqual([]);
  expect(parseFolders({ items: [] })).toEqual([]);
  expect(parseFolders([null, 3, { id: "x" }, { id: 5, guild_ids: [1, null, "2"], color: "red" }])).toEqual([{ id: "5", name: "", color: null, guildIds: ["1", "2"] }]);
});

test("按文件夹顺序排列，未归组为独立服务器", () => {
  const rail = buildRail(["g", "f", "e", "d", "c", "b", "a"], parseFolders(raw));
  expect(rail).toEqual([
    { kind: "guild", id: "a" },
    { kind: "folder", id: "f1", name: "组一", color: 9408511, guildIds: ["b", "c", "d", "e", "f"] },
    { kind: "guild", id: "g" },
  ]);
});

test("失效、重复引用与空文件夹", () => {
  const rail = buildRail(["a", "b", "c"], [
    { id: "f1", name: "", color: null, guildIds: ["x", "a", "a"] },
    { id: "f2", name: "", color: null, guildIds: ["a", "y"] },
    { id: "f1", name: "", color: null, guildIds: ["b"] },
    { id: "", name: "", color: null, guildIds: ["a", "b"] },
  ]);
  expect(rail).toEqual([
    { kind: "folder", id: "f1", name: "", color: null, guildIds: ["a"] },
    { kind: "guild", id: "b" },
    { kind: "guild", id: "c" },
  ]);
});

test("文件夹缺失或读取失败时为平铺列表", () => {
  expect(buildRail(["a", "b"], null)).toEqual([{ kind: "guild", id: "a" }, { kind: "guild", id: "b" }]);
  expect(buildRail([], parseFolders(raw))).toEqual([]);
});

test("未被文件夹覆盖的服务器追加在末尾", () => {
  expect(buildRail(["n", "a"], [{ id: "", name: "", color: null, guildIds: ["a"] }])).toEqual([{ kind: "guild", id: "a" }, { kind: "guild", id: "n" }]);
});

test("文件夹底色", () => {
  expect(folderTint(0x8f8fff, 0.3)).toBe("rgba(143, 143, 255, 0.3)");
  expect(folderTint(null, 0.3)).toContain("var(--c-brand) 30%");
  expect(folderTint(-1, 0.3)).toContain("var(--c-brand)");
});
