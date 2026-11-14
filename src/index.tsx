/* @refresh reload */
import { render } from "solid-js/web";
import { hasBackdrop, platform } from "./lib/platform";
import "./styles/theme.css";
import "./styles/layout.css";

const pip = new URLSearchParams(location.search).has("pip");

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

const root = document.getElementById("root") as HTMLElement;
if (pip) {
  document.documentElement.dataset.pip = "";
  void import("./components/layout/PipView").then(({ default: PipView }) => render(() => <PipView />, root));
} else {
  void import("./App").then(({ default: App }) => render(() => <App />, root));
}

document.addEventListener("dragstart", (e) => {
  if (e.target instanceof Element && e.target.closest("img, svg, a")) e.preventDefault();
});
