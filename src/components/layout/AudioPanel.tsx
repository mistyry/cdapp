import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { setUi } from "../../stores/ui";
import {
  DENOISE_MODES, clampVolume, defaultName, denoiseMode, deviceOptions, inputs, kookFallback, loadInputs, loadOutputs, output, outputs, setDenoiseMode,
  selectInput, selectOutput, shownSelection, setMasterVolume, setMicGain,
} from "../../stores/voice";

export type AudioKind = "input" | "output";

export const volumeOf = (k: AudioKind) => (k === "input" ? output.gain : output.master);
export const setVolumeOf = (k: AudioKind, v: number) => void (k === "input" ? setMicGain(v) : setMasterVolume(v));

export function VolumePop(p: { kind: AudioKind; onDrag: (dragging: boolean) => void }) {
  return <VolumeTrack label={p.kind === "input" ? "麦克风音量" : "扬声器音量"} value={() => volumeOf(p.kind)} set={(v) => setVolumeOf(p.kind, v)} onDrag={p.onDrag} />;
}

export function VolumeTrack(p: { label: string; value: () => number; set: (v: number) => void; onDrag: (dragging: boolean) => void; class?: string; style?: Record<string, string> }) {
  const [drag, setDrag] = createSignal(false);
  const [onKnob, setOnKnob] = createSignal(false);
  let track!: HTMLDivElement;
  const label = p.label;
  const value = p.value;
  const set = (v: number) => p.set(v);
  const pct = () => value() / 2;
  const at = (y: number) => {
    const r = track.getBoundingClientRect();
    return clampVolume(((r.bottom - y) / r.height) * 200);
  };
  const down = (e: PointerEvent) => {
    e.preventDefault();
    track.setPointerCapture(e.pointerId);
    setDrag(true);
    p.onDrag(true);
    set(at(e.clientY));
  };
  const move = (e: PointerEvent) => {
    if (!drag()) return;
    const v = at(e.clientY);
    if (v !== value()) set(v);
  };
  const up = () => {
    setDrag(false);
    p.onDrag(false);
  };
  const key = (e: KeyboardEvent) => {
    const step = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1, PageUp: 10, PageDown: -10 }[e.key];
    let v: number | undefined;
    if (step) v = value() + step;
    else if (e.key === "Home") v = 0;
    else if (e.key === "End") v = 200;
    if (v === undefined) return;
    e.preventDefault();
    set(clampVolume(v));
  };
  return (
    <div class={`vol-pop ${p.class ?? ""}`} style={p.style}>
      <div
        ref={track}
        class="vol-track"
        role="slider"
        tabIndex={0}
        aria-label={label}
        aria-orientation="vertical"
        aria-valuemin={0}
        aria-valuemax={200}
        aria-valuenow={value()}
        aria-valuetext={`${value()}%`}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        onDblClick={() => set(100)}
        onKeyDown={key}
      >
        <div class="vol-fill" style={{ height: `${pct()}%` }} />
        <div class="vol-mid" />
        <div class="vol-knob" style={{ bottom: `${pct()}%` }} onPointerEnter={() => setOnKnob(true)} onPointerLeave={() => setOnKnob(false)} />
        <Show when={onKnob() || drag()}>
          <div class="vol-tip" style={{ bottom: `${pct()}%` }}>{value()}%</div>
        </Show>
      </div>
    </div>
  );
}

export function DenoiseMenu(p: { x: number; onClose: (refocus: boolean) => void }) {
  let root!: HTMLDivElement;
  onMount(() => (root.querySelector<HTMLElement>("[aria-checked=true]") ?? root.querySelector<HTMLElement>("button"))?.focus());
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
  return (
    <div class="audio-panel" ref={root} role="menu" aria-label="降噪模型" style={{ left: `${p.x}px` }} onFocusOut={onFocusOut} onKeyDown={onListKey}>
      <div class="audio-head">降噪模型</div>
      <div class="audio-list">
        <For each={DENOISE_MODES}>
          {(m) => (
            <button class="audio-item" role="menuitemradio" aria-checked={denoiseMode() === m.id} onClick={() => void setDenoiseMode(m.id)}>
              <span class="audio-name">{m.name}</span>
              <span class="audio-radio" />
            </button>
          )}
        </For>
      </div>
      <Show when={kookFallback() && denoiseMode() === "kook"}>
        <div class="audio-note" role="status">正在使用 RNNoise</div>
      </Show>
      <div class="audio-line" />
      <button class="audio-item audio-link" role="menuitem" onClick={() => { p.onClose(false); setUi("settings", "voice"); }}>语音设置</button>
    </div>
  );
}

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
    <button class="audio-item" role="menuitemradio" aria-checked={shownSelection(list()) === id} data-tip={tip} aria-label={tip} onClick={() => select(id)}>
      <span class="audio-name">{label}</span>
      <span class="audio-radio" />
    </button>
  );

  return (
    <div class="audio-panel" ref={root} role="menu" aria-label={title} style={{ left: `${p.x}px` }} onFocusOut={onFocusOut} onKeyDown={onListKey}>
      <div class="audio-head">{title}</div>
      <div class="audio-list">
        {item(null, "系统默认", defaultName(list()))}
        <For each={deviceOptions(list())}>{(d) => item(d.id, d.label, d.title)}</For>
      </div>
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
