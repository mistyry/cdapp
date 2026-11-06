import { expect, test } from "bun:test";
import { dragAttr, platformOf } from "../platform";

test("按 WebView 的 UA 判定平台", () => {
  expect(platformOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)")).toBe("macos");
  expect(platformOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36 Edg/142.0.0.0")).toBe("windows");
  expect(platformOf("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)")).toBe("linux");
  expect(platformOf("Bun/1.4.4")).toBe("other");
});

test("不叠加标题栏时不输出拖拽区", () => {
  expect(dragAttr(false, "")).toBeUndefined();
  expect(dragAttr(false, "deep")).toBeUndefined();
  expect(dragAttr(true, "deep")).toBe("deep");
});
