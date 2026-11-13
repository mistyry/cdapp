/* @refresh reload */
import { render } from "solid-js/web";
import App from "./App";
import PipView from "./components/layout/PipView";
import { hasBackdrop, platform } from "./lib/platform";
import "./styles/theme.css";
import "./styles/layout.css";

const pip = Number(new URLSearchParams(location.search).get("pip") ?? 0);

const isFileDrag = (e: DragEvent) => [...(e.dataTransfer?.types ?? [])].includes("Files");
for (const type of ["dragover", "drop"] as const) {
  window.addEventListener(type, (e) => {
    if (isFileDrag(e)) e.preventDefault();
  });
}

if (platform === "macos" || platform === "windows") document.documentElement.dataset.platform = platform;
void hasBackdrop().then((ok) => {
  if (ok) document.documentElement.dataset.backdrop = "";
});

if (pip > 0) {
  document.documentElement.dataset.pip = "";
  render(() => <PipView generation={pip} />, document.getElementById("root") as HTMLElement);
} else {
  render(() => <App />, document.getElementById("root") as HTMLElement);
}

document.addEventListener("dragstart", (e) => {
  if (e.target instanceof Element && e.target.closest("img, svg, a")) e.preventDefault();
});
