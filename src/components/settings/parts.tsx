import { type JSX, Show } from "solid-js";
import { ChevronDown } from "lucide-solid";
import RangeSlider from "../common/RangeSlider";

// 设置页通用控件：分区卡片、行、开关、滑块与下拉框。

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
  return (
    <div class="st-select">
      <select id={p.id} value={p.value} disabled={p.disabled} aria-label={p.label} onChange={(e) => p.onChange(e.currentTarget.value)}>
        {p.children}
      </select>
      <ChevronDown size={16} class="st-select-arrow" />
    </div>
  );
}

export function Note(p: { children: JSX.Element; warn?: boolean }) {
  return <p class={`st-note ${p.warn ? "warn" : ""}`}>{p.children}</p>;
}
