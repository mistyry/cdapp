import { Show, onCleanup, onMount } from "solid-js";
import { Fullscreen, Maximize, Minimize, PictureInPicture2, Shrink, Volume2, X } from "lucide-solid";
import { attachCanvas, setShareVolume, setWatchMode, stopWatch, toggleShareMute, watch, watchingSelf } from "../../stores/watch";
import { shareVolumeOf, voice } from "../../stores/voice";
import { SpeakerOffMark } from "../common/icons";
import { HoverVolume } from "./AudioPanel";

export function SharePaused(p: { name: string }) {
  return (
    <div class="screen-paused" role="status" aria-label="屏幕分享已暂停">
      <div class="screen-paused-title">屏幕分享已暂停</div>
      <div class="screen-paused-sub">{p.name} 将自己的程序窗口最小化了</div>
    </div>
  );
}

export default function ScreenView() {
  let canvas!: HTMLCanvasElement;
  onMount(() => attachCanvas(canvas));
  onCleanup(() => attachCanvas(null));
  const name = () => {
    if (watchingSelf()) return voice.me?.name || "我";
    const p = voice.peers.find((x) => x.id === watch.userId);
    return p?.name || "未知用户";
  };
  const expanded = () => watch.mode === "app" || watch.mode === "screen";
  const muted = () => shareVolumeOf(watch.userId) === 0;
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
      <Show when={watch.paused} fallback={
        <Show when={watch.phase !== "playing"}>
          <div class="screen-wait" role="status" aria-label="正在连接" />
        </Show>
      }>
        <SharePaused name={name()} />
      </Show>
      <div class="screen-bar">
        <span class="screen-name">{name()}</span>
        <span class="stage-gap" />
        <Show when={!watchingSelf()}>
          <HoverVolume label="屏幕共享音量" value={() => shareVolumeOf(watch.userId)} set={(v) => void setShareVolume(watch.userId, v)}>
            <button class="screen-btn" aria-pressed={muted()} data-tip={muted() ? "取消静音屏幕共享" : "静音屏幕共享"} aria-label={muted() ? "取消静音屏幕共享" : "静音屏幕共享"} onClick={() => void toggleShareMute(watch.userId)}>
              {muted() ? <SpeakerOffMark size={18} /> : <Volume2 size={18} />}
            </button>
          </HoverVolume>
        </Show>
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
