import { Show, createSignal, onMount } from "solid-js";
import { X } from "lucide-solid";
import { clearCache, loadStorage, relocate, relocateDefault, storage } from "../../stores/storage";

const size = (n: number) => {
  if (n < 1024 * 1024) return `${Math.ceil(n / 1024)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
};

// 数据存储：数据位置、迁移与缓存。登录凭据保存在系统应用目录，不随数据迁移。
export default function StorageSettings(p: { close: () => void }) {
  const [confirm, setConfirm] = createSignal<null | "move" | "reset">(null);
  onMount(() => void loadStorage());
  const go = async () => {
    const kind = confirm();
    setConfirm(null);
    if (kind === "move") await relocate();
    else if (kind === "reset") await relocateDefault();
  };
  return (
    <div class="settings-page">
      <header class="settings-header">
        <h1>数据存储</h1>
        <button class="settings-close" data-tip="关闭" aria-label="关闭" onClick={p.close}><X size={20} /></button>
      </header>
      <h2 class="settings-panel-title">数据位置</h2>
      <section class="settings-panel">
        <div class="settings-box wide">
          <div class="settings-row">
            <span class="settings-path" data-tip={storage.info?.root}>{storage.info?.root}</span>
            <span>
              <button class="settings-btn" disabled={storage.busy} onClick={() => setConfirm("move")}>更改位置</button>
              <Show when={storage.info?.custom}>
                <button class="settings-btn" disabled={storage.busy} onClick={() => setConfirm("reset")}>恢复默认位置</button>
              </Show>
            </span>
          </div>
        </div>
        <Show when={storage.busy}><div class="settings-note">正在迁移数据。</div></Show>
        <Show when={storage.error}>{(m) => <div class="settings-note">{m()}</div>}</Show>
        <div class="settings-note">数据位置保存设置、账号状态与缓存。</div>
        <div class="settings-note">更改位置时会断开语音，复制数据后重启应用。</div>
        <div class="settings-note">新位置须为空文件夹或已有的 cdapp 数据文件夹。</div>
        <div class="settings-note">登录凭据保存在系统应用目录，不随数据迁移。</div>
      </section>
      <h2 class="settings-panel-title">缓存</h2>
      <section class="settings-panel">
        <div class="settings-box wide">
          <div class="settings-row">
            <span class="settings-label">已用空间 {size(storage.info?.cacheBytes ?? 0)}</span>
            <button class="settings-btn" disabled={storage.busy} onClick={() => void clearCache()}>清除缓存</button>
          </div>
        </div>
        <div class="settings-note">缓存超过 512 MB 时，启动时自动清理最早的内容。</div>
      </section>
      <Show when={confirm()}>
        <div class="dialog-mask" onClick={(e) => e.target === e.currentTarget && setConfirm(null)}>
          <div class="dialog" role="alertdialog" aria-labelledby="storage-title">
            <h3 id="storage-title">{confirm() === "move" ? "更改数据位置" : "恢复默认位置"}</h3>
            <p>迁移完成后应用将重启。</p>
            <div class="dialog-actions">
              <button class="dialog-cancel" onClick={() => setConfirm(null)}>取消</button>
              <button class="dialog-danger" onClick={() => void go()}>继续</button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}
