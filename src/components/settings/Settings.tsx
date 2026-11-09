import { For, Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { HardDrive, LogOut, Mic, User, X } from "lucide-solid";
import { logout, session } from "../../stores/session";
import { setUi, ui } from "../../stores/ui";
import { DENOISE_MODES, applyDenoise, mic, denoise, denoiseMode, setDenoise, deviceOptions, loadInputs, inputs, selectInput, selectionMissing, loadOutputs, outputs, selectOutput, output, setMasterVolume, setMicGain, micTest, stopMicTest, toggleMicTest, voiceActive } from "../../stores/voice";
import Avatar from "../common/Avatar";
import { effects, setEffectVolume } from "../../stores/effects";
import StorageSettings from "./StorageSettings";
import { Note, Row, Section, Select, Slider } from "./parts";
import type { DenoiseMode } from "../../lib/api";

type Page = "account" | "voice" | "storage";
const PAGES: { id: Page; name: string; group: string; icon: () => JSX.Element }[] = [
  { id: "account", name: "账号设置", group: "个人设置", icon: () => <User size={18} /> },
  { id: "voice", name: "语音设置", group: "个人设置", icon: () => <Mic size={18} /> },
  { id: "storage", name: "数据存储", group: "应用设置", icon: () => <HardDrive size={18} /> },
];

// 全屏设置页。只保留本客户端实际支持的项目，不展示商城、背包等运营入口。
export default function Settings() {
  const [confirming, setConfirming] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const close = () => setUi("settings", null);
  const title = () => PAGES.find((x) => x.id === ui.settings)?.name ?? "";

  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || e.defaultPrevented) return;
    if (document.querySelector(".dialog-mask")) {
      if (confirming() && !busy()) setConfirming(false);
      return;
    }
    close();
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));

  // 导航按方向键在分区间移动
  const navKey = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const i = PAGES.findIndex((x) => x.id === ui.settings);
    const next = PAGES[(i + (e.key === "ArrowDown" ? 1 : PAGES.length - 1)) % PAGES.length];
    setUi("settings", next.id);
    document.getElementById(`st-tab-${next.id}`)?.focus();
  };

  const doLogout = async () => {
    setBusy(true);
    await logout();
  };

  return (
    <div class="settings">
      <nav class="st-nav" aria-label="设置">
        <div class="st-nav-inner" role="tablist" aria-orientation="vertical" onKeyDown={navKey}>
          <For each={PAGES}>
            {(x, i) => (
              <>
                <Show when={i() === 0 || PAGES[i() - 1].group !== x.group}><div class="st-nav-group">{x.group}</div></Show>
                <button
                  id={`st-tab-${x.id}`}
                  role="tab"
                  class="st-nav-item"
                  classList={{ active: ui.settings === x.id }}
                  aria-selected={ui.settings === x.id}
                  tabIndex={ui.settings === x.id ? 0 : -1}
                  onClick={() => setUi("settings", x.id)}
                >
                  {x.icon()}
                  <span>{x.name}</span>
                </button>
              </>
            )}
          </For>
          <div class="st-nav-line" />
          <button class="st-nav-item danger" onClick={() => setConfirming(true)}>
            <LogOut size={18} />
            <span>退出登录</span>
          </button>
        </div>
      </nav>
      <main class="st-main">
        <div class="st-page" role="tabpanel" aria-labelledby={`st-tab-${ui.settings}`}>
          <header class="st-header">
            <h1>{title()}</h1>
            <button class="st-close" data-tip="关闭" aria-label="关闭" onClick={close}>
              <X size={20} />
              <kbd>Esc</kbd>
            </button>
          </header>
          <Show when={ui.settings === "account"}><AccountSettings onLogout={() => setConfirming(true)} /></Show>
          <Show when={ui.settings === "voice"}><VoiceSettings /></Show>
          <Show when={ui.settings === "storage"}><StorageSettings /></Show>
        </div>
      </main>

      <Show when={confirming()}>
        <div class="dialog-mask" onClick={(e) => e.target === e.currentTarget && !busy() && setConfirming(false)}>
          <div class="dialog" role="alertdialog" aria-labelledby="logout-title">
            <h3 id="logout-title">退出登录</h3>
            <p>你确认要退出登录吗？</p>
            <div class="dialog-actions">
              <button class="dialog-cancel" disabled={busy()} onClick={() => setConfirming(false)}>取消</button>
              <button class="dialog-danger" disabled={busy()} ref={(el) => queueMicrotask(() => el.focus())} onClick={doLogout}>退出登录</button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}

function AccountSettings(p: { onLogout: () => void }) {
  return (
    <Show when={session.me}>
      {(me) => (
        <>
          <div class="st-profile">
            <Avatar user={me()} size={64} />
            <div class="st-profile-text">
              <div class="st-profile-name">
                {me().name}
                <Show when={me().tag}><span class="st-profile-tag">#{me().tag}</span></Show>
              </div>
              <div class="st-profile-id">ID {me().id}</div>
            </div>
            <button class="st-btn" onClick={p.onLogout}>退出登录</button>
          </div>
        </>
      )}
    </Show>
  );
}

// 语音设置：与语音栏快捷面板共用状态，保存在本机。
function VoiceSettings() {
  onMount(() => {
    void loadInputs();
    void loadOutputs();
  });
  onCleanup(() => void stopMicTest());
  return (
    <>
      <Section
        title="设备和音量"
        notes={
          <>
            <Show when={selectionMissing(inputs)}><Note warn>所选麦克风不可用。</Note></Show>
            <Show when={inputs.error}>{(m) => <Note warn>{m()}</Note>}</Show>
            <Show when={!inputs.error && !inputs.loading && inputs.devices.length === 0}><Note warn>没有找到麦克风。</Note></Show>
            <Show when={selectionMissing(outputs)}><Note warn>所选扬声器不可用。</Note></Show>
            <Show when={outputs.error}>{(m) => <Note warn>{m()}</Note>}</Show>
            <Show when={!outputs.error && !outputs.loading && outputs.devices.length === 0}><Note warn>没有找到扬声器。</Note></Show>
            <Show when={micTest.error}>{(m) => <Note warn>{m()}</Note>}</Show>
            <Show when={mic.message}>{(m) => <Note warn>{m()}</Note>}</Show>
          </>
        }
      >
        <Row label="输入设备" for="st-input">
          <Select id="st-input" label="输入设备" disabled={inputs.loading} value={inputs.selected ?? ""} onChange={(v) => void selectInput(v || null)}>
            <option value="">系统默认</option>
            <For each={deviceOptions(inputs)}>{(d) => <option value={d.id} title={d.title}>{d.label}</option>}</For>
            <Show when={selectionMissing(inputs)}><option value={inputs.selected!}>所选设备不可用</option></Show>
          </Select>
        </Row>
        <Row label="输出设备" for="st-output">
          <Select id="st-output" label="输出设备" disabled={outputs.loading} value={outputs.selected ?? ""} onChange={(v) => void selectOutput(v || null)}>
            <option value="">系统默认</option>
            <For each={deviceOptions(outputs)}>{(d) => <option value={d.id} title={d.title}>{d.label}</option>}</For>
            <Show when={selectionMissing(outputs)}><option value={outputs.selected!}>所选设备不可用</option></Show>
          </Select>
        </Row>
        <div class="st-divider" />
        <Row label="麦克风输入音量">
          <Slider label="麦克风输入音量" max={200} value={output.gain} onInput={(v) => void setMicGain(v)} />
        </Row>
        <Row label="语音输出音量">
          <Slider label="语音输出音量" max={200} value={output.master} onInput={(v) => void setMasterVolume(v)} />
        </Row>
        <div class="st-mictest">
          <button
            class="st-btn"
            classList={{ active: micTest.on }}
            disabled={voiceActive()}
            aria-pressed={micTest.on}
            data-tip={voiceActive() ? "语音通话中无法测试" : undefined}
            onClick={() => void toggleMicTest()}
          >
            {micTest.on ? "停止检查" : "检查一下"}
          </button>
          <div class="st-meter" role="meter" aria-label="麦克风电平" aria-valuemin={0} aria-valuemax={100} aria-valuenow={micTest.level}>
            <div class="st-meter-fill" style={{ "clip-path": `inset(0 ${100 - micTest.level}% 0 0)` }} />
          </div>
        </div>
      </Section>

      <Section title="音频设置">
        <Row label="降噪" for="st-denoise">
          <Select
            id="st-denoise"
            label="降噪"
            value={denoise() ? denoiseMode() : "off"}
            onChange={(v) => void (v === "off" ? setDenoise(false) : applyDenoise(true, v as DenoiseMode))}
          >
            <option value="off">关闭</option>
            <For each={DENOISE_MODES}>{(m) => <option value={m.id}>{m.name}</option>}</For>
          </Select>
        </Row>
        <Row label="音效音量">
          <Slider label="音效音量" max={100} value={effects.volume} onInput={(v) => void setEffectVolume(v)} />
        </Row>
      </Section>
    </>
  );
}
