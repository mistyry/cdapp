/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";

// 原生拖放已关闭：未被聊天区接收的文件拖放一律取消，避免窗口导航到 file:// 地址而丢失应用状态。
const isFileDrag = (e: DragEvent) => [...(e.dataTransfer?.types ?? [])].includes("Files");
for (const type of ["dragover", "drop"] as const) {
  window.addEventListener(type, (e) => {
    if (isFileDrag(e)) e.preventDefault();
  });
}

render(() => <App />, document.getElementById("root") as HTMLElement);
