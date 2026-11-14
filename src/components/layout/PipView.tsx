import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Maximize2, X } from "lucide-solid";
import { api, type WatchEvent } from "../../lib/api";
import { ScreenDecoder } from "../../lib/screenDecoder";
import { notifyMain } from "../../stores/watch";
import { TipLayer } from "../common/Tip";

export default function PipView(p: { generation: number }) {
  let canvas!: HTMLCanvasElement;
  const [waiting, setWaiting] = createSignal(true);
  const [paused, setPaused] = createSignal(false);
  const win = getCurrentWindow();
  const close = () => void win.destroy().catch(() => {});

  onMount(() => {
    const dec = new ScreenDecoder(
      () => void api.watchKeyFrame(p.generation).catch(() => {}),
      () => setWaiting(false),
    );
    dec.attach(canvas);
    const ch = new Channel<ArrayBuffer>();
    ch.onmessage = (buf) => dec.push(buf);
    const off = listen<{ generation: number; state: WatchEvent }>("watch://event", ({ payload }) => {
      if (payload.generation !== p.generation) return;
      if (payload.state.kind === "ended") close();
      else if (payload.state.kind === "paused") setPaused(payload.state.paused);
    });
    void off.then(() => api.watchRedirect(p.generation, ch)).catch(close);
    onCleanup(() => {
      dec.close();
      void off.then((f) => f());
    });
  });

  return (
    <div class="pip" data-tauri-drag-region>
      <TipLayer />
      <canvas ref={canvas} class="screen-canvas" data-tauri-drag-region />
      <Show when={paused()} fallback={
        <Show when={waiting()}>
          <div class="screen-wait" role="status" aria-label="正在连接" />
        </Show>
      }>
        <div class="screen-paused" role="status" aria-label="屏幕分享已暂停">
          <div class="screen-paused-title">屏幕分享已暂停</div>
        </div>
      </Show>
      <div class="pip-bar">
        <button class="screen-btn" aria-label="回到 stage" data-tip="回到 stage" onClick={() => void notifyMain(p.generation, "back")}>
          <Maximize2 size={16} />
        </button>
        <button class="screen-btn" aria-label="停止观看" data-tip="停止观看" onClick={() => void notifyMain(p.generation, "stop")}>
          <X size={16} />
        </button>
      </div>
    </div>
  );
}
