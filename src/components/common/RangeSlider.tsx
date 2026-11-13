import { Show, createSignal } from "solid-js";
import { keyboardFocus } from "./Tip";

export default function RangeSlider(p: {
  value: number;
  min?: number;
  max: number;
  label: string;
  class?: string;
  onInput: (v: number) => void;
}) {
  const min = () => p.min ?? 0;
  const [drag, setDrag] = createSignal(false);
  const [onKnob, setOnKnob] = createSignal(false);
  const [focus, setFocus] = createSignal(false);
  let track!: HTMLDivElement;
  const clamp = (v: number) => Math.max(min(), Math.min(p.max, Math.round(v)));
  const pct = () => ((p.value - min()) / (p.max - min())) * 100;
  const set = (v: number) => {
    const c = clamp(v);
    if (c !== p.value) p.onInput(c);
  };
  const at = (x: number) => {
    const r = track.getBoundingClientRect();
    return min() + ((x - r.left) / r.width) * (p.max - min());
  };
  const down = (e: PointerEvent) => {
    e.preventDefault();
    setFocus(false);
    track.focus();
    track.setPointerCapture(e.pointerId);
    setDrag(true);
    set(at(e.clientX));
  };
  const move = (e: PointerEvent) => drag() && set(at(e.clientX));
  const up = () => setDrag(false);
  const key = (e: KeyboardEvent) => {
    const step = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1, PageUp: 10, PageDown: -10 }[e.key];
    let v: number | undefined;
    if (step) v = p.value + step;
    else if (e.key === "Home") v = min();
    else if (e.key === "End") v = p.max;
    if (v === undefined) return;
    e.preventDefault();
    setFocus(true);
    set(v);
  };
  return (
    <div
      ref={track}
      class={`range ${p.class ?? ""}`}
      classList={{ dragging: drag() }}
      role="slider"
      tabIndex={0}
      aria-label={p.label}
      aria-valuemin={min()}
      aria-valuemax={p.max}
      aria-valuenow={p.value}
      aria-valuetext={`${p.value}%`}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onKeyDown={key}
      onFocus={(e) => setFocus(keyboardFocus(e.currentTarget))}
      onBlur={() => setFocus(false)}
    >
      <div class="range-rail" />
      <div class="range-fill" style={{ width: `${pct()}%` }} />
      <div class="range-knob" style={{ left: `${pct()}%` }} onPointerEnter={() => setOnKnob(true)} onPointerLeave={() => setOnKnob(false)} />
      <Show when={onKnob() || drag() || focus()}>
        <div class="range-tip" style={{ left: `${pct()}%` }}>{p.value}%</div>
      </Show>
    </div>
  );
}
