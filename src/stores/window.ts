import { createSignal } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { dragAttr, isMac, isWindows } from "../lib/platform";

const custom = isMac || isWindows;
const [fullscreen, setFullscreen] = createSignal(false);
const [maximized, setMaximized] = createSignal(false);

export const titleInset = () => custom && !fullscreen();
export { maximized };

export const TITLE_H = 36;

export const titleTop = () => (titleInset() ? TITLE_H : 0);

export const dragRegion = (v: "" | "deep" = "") => dragAttr(titleInset(), v);

export const minimize = () => void getCurrentWindow().minimize();
export const toggleMaximize = () => void getCurrentWindow().toggleMaximize();
export const closeWindow = () => void getCurrentWindow().close();

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
