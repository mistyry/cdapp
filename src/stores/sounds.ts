import { createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { Channel } from "@tauri-apps/api/core";
import { api } from "../lib/api";

export const [shareSdkReady, setShareSdkReady] = createSignal(false);

export const [sounds, setSounds] = createStore({
  stage: "idle" as "idle" | "checking" | "downloading" | "done",
  done: 0,
  total: 0,
});

export async function syncSounds() {
  if (sounds.stage === "checking" || sounds.stage === "downloading") return;
  setSounds({ stage: "checking", done: 0, total: 0 });
  const progress = new Channel<{ done: number; total: number }>();
  progress.onmessage = (p) => setSounds({ stage: p.total > 0 && p.done < p.total ? "downloading" : "checking", done: p.done, total: p.total });
  void api.shareSdkSync().then((s) => setShareSdkReady(s.status === "ready"), () => {});
  await Promise.all([api.soundSync(progress).catch(() => {}), api.denoiseSync().catch(() => {})]);
  setSounds("stage", "done");
}

export const soundsPending = () => sounds.stage === "checking" || sounds.stage === "downloading";

export function bootLabel(): string {
  if (sounds.stage === "checking") return "正在检查资源";
  if (sounds.stage === "downloading") return `正在下载资源 ${sounds.done}/${sounds.total}`;
  return "加载中...";
}
