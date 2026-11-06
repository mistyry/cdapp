/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import { platform } from "./lib/platform";

// 原生拖放已关闭：未被聊天区接收的文件拖放一律取消，避免窗口导航到 file:// 地址而丢失应用状态。
const isFileDrag = (e: DragEvent) => [...(e.dataTransfer?.types ?? [])].includes("Files");
for (const type of ["dragover", "drop"] as const) {
  window.addEventListener(type, (e) => {
    if (isFileDrag(e)) e.preventDefault();
  });
}

// 平台差异的样式按此标记区分：macOS 窗口透明并铺有系统材质，Windows 标题栏不为红绿灯留位，见 theme.css 与 layout.css。
if (platform === "macos" || platform === "windows") document.documentElement.dataset.platform = platform;

render(() => <App />, document.getElementById("root") as HTMLElement);
