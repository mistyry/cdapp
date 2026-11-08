import { expect, test } from "bun:test";
import { dragAttr, hasBackdrop, platformOf } from "../platform";

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

test("系统材质：macOS 始终有，Windows 按系统版本判定，失败按不支持", async () => {
  const nav = globalThis.navigator as unknown as Record<string, unknown>;
  const saved = nav.userAgentData;
  const ua = (platformVersion: string | Error) => ({
    getHighEntropyValues: () => (platformVersion instanceof Error ? Promise.reject(platformVersion) : Promise.resolve({ platformVersion })),
  });
  try {
    expect(await hasBackdrop("macos")).toBe(true);
    expect(await hasBackdrop("linux")).toBe(false);
    nav.userAgentData = ua("15.0.0");
    expect(await hasBackdrop("windows")).toBe(true);
    nav.userAgentData = ua("10.0.0");
    expect(await hasBackdrop("windows")).toBe(false);
    nav.userAgentData = ua(new Error("denied"));
    expect(await hasBackdrop("windows")).toBe(false);
    nav.userAgentData = undefined;
    expect(await hasBackdrop("windows")).toBe(false);
  } finally {
    nav.userAgentData = saved;
  }
});
