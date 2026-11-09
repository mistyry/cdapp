import { createStore } from "solid-js/store";
import { Channel } from "@tauri-apps/api/core";
import { api } from "../lib/api";

// 启动时的音效缓存。checking 检查索引；downloading 下载缺少的文件；done 结束，失败不阻塞启动。
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
  // 降噪模块与音效同时缓存，失败时下次启动再下载。
  await Promise.all([api.soundSync(progress).catch(() => {}), api.denoiseSync().catch(() => {})]);
  setSounds("stage", "done");
}

export const soundsPending = () => sounds.stage === "checking" || sounds.stage === "downloading";

export function bootLabel(): string {
  if (sounds.stage === "checking") return "正在检查资源";
  if (sounds.stage === "downloading") return `正在下载资源 ${sounds.done}/${sounds.total}`;
  return "加载中...";
}
