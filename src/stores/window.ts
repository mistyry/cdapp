import { createSignal } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { dragAttr, isMac } from "../lib/platform";

// macOS 窗口标题栏叠在页面上：顶部 36px 为标题栏，左侧红绿灯，其后为应用标识与名称，拖动窗口由页面中的拖拽区负责。
// 红绿灯在标题栏内垂直居中，与 tauri.conf.json 的 trafficLightPosition 对应。
// 全屏时系统隐藏红绿灯，留白与拖拽区一并移除，避免全屏中双击改变窗口尺寸。
// 其他平台使用原生窗口标题栏，不输出拖拽区。
const [fullscreen, setFullscreen] = createSignal(false);

export const titleInset = () => isMac && !fullscreen();

export const TITLE_H = 36;

// 页面顶部被标题栏占用的高度，弹层定位时避开。
export const titleTop = () => (titleInset() ? TITLE_H : 0);

export const dragRegion = (v: "" | "deep" = "") => dragAttr(titleInset(), v);

// 跟踪全屏状态。返回注销函数。
export async function watchWindow(): Promise<() => void> {
  if (!isMac) return () => {};
  const w = getCurrentWindow();
  const sync = () => void w.isFullscreen().then(setFullscreen, () => {});
  sync();
  return w.onResized(sync);
}
