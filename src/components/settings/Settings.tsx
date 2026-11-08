import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { X } from "lucide-solid";
import { logout, session } from "../../stores/session";
import { setUi, ui } from "../../stores/ui";
import { deviceOptions, loadInputs, inputs, selectInput, selectionMissing, loadOutputs, outputs, selectOutput, output, setMasterVolume, setMicGain } from "../../stores/voice";
import Avatar from "../common/Avatar";
import StorageSettings from "./StorageSettings";

// 全屏设置页，布局参照网页端“个人设置”。只保留本客户端实际支持的项目，不展示商城、背包等运营入口。
export default function Settings() {
  const [confirming, setConfirming] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const close = () => setUi("settings", null);

  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    if (confirming()) setConfirming(false);
    else close();
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));

  const doLogout = async () => {
    setBusy(true);
    await logout();
  };

  return (
    <div class="settings">
      <nav class="settings-nav">
        <div class="settings-nav-inner">
          <div class="settings-group">个人设置</div>
          <div class={`settings-item ${ui.settings === "account" ? "active" : ""}`} onClick={() => setUi("settings", "account")}>账号设置</div>
          <div class={`settings-item ${ui.settings === "voice" ? "active" : ""}`} onClick={() => setUi("settings", "voice")}>语音设置</div>
          <div class="settings-group">应用设置</div>
          <div class={`settings-item ${ui.settings === "storage" ? "active" : ""}`} onClick={() => setUi("settings", "storage")}>数据存储</div>
          <div class="settings-line" />
          <div class="settings-item danger" onClick={() => setConfirming(true)}>退出登录</div>
        </div>
      </nav>
      <div class="settings-main">
        <Show when={ui.settings === "voice"}><VoiceSettings close={close} /></Show>
        <Show when={ui.settings === "storage"}><StorageSettings close={close} /></Show>
        <div class="settings-page" hidden={ui.settings !== "account"}>
          <header class="settings-header">
            <h1>账号设置</h1>
            <button class="settings-close" data-tip="关闭" aria-label="关闭" onClick={close}><X size={20} /></button>
          </header>
          <h2 class="settings-panel-title">个人信息</h2>
          <Show when={session.me}>
            {(me) => (
              <section class="settings-panel">
                <div class="settings-sub">基本信息</div>
                <div class="settings-box">
                  <div class="settings-row">
                    <span class="settings-label">用户名</span>
                    <span class="settings-value">{me().name}{me().tag ? `#${me().tag}` : ""}</span>
                  </div>
                  <div class="settings-row">
                    <span class="settings-label">头像</span>
                    <Avatar user={me()} size={24} />
                  </div>
                </div>
              </section>
            )}
          </Show>
        </div>
      </div>

      <Show when={confirming()}>
        <div class="dialog-mask" onClick={(e) => e.target === e.currentTarget && !busy() && setConfirming(false)}>
          <div class="dialog" role="alertdialog" aria-labelledby="logout-title">
            <h3 id="logout-title">退出登录</h3>
            <p>你确认要退出登录吗？</p>
            <div class="dialog-actions">
              <button class="dialog-cancel" disabled={busy()} onClick={() => setConfirming(false)}>取消</button>
              <button class="dialog-danger" disabled={busy()} onClick={doLogout}>退出登录</button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}

// 语音设置：输入与输出设备、麦克风音量与总音量。与语音栏快捷面板共用状态，保存在本机。
function VoiceSettings(p: { close: () => void }) {
  onMount(() => {
    void loadInputs();
    void loadOutputs();
  });
  return (
    <div class="settings-page">
      <header class="settings-header">
        <h1>语音设置</h1>
        <button class="settings-close" data-tip="关闭" aria-label="关闭" onClick={p.close}><X size={20} /></button>
      </header>
      <h2 class="settings-panel-title">输入</h2>
      <section class="settings-panel">
        <div class="settings-box">
          <div class="settings-row">
            <span class="settings-label">麦克风</span>
            <select
              class="settings-select"
              disabled={inputs.loading}
              value={inputs.selected ?? ""}
              onChange={(e) => void selectInput(e.currentTarget.value || null)}
            >
              <option value="">系统默认</option>
              <For each={deviceOptions(inputs)}>{(d) => <option value={d.id} title={d.title}>{d.label}</option>}</For>
              <Show when={selectionMissing(inputs)}><option value={inputs.selected!}>所选设备不可用</option></Show>
            </select>
          </div>
          <div class="settings-row">
            <span class="settings-label">麦克风音量</span>
            <input type="range" min="0" max="200" step="5" value={output.gain} aria-label="麦克风音量" onInput={(e) => void setMicGain(Number(e.currentTarget.value))} />
            <span>{output.gain}%</span>
          </div>
        </div>
        <Show when={selectionMissing(inputs)}><div class="settings-note">所选麦克风不可用。</div></Show>
        <Show when={inputs.error}>{(m) => <div class="settings-note">{m()}</div>}</Show>
        <Show when={!inputs.error && !inputs.loading && inputs.devices.length === 0}><div class="settings-note">没有找到麦克风。</div></Show>
        <div class="settings-note">语音感应与按键说话尚未实现，目前持续开麦。</div>
        <div class="settings-note">没有回声消除，建议使用耳机。</div>
        <div class="settings-note">加入语音频道时才会打开麦克风。麦克风音量作用于发送前的声音，100% 为原始音量。</div>
      </section>
      <h2 class="settings-panel-title">输出</h2>
      <section class="settings-panel">
        <div class="settings-box">
          <div class="settings-row">
            <span class="settings-label">扬声器</span>
            <select
              class="settings-select"
              disabled={outputs.loading}
              value={outputs.selected ?? ""}
              onChange={(e) => void selectOutput(e.currentTarget.value || null)}
            >
              <option value="">系统默认</option>
              <For each={deviceOptions(outputs)}>{(d) => <option value={d.id} title={d.title}>{d.label}</option>}</For>
              <Show when={selectionMissing(outputs)}><option value={outputs.selected!}>所选设备不可用</option></Show>
            </select>
          </div>
          <div class="settings-row">
            <span class="settings-label">总音量</span>
            <input type="range" min="0" max="200" step="5" value={output.master} aria-label="总音量" onInput={(e) => void setMasterVolume(Number(e.currentTarget.value))} />
            <span>{output.master}%</span>
          </div>
        </div>
        <Show when={outputs.error}>{(m) => <div class="settings-note">{m()}</div>}</Show>
        <Show when={!outputs.error && !outputs.loading && outputs.devices.length === 0}><div class="settings-note">没有找到扬声器。</div></Show>
        <Show when={selectionMissing(outputs)}><div class="settings-note">所选扬声器不可用。</div></Show>
        <div class="settings-note">成员音量可在语音频道中点击成员调节，范围 0% 至 200%。</div>
        <div class="settings-note">未提供回声消除与降噪。外放时对方可能听到回声，建议使用耳机。</div>
      </section>
    </div>
  );
}
