import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { setUi } from "../../stores/ui";
import {
  clampVolume, defaultLabel, deviceOptions, inputs, loadInputs, loadOutputs, output, outputs,
  selectInput, selectOutput, selectionMissing, setMasterVolume, setMicGain,
} from "../../stores/voice";

export type AudioKind = "input" | "output";

const volumeOf = (k: AudioKind) => (k === "input" ? output.gain : output.master);
const setVolumeOf = (k: AudioKind, v: number) => void (k === "input" ? setMicGain(v) : setMasterVolume(v));

// 悬停在麦克风或耳机按钮上时显示的竖向音量条。拖动、点击轨道或方向键调节，双击恢复 100%。
// 百分比只在悬停手柄或拖动时以提示显示。
// 只改变音量，不改变静音与耳机状态。
export function VolumePop(p: { kind: AudioKind; onDrag: (dragging: boolean) => void }) {
  const [drag, setDrag] = createSignal(false);
  const [onKnob, setOnKnob] = createSignal(false);
  let track!: HTMLDivElement;
  const label = p.kind === "input" ? "麦克风音量" : "扬声器音量";
  const pct = () => volumeOf(p.kind) / 2;
  const at = (y: number) => {
    const r = track.getBoundingClientRect();
    return clampVolume(Math.round(((r.bottom - y) / r.height) * 40) * 5);
  };
  const down = (e: PointerEvent) => {
    e.preventDefault();
    track.setPointerCapture(e.pointerId);
    setDrag(true);
    p.onDrag(true);
    setVolumeOf(p.kind, at(e.clientY));
  };
  const move = (e: PointerEvent) => {
    if (!drag()) return;
    const v = at(e.clientY);
    if (v !== volumeOf(p.kind)) setVolumeOf(p.kind, v);
  };
  const up = () => {
    setDrag(false);
    p.onDrag(false);
  };
  const key = (e: KeyboardEvent) => {
    const step = { ArrowUp: 5, ArrowRight: 5, ArrowDown: -5, ArrowLeft: -5, PageUp: 25, PageDown: -25 }[e.key];
    let v: number | undefined;
    if (step) v = volumeOf(p.kind) + step;
    else if (e.key === "Home") v = 0;
    else if (e.key === "End") v = 200;
    if (v === undefined) return;
    e.preventDefault();
    setVolumeOf(p.kind, clampVolume(v));
  };
  return (
    <div class="vol-pop">
      <div
        ref={track}
        class="vol-track"
        role="slider"
        tabIndex={0}
        aria-label={label}
        aria-orientation="vertical"
        aria-valuemin={0}
        aria-valuemax={200}
        aria-valuenow={volumeOf(p.kind)}
        aria-valuetext={`${volumeOf(p.kind)}%`}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        onDblClick={() => setVolumeOf(p.kind, 100)}
        onKeyDown={key}
      >
        <div class="vol-fill" style={{ height: `${pct()}%` }} />
        <div class="vol-mid" />
        <div class="vol-knob" style={{ bottom: `${pct()}%` }} onPointerEnter={() => setOnKnob(true)} onPointerLeave={() => setOnKnob(false)} />
        <Show when={onKnob() || drag()}>
          <div class="vol-tip" style={{ bottom: `${pct()}%` }}>{volumeOf(p.kind)}%</div>
        </Show>
      </div>
    </div>
  );
}

// 右键麦克风或耳机按钮打开的设备菜单。只读取设备列表，不打开设备，不改变静音状态。
export function DeviceMenu(p: { kind: AudioKind; x: number; onClose: (refocus: boolean) => void }) {
  const isIn = p.kind === "input";
  const list = () => (isIn ? inputs : outputs);
  const select = (id: string | null) => void (isIn ? selectInput(id) : selectOutput(id));
  let root!: HTMLDivElement;

  onMount(() => {
    void (isIn ? loadInputs() : loadOutputs());
    (root.querySelector<HTMLElement>("[aria-checked=true]") ?? root.querySelector<HTMLElement>("button"))?.focus();
  });

  const onDown = (e: PointerEvent) => {
    if (!root.contains(e.target as Node)) p.onClose(false);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    p.onClose(true);
  };
  const onFocusOut = (e: FocusEvent) => {
    const t = e.relatedTarget as Node | null;
    if (t && !root.contains(t)) p.onClose(false);
  };
  document.addEventListener("pointerdown", onDown, true);
  document.addEventListener("keydown", onKey, true);
  onCleanup(() => {
    document.removeEventListener("pointerdown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
  });

  const onListKey = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...root.querySelectorAll<HTMLElement>(".audio-item")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
    e.preventDefault();
  };

  const title = isIn ? "输入设备" : "输出设备";
  const item = (id: string | null, label: string, tip: string) => (
    <button class="audio-item" role="menuitemradio" aria-checked={list().selected === id} data-tip={tip} aria-label={tip} onClick={() => select(id)}>
      <span class="audio-name">{label}</span>
      <span class="audio-radio" />
    </button>
  );

  return (
    <div class="audio-panel" ref={root} role="menu" aria-label={title} style={{ left: `${p.x}px` }} onFocusOut={onFocusOut} onKeyDown={onListKey}>
      <div class="audio-head">{title}</div>
      <div class="audio-list">
        {item(null, "系统默认", `${defaultLabel(list())}，跟随系统设置变化`)}
        <For each={deviceOptions(list())}>{(d) => item(d.id, d.label, d.title)}</For>
      </div>
      <Show when={selectionMissing(list())}>
        <div class="audio-note" role="status">{isIn ? "所选麦克风" : "所选扬声器"}当前不可用，连接时使用系统默认设备</div>
      </Show>
      <Show when={list().loading && !list().loaded}><div class="audio-note">正在读取设备</div></Show>
      <Show when={list().error}>
        {(m) => (
          <div class="audio-note warn">
            {m()} <button class="link-btn" onClick={() => void (isIn ? loadInputs() : loadOutputs())}>重试</button>
          </div>
        )}
      </Show>
      <Show when={list().loaded && !list().error && list().devices.length === 0}>
        <div class="audio-note">{isIn ? "没有找到麦克风" : "没有找到扬声器"}</div>
      </Show>
      <div class="audio-line" />
      <button class="audio-item audio-link" role="menuitem" onClick={() => { p.onClose(false); setUi("settings", "voice"); }}>语音设置</button>
    </div>
  );
}
