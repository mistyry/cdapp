import { Show, createSignal, onCleanup } from "solid-js";
import { cancelPassword, enterVoice, pwPrompt } from "../../stores/session";

// 语音频道密码。密码只用于本次加入请求，关闭后清空，不保存。
export default function PasswordPrompt() {
  const [value, setValue] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const close = () => {
    setValue("");
    cancelPassword();
  };
  const submit = async () => {
    const pw = value();
    if (!pw || busy()) return;
    setBusy(true);
    setValue("");
    try {
      await enterVoice(pwPrompt.channelId, pw);
    } finally {
      setBusy(false);
    }
  };
  // 打开时记录触发位置，关闭后把焦点还回去。
  const Dialog = () => {
    const back = document.activeElement as HTMLElement | null;
    onCleanup(() => back?.isConnected && back.focus());
    let box!: HTMLDivElement;
    // Tab 在对话框内循环。
    const trap = (e: KeyboardEvent) => {
      if (e.key === "Escape") return close();
      if (e.key !== "Tab") return;
      const items = [...box.querySelectorAll<HTMLElement>("input, button:not(:disabled)")];
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) (e.preventDefault(), last?.focus());
      else if (!e.shiftKey && document.activeElement === last) (e.preventDefault(), first?.focus());
    };
    return body(trap, (el) => (box = el));
  };
  const body = (trap: (e: KeyboardEvent) => void, ref: (el: HTMLDivElement) => void) => (
    <div class="dialog-mask" onClick={(e) => e.target === e.currentTarget && close()}>
      <div class="dialog" ref={ref} role="dialog" aria-modal="true" aria-labelledby="pw-title" onKeyDown={trap}>
        <h3 id="pw-title">输入频道密码</h3>
        <input class="dialog-input" type="password" autocomplete="off" aria-label="频道密码" value={value()}
          ref={(el) => queueMicrotask(() => el.focus())}
          onInput={(e) => setValue(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void submit()} />
        <Show when={pwPrompt.error}>{(m) => <p class="dialog-error" role="alert">{m()}</p>}</Show>
        <div class="dialog-actions">
          <button class="dialog-cancel" onClick={close}>取消</button>
          <button class="dialog-primary" disabled={!value() || busy()} onClick={() => void submit()}>加入</button>
        </div>
      </div>
    </div>
  );
  return <Show when={pwPrompt.channelId}><Dialog /></Show>;
}
