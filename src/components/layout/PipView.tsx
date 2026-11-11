import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Maximize2, X } from "lucide-solid";
import { api, type WatchEvent } from "../../lib/api";
import { ScreenDecoder } from "../../lib/screenDecoder";
import { notifyMain } from "../../stores/watch";
import { TipLayer } from "../common/Tip";

// 悬浮窗：独立的置顶小窗，自己接收画面并解码。整窗可拖动，悬停时显示回到舞台与停止观看按钮。
// 观看结束或会话已不是本窗口的代次时关闭自己。主窗口负责会话与状态，本窗口只显示画面。
export default function PipView(p: { generation: number }) {
  let canvas!: HTMLCanvasElement;
  const [waiting, setWaiting] = createSignal(true);
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
    void api.watchRedirect(p.generation, ch).catch(close);
    const off = listen<{ generation: number; state: WatchEvent }>("watch://event", ({ payload }) => {
      if (payload.generation === p.generation && payload.state.kind === "ended") close();
    });
    onCleanup(() => {
      dec.close();
      void off.then((f) => f());
    });
  });

  return (
    <div class="pip" data-tauri-drag-region>
      <TipLayer />
      <canvas ref={canvas} class="screen-canvas" data-tauri-drag-region />
      <Show when={waiting()}>
        <div class="screen-wait" role="status" aria-label="正在连接" />
      </Show>
      <div class="pip-bar">
        <button class="screen-btn" aria-label="回到舞台" data-tip="回到舞台" onClick={() => void notifyMain(p.generation, "back")}>
          <Maximize2 size={16} />
        </button>
        <button class="screen-btn" aria-label="停止观看" data-tip="停止观看" onClick={() => void notifyMain(p.generation, "stop")}>
          <X size={16} />
        </button>
      </div>
    </div>
  );
}
