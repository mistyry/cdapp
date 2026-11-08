export type Platform = "macos" | "windows" | "linux" | "other";

// 按 WebView 的 UA 判定桌面平台：WKWebView 含 Macintosh，WebView2 含 Windows，WebKitGTK 含 Linux。
export function platformOf(ua: string): Platform {
  if (ua.includes("Macintosh")) return "macos";
  if (ua.includes("Windows")) return "windows";
  if (ua.includes("Linux")) return "linux";
  return "other";
}

export const platform: Platform = typeof navigator === "undefined" ? "other" : platformOf(navigator.userAgent);
export const isMac = platform === "macos";
export const isWindows = platform === "windows";

// 窗口底层是否有系统材质。macOS 始终有；Windows 仅 11 支持 Mica，WebView2 的 UA 仍写 NT 10.0，
// 改按 UA-CH 的 platformVersion 判定，主版本 13 起为 Windows 11。查询失败按不支持处理，保持不透明底色。
export async function hasBackdrop(p: Platform = platform): Promise<boolean> {
  if (p === "macos") return true;
  if (p !== "windows") return false;
  const data = (navigator as Navigator & { userAgentData?: { getHighEntropyValues(h: string[]): Promise<{ platformVersion?: string }> } }).userAgentData;
  try {
    const v = (await data?.getHighEntropyValues(["platformVersion"]))?.platformVersion;
    return Number.parseInt(v ?? "", 10) >= 13;
  } catch {
    return false;
  }
}

// 拖拽区属性值。"" 只在点到元素本身时拖动，"deep" 覆盖整个子树，按钮等控件自动排除。
// 不叠加标题栏时不输出属性，原生标题栏下双击页面不会缩放窗口。
export const dragAttr = (inset: boolean, v: "" | "deep") => (inset ? v : undefined);
