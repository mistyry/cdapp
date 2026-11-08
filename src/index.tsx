/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import { hasBackdrop, platform } from "./lib/platform";

// 原生拖放已关闭：未被聊天区接收的文件拖放一律取消，避免窗口导航到 file:// 地址而丢失应用状态。
const isFileDrag = (e: DragEvent) => [...(e.dataTransfer?.types ?? [])].includes("Files");
for (const type of ["dragover", "drop"] as const) {
  window.addEventListener(type, (e) => {
    if (isFileDrag(e)) e.preventDefault();
  });
}

// 平台差异的样式按此标记区分：Windows 标题栏不为红绿灯留位，见 layout.css。
// 窗口底层有系统材质时标记 data-backdrop，底色改为半透明，见 theme.css。
if (platform === "macos" || platform === "windows") document.documentElement.dataset.platform = platform;
void hasBackdrop().then((ok) => {
  if (ok) document.documentElement.dataset.backdrop = "";
});

render(() => <App />, document.getElementById("root") as HTMLElement);

// 图片、图标与链接不可拖出，选中的消息文字仍可拖动。
document.addEventListener("dragstart", (e) => {
  if (e.target instanceof Element && e.target.closest("img, svg, a")) e.preventDefault();
});
