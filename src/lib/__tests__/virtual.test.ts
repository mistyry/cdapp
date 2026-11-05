import { expect, test } from "bun:test";
import { indexAt, prefix, visibleRange } from "../virtual";

test("偏移与定位", () => {
  const o = prefix([10, 20, 30]);
  expect(o).toEqual([0, 10, 30, 60]);
  expect(indexAt(o, 0)).toBe(0);
  expect(indexAt(o, 9)).toBe(0);
  expect(indexAt(o, 10)).toBe(1);
  expect(indexAt(o, 59)).toBe(2);
  expect(indexAt(o, 999)).toBe(2);
});

test("空列表与短列表", () => {
  expect(visibleRange(prefix([]), 0, 500, 100)).toEqual([0, 0]);
  expect(visibleRange(prefix([10, 10]), 0, 500, 100)).toEqual([0, 2]);
});

test("5000 条时渲染数量受可视区约束，滚到任意位置都连续覆盖可视区", () => {
  const h = Array.from({ length: 5000 }, (_, i) => 40 + (i % 7) * 13);
  const o = prefix(h);
  for (let top = 0; top < o[5000]; top += 997) {
    const [s, e] = visibleRange(o, top, 800, 400);
    expect(e - s).toBeLessThan(60);
    expect(o[s]).toBeLessThanOrEqual(Math.max(0, top - 400));
    expect(o[e]).toBeGreaterThanOrEqual(Math.min(o[5000], top + 800));
  }
});
