import { For, Show, createSignal, onCleanup } from "solid-js";
import { Check, X } from "lucide-solid";

type Tab = "screen" | "window";
type Source = { id: string; name: string };

// 模拟数据，接入原生采集后由后端枚举。
const MOCK: Record<Tab, Source[]> = {
  screen: Array.from({ length: 6 }, (_, i) => ({ id: `s${i + 1}`, name: `屏幕 ${i + 1}` })),
  window: ["Visual Studio Code", "Microsoft Edge", "文件资源管理器", "Steam", "哔哩哔哩", "网易云音乐", "微信", "记事本", "Windows 终端", "Photoshop"]
    .map((name, i) => ({ id: `w${i + 1}`, name })),
};

const [open, setOpen] = createSignal(false);
export const openSharePicker = () => setOpen(true);

// 屏幕共享选源对话框。
export default function ScreenSharePicker() {
  const Dialog = () => {
    const [tab, setTab] = createSignal<Tab>("screen");
    const [picked, setPicked] = createSignal<Record<Tab, string | null>>({ screen: "s1", window: null });
    const [audio, setAudio] = createSignal<Record<Tab, boolean>>({ screen: false, window: false });
    const [step, setStep] = createSignal<"source" | "quality">("source");
    const [res, setRes] = createSignal(720);
    const [fps, setFps] = createSignal(30);
    const options = (label: string, list: { v: number; name: string }[], get: () => number, set: (v: number) => void) => (
      <div class="share-field">
        <div class="share-label">{label}</div>
        <div class="share-opts" role="radiogroup" aria-label={label}>
          <For each={list}>
            {(o) => (
              <button class="share-opt" role="radio" aria-checked={get() === o.v} onClick={() => set(o.v)}>
                <span class="audio-radio" />{o.name}
              </button>
            )}
          </For>
        </div>
      </div>
    );
    const close = () => setOpen(false);
    const back = document.activeElement as HTMLElement | null;
    onCleanup(() => back?.isConnected && back.focus());
    let box!: HTMLDivElement;
    const trap = (e: KeyboardEvent) => {
      if (e.key === "Escape") return close();
      if (e.key !== "Tab") return;
      const items = [...box.querySelectorAll<HTMLElement>("input, button:not(:disabled)")];
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) (e.preventDefault(), last?.focus());
      else if (!e.shiftKey && document.activeElement === last) (e.preventDefault(), first?.focus());
    };
    const tabBtn = (t: Tab, label: string) => (
      <button class="share-tab" role="tab" aria-selected={tab() === t} onClick={() => setTab(t)}>{label}</button>
    );
    return (
      <div class="dialog-mask" onClick={(e) => e.target === e.currentTarget && close()}>
        <div class="dialog share-dialog" ref={box} role="dialog" aria-modal="true" aria-labelledby="share-title" onKeyDown={trap}>
          <div class="share-head">
            <h3 id="share-title">屏幕共享</h3>
            <button class="icon-btn" data-tip="关闭" aria-label="关闭" onClick={close}><X size={20} /></button>
          </div>
          <Show when={step() === "source"} fallback={
            <div class="share-quality">
              {options("分辨率", [{ v: 720, name: "720p" }, { v: 1080, name: "1080p" }], res, setRes)}
              {options("帧率", [{ v: 30, name: "30 帧" }, { v: 60, name: "60 帧" }], fps, setFps)}
            </div>
          }>
          <div class="share-tabs" role="tablist">
            {tabBtn("screen", "屏幕")}
            {tabBtn("window", "窗口")}
          </div>
          <div class="share-grid" role="listbox" aria-label={tab() === "screen" ? "屏幕" : "窗口"}>
            <For each={MOCK[tab()]}>
              {(s) => (
                <button class="share-item" role="option" aria-selected={picked()[tab()] === s.id}
                  onClick={() => setPicked((p) => ({ ...p, [tab()]: s.id }))}>
                  <span class="share-thumb" />
                  <span class="share-name">{s.name}</span>
                </button>
              )}
            </For>
          </div>
          </Show>
          <div class="share-foot">
            <Show when={step() === "source"} fallback={<span />}>
            <button class="share-check" role="checkbox" aria-checked={audio()[tab()]}
              onClick={() => setAudio((a) => ({ ...a, [tab()]: !a[tab()] }))}>
              <span class="check-box" classList={{ on: audio()[tab()] }}>
                <Show when={audio()[tab()]}><Check size={12} stroke-width={3} /></Show>
              </span>
              {tab() === "screen" ? "分享系统声音" : "分享程序声音"}
            </button>
            </Show>
            <div class="dialog-actions">
              <Show when={step() === "source"} fallback={
                <>
                  <button class="dialog-cancel" onClick={() => setStep("source")}>上一步</button>
                  <button class="dialog-primary" onClick={close}>开始共享</button>
                </>
              }>
                <button class="dialog-cancel" onClick={close}>取消</button>
                <button class="dialog-primary" disabled={!picked()[tab()]} onClick={() => setStep("quality")}>下一步</button>
              </Show>
            </div>
          </div>
        </div>
      </div>
    );
  };
  return <Show when={open()}><Dialog /></Show>;
}
