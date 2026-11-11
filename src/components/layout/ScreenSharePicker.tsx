import { For, Show, createResource, createSignal, onCleanup } from "solid-js";
import { Check, Monitor, X } from "lucide-solid";
import type { ShareSource } from "../../lib/api";
import { api } from "../../lib/api";
import { FRAME_RATES, RESOLUTIONS, loadSources, share, sharing, startShare, updateShare } from "../../stores/share";

type Tab = "screen" | "window";

const [open, setOpen] = createSignal(false);
export const openSharePicker = () => setOpen(true);

// 缩略图按需取，同一次打开内缓存。源句柄只在本次运行中有效，关闭对话框即丢弃。
function Thumb(props: { source: ShareSource }) {
  const [src] = createResource(() => api.shareThumbnail(props.source.kind, props.source.id).catch(() => null));
  return (
    <span class="share-thumb">
      <Show when={src()} fallback={<Monitor size={28} />}>
        {(url) => <img src={url()} alt="" draggable={false} />}
      </Show>
    </span>
  );
}

const label = (s: ShareSource) => (s.kind === "screen" ? screenName(s) : s.name || s.application);
// 显示器名为 \\.\DISPLAY1 这类系统名，转为「屏幕 1」。主屏幕另在可访问名称中标明。
function screenName(s: ShareSource) {
  return `屏幕 ${s.name.replace(/^\\\\\.\\DISPLAY/, "")}`;
}

// 屏幕共享对话框。未分享时选源、声音与档位；分享中只改档。
export default function ScreenSharePicker() {
  const Dialog = () => {
    const live = sharing();
    // 对话框打开期间保留预览引擎，列源与缩略图复用。开播时后端自动释放，关闭时释放。
    const [sources] = createResource(() =>
      live ? Promise.resolve([] as ShareSource[]) : api.sharePreviewOpen().catch(() => {}).then(loadSources),
    );
    if (!live) onCleanup(() => void api.sharePreviewClose().catch(() => {}));
    const [tab, setTab] = createSignal<Tab>("screen");
    const [picked, setPicked] = createSignal<Record<Tab, string | null>>({ screen: null, window: null });
    const [audio, setAudio] = createSignal<Record<Tab, boolean>>({ screen: false, window: false });
    const [step, setStep] = createSignal<"source" | "quality">(live ? "quality" : "source");
    const [res, setRes] = createSignal(share.resolution);
    const [fps, setFps] = createSignal(share.frameRate);
    const list = () => (sources() ?? []).filter((s) => s.kind === tab());
    // 默认选中主屏幕。
    const chosen = (t: Tab) => {
      const id = picked()[t];
      const all = (sources() ?? []).filter((s) => s.kind === t);
      return all.find((s) => s.id === id) ?? (t === "screen" ? all[0] : undefined);
    };
    const options = (name: string, opts: { v: number; name: string }[], get: () => number, set: (v: number) => void) => (
      <div class="share-field">
        <div class="share-label">{name}</div>
        <div class="share-opts" role="radiogroup" aria-label={name}>
          <For each={opts}>
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
    const confirm = () => {
      if (live) {
        void updateShare(res(), fps());
      } else {
        const s = chosen(tab());
        if (s) void startShare(s, res(), fps(), audio()[tab()]);
      }
      close();
    };
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
    const tabBtn = (t: Tab, name: string) => (
      <button class="share-tab" role="tab" aria-selected={tab() === t} onClick={() => setTab(t)}>{name}</button>
    );
    return (
      <div class="dialog-mask" onClick={(e) => e.target === e.currentTarget && close()}>
        <div class="dialog share-dialog" ref={box} role="dialog" aria-modal="true" aria-labelledby="share-title" onKeyDown={trap}>
          <div class="share-head">
            <h3 id="share-title">{live ? "共享画质" : "屏幕共享"}</h3>
            <button class="icon-btn" data-tip="关闭" aria-label="关闭" onClick={close}><X size={20} /></button>
          </div>
          <Show when={step() === "source"} fallback={
            <div class="share-quality">
              {options("分辨率", RESOLUTIONS, res, setRes)}
              {options("帧率", FRAME_RATES, fps, setFps)}
            </div>
          }>
            <div class="share-tabs" role="tablist">
              {tabBtn("screen", "屏幕")}
              {tabBtn("window", "窗口")}
            </div>
            <Show when={!sources.loading} fallback={<div class="share-empty">正在读取</div>}>
              <Show when={!sources.error} fallback={<div class="share-empty">无法读取可共享的内容</div>}>
                <Show when={list().length} fallback={<div class="share-empty">{tab() === "screen" ? "没有可共享的屏幕" : "没有可共享的窗口"}</div>}>
                  <div class="share-grid" role="listbox" aria-label={tab() === "screen" ? "屏幕" : "窗口"}>
                    <For each={list()}>
                      {(s) => (
                        <button class="share-item" role="option" aria-selected={chosen(tab())?.id === s.id}
                          aria-description={s.kind === "screen" && s.primary ? "主屏幕" : undefined}
                          data-tip={s.kind === "window" && s.application ? s.application : undefined}
                          onClick={() => setPicked((p) => ({ ...p, [tab()]: s.id }))}>
                          <Thumb source={s} />
                          <span class="share-name">{label(s)}</span>
                        </button>
                      )}
                    </For>
                  </div>
                </Show>
              </Show>
            </Show>
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
                  <button class="dialog-cancel" onClick={() => (live ? close() : setStep("source"))}>{live ? "取消" : "上一步"}</button>
                  <button class="dialog-primary" disabled={live && share.updating} onClick={confirm}>{live ? "应用" : "开始共享"}</button>
                </>
              }>
                <button class="dialog-cancel" onClick={close}>取消</button>
                <button class="dialog-primary" disabled={!chosen(tab())} onClick={() => setStep("quality")}>下一步</button>
              </Show>
            </div>
          </div>
        </div>
      </div>
    );
  };
  return <Show when={open()}><Dialog /></Show>;
}
