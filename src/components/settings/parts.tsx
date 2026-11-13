import { type JSX, Show, children, createEffect } from "solid-js";
import { ChevronDown } from "lucide-solid";
import RangeSlider from "../common/RangeSlider";

export function Section(p: { title: string; children: JSX.Element; notes?: JSX.Element }) {
  return (
    <section class="st-section">
      <h2 class="st-section-title">{p.title}</h2>
      <div class="st-card">{p.children}</div>
      <Show when={p.notes}><div class="st-notes">{p.notes}</div></Show>
    </section>
  );
}

export function Row(p: { label: string; desc?: string; children?: JSX.Element; for?: string }) {
  return (
    <div class="st-row">
      <div class="st-row-text">
        <label class="st-row-label" for={p.for}>{p.label}</label>
        <Show when={p.desc}><div class="st-row-desc">{p.desc}</div></Show>
      </div>
      <div class="st-row-ctl">{p.children}</div>
    </div>
  );
}

export function Slider(p: { value: number; max: number; label: string; onInput: (v: number) => void }) {
  return <RangeSlider class="st-slider" value={p.value} max={p.max} label={p.label} onInput={p.onInput} />;
}

export function Select(p: { id?: string; value: string; disabled?: boolean; label: string; onChange: (v: string) => void; children: JSX.Element }) {
  let el!: HTMLSelectElement;
  const opts = children(() => p.children);
  createEffect(() => {
    opts();
    const v = p.value;
    queueMicrotask(() => { if (el.value !== v) el.value = v; });
  });
  return (
    <div class="st-select">
      <select ref={el} id={p.id} value={p.value} disabled={p.disabled} aria-label={p.label} onChange={(e) => p.onChange(e.currentTarget.value)}>
        {opts()}
      </select>
      <ChevronDown size={16} class="st-select-arrow" />
    </div>
  );
}

export function Switch(p: { id?: string; checked: boolean; label: string; onChange: (v: boolean) => void }) {
  return (
    <button id={p.id} class="st-switch" classList={{ on: p.checked }} role="switch" aria-checked={p.checked} aria-label={p.label} onClick={() => p.onChange(!p.checked)}>
      <span class="st-switch-knob" />
    </button>
  );
}

export function Note(p: { children: JSX.Element; warn?: boolean }) {
  return <p class={`st-note ${p.warn ? "warn" : ""}`}>{p.children}</p>;
}
