import { Show, onCleanup, onMount } from "solid-js";
import { Fullscreen, Maximize, Minimize, PictureInPicture2, Shrink, X } from "lucide-solid";
import { attachCanvas, setWatchMode, stopWatch, watch } from "../../stores/watch";
import { voice } from "../../stores/voice";

// 观看中的屏幕分享画面。canvas 由解码器绘制，控制台浮在画面上，悬停或键盘聚焦时显示。
// 舞台内、软件内全屏与全屏共用本组件，只改变外层布局；悬浮窗由独立窗口显示，见 PipView。
export default function ScreenView() {
  let canvas!: HTMLCanvasElement;
  onMount(() => attachCanvas(canvas));
  onCleanup(() => attachCanvas(null));
  const name = () => {
    const p = voice.peers.find((x) => x.id === watch.userId);
    return p?.name || "未知用户";
  };
  const expanded = () => watch.mode === "app" || watch.mode === "screen";
  // Esc 退出软件内全屏或全屏，回到舞台。
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && expanded()) {
      e.preventDefault();
      void setWatchMode("stage");
    }
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  return (
    <div class={`screen-view ${expanded() ? "expanded" : ""}`} aria-label={`${name()} 的屏幕共享`}>
      <canvas ref={canvas} class="screen-canvas" />
      <Show when={watch.phase !== "playing"}>
        <div class="screen-wait" role="status" aria-label="正在连接" />
      </Show>
      <div class="screen-bar">
        <span class="screen-name">{name()}</span>
        <span class="stage-gap" />
        <button class="screen-btn" data-tip="悬浮窗" aria-label="悬浮窗" onClick={() => void setWatchMode("pip")}>
          <PictureInPicture2 size={18} />
        </button>
        <Show when={watch.mode !== "screen"} fallback={
          <button class="screen-btn" data-tip="退出全屏" aria-label="退出全屏" onClick={() => void setWatchMode("stage")}>
            <Minimize size={18} />
          </button>
        }>
          <Show when={watch.mode === "app"} fallback={
            <button class="screen-btn" data-tip="软件内全屏" aria-label="软件内全屏" onClick={() => void setWatchMode("app")}>
              <Maximize size={18} />
            </button>
          }>
            <button class="screen-btn" data-tip="退出软件内全屏" aria-label="退出软件内全屏" onClick={() => void setWatchMode("stage")}>
              <Shrink size={18} />
            </button>
          </Show>
          <button class="screen-btn" data-tip="全屏" aria-label="全屏" onClick={() => void setWatchMode("screen")}>
            <Fullscreen size={18} />
          </button>
        </Show>
        <button class="screen-btn" data-tip="停止观看" aria-label="停止观看" onClick={() => void stopWatch()}>
          <X size={18} />
        </button>
      </div>
    </div>
  );
}
