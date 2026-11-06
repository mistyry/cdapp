import { createSignal } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { dragAttr, isMac, isWindows } from "../lib/platform";

// 自绘窗口标题栏：页面顶部 36px，左侧为应用标识与名称，拖动窗口由页面中的拖拽区负责。
// macOS 使用 Overlay 标题栏，红绿灯在标题栏内垂直居中，与 tauri.macos.conf.json 的 trafficLightPosition 对应。
// Windows 窗口无边框，右侧绘制最小化、最大化与关闭按钮，见 tauri.windows.conf.json。
// 全屏时移除标题栏与拖拽区，避免全屏中双击改变窗口尺寸。其他平台使用原生窗口标题栏。
const custom = isMac || isWindows;
const [fullscreen, setFullscreen] = createSignal(false);
const [maximized, setMaximized] = createSignal(false);

export const titleInset = () => custom && !fullscreen();
export { maximized };

export const TITLE_H = 36;

// 页面顶部被标题栏占用的高度，弹层定位时避开。
export const titleTop = () => (titleInset() ? TITLE_H : 0);

export const dragRegion = (v: "" | "deep" = "") => dragAttr(titleInset(), v);

export const minimize = () => void getCurrentWindow().minimize();
export const toggleMaximize = () => void getCurrentWindow().toggleMaximize();
export const closeWindow = () => void getCurrentWindow().close();

// 跟踪全屏与最大化状态。返回注销函数。
export async function watchWindow(): Promise<() => void> {
  if (!custom) return () => {};
  const w = getCurrentWindow();
  const sync = () => {
    void w.isFullscreen().then(setFullscreen, () => {});
    if (isWindows) void w.isMaximized().then(setMaximized, () => {});
  };
  sync();
  return w.onResized(sync);
}
