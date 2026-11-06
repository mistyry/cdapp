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

// 拖拽区属性值。"" 只在点到元素本身时拖动，"deep" 覆盖整个子树，按钮等控件自动排除。
// 不叠加标题栏时不输出属性，原生标题栏下双击页面不会缩放窗口。
export const dragAttr = (inset: boolean, v: "" | "deep") => (inset ? v : undefined);
