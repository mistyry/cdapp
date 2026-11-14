import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Undo2 from "lucide-solid/icons/undo-2";
import { api, type WatchEvent } from "../../lib/api";
import { ScreenDecoder } from "../../lib/screenDecoder";
import { notifyMain } from "../../lib/pip";
import { TipLayer } from "../common/Tip";

export default function PipView() {
  let canvas!: HTMLCanvasElement;
  const [waiting, setWaiting] = createSignal(true);
  const [paused, setPaused] = createSignal(false);
  const [id, setId] = createSignal<{ generation: number; seq: number } | null>(null);
  const win = getCurrentWindow();
  const close = () => void win.destroy().catch(() => {});
  const back = () => {
    const cur = id();
    if (cur) void notifyMain(cur.generation, cur.seq, "back");
  };

  onMount(() => {
    let dec: ScreenDecoder | null = null;
    let off: Promise<() => void> | null = null;
    onCleanup(() => {
      dec?.close();
      void off?.then((f) => f());
    });
    const start = async () => {
      await win.show();
      const cur = await api.watchPipCurrent();
      if (!cur) return close();
      setId(cur);
      const { generation } = cur;
      const d = new ScreenDecoder(
        () => void api.watchKeyFrame(generation).catch(() => {}),
        (width, height) => {
          setWaiting(false);
          void api.watchPipRatio(width, height).catch(() => {});
        },
      );
      dec = d;
      d.attach(canvas);
      const ch = new Channel<ArrayBuffer>();
      ch.onmessage = (buf) => d.push(buf);
      off = listen<{ generation: number; state: WatchEvent }>("watch://event", ({ payload }) => {
        if (payload.generation !== generation) return;
        if (payload.state.kind === "ended") close();
        else if (payload.state.kind === "paused") setPaused(payload.state.paused);
      });
      await off;
      await notifyMain(generation, cur.seq, "ready");
      await api.watchRedirect(generation, ch);
    };
    start().catch(close);
  });

  return (
    <div class={`pip ${waiting() ? "loading" : ""}`} data-tauri-drag-region="deep">
      <TipLayer />
      <canvas ref={canvas} class="screen-canvas" />
      <Show when={paused()} fallback={
        <Show when={waiting()}>
          <div class="screen-wait" role="status" aria-label="正在连接" />
        </Show>
      }>
        <div class="screen-paused" role="status" aria-label="屏幕共享已暂停">
          <div class="screen-paused-title">屏幕共享已暂停</div>
        </div>
      </Show>
      <div class="pip-bar">
        <button class="screen-btn" aria-label="返回" data-tip="返回" onClick={back}>
          <Undo2 size={16} />
        </button>
      </div>
    </div>
  );
}
