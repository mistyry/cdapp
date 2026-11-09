import { Show, createSignal, onMount } from "solid-js";
import { clearCache, loadStorage, relocate, relocateDefault, storage } from "../../stores/storage";
import { Note, Row, Section } from "./parts";

const size = (n: number) => {
  if (n < 1024 * 1024) return `${Math.ceil(n / 1024)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
};

// 数据存储：数据位置、迁移与缓存。登录凭据保存在系统应用目录，不随数据迁移。
export default function StorageSettings() {
  const [confirm, setConfirm] = createSignal<null | "move" | "reset" | "clear">(null);
  onMount(() => void loadStorage());
  const go = async () => {
    const kind = confirm();
    setConfirm(null);
    if (kind === "move") await relocate();
    else if (kind === "reset") await relocateDefault();
    else if (kind === "clear") await clearCache();
  };
  const titles = { move: "更改数据位置", reset: "恢复默认位置", clear: "清除缓存" };
  const bodies = { move: "迁移完成后应用将重启。", reset: "迁移完成后应用将重启。", clear: "将删除全部缓存内容。" };
  return (
    <>
      <Section
        title="数据位置"
        notes={
          <>
            <Show when={storage.busy}><Note>正在迁移数据。</Note></Show>
            <Show when={storage.error}>{(m) => <Note warn>{m()}</Note>}</Show>
          </>
        }
      >
        <div class="st-path" data-tip={storage.info?.root}>{storage.info?.root}</div>
        <div class="st-actions">
          <button class="st-btn" disabled={storage.busy} onClick={() => setConfirm("move")}>更改位置</button>
          <Show when={storage.info?.custom}>
            <button class="st-btn" disabled={storage.busy} onClick={() => setConfirm("reset")}>恢复默认位置</button>
          </Show>
        </div>
      </Section>
      <Section title="缓存">
        <Row label="已用空间" desc={size(storage.info?.cacheBytes ?? 0)}>
          <button class="st-btn" disabled={storage.busy || !storage.info?.cacheBytes} onClick={() => setConfirm("clear")}>清除缓存</button>
        </Row>
      </Section>
      <Show when={confirm()}>
        {(k) => (
          <div class="dialog-mask" onClick={(e) => e.target === e.currentTarget && setConfirm(null)} onKeyDown={(e) => e.key === "Escape" && setConfirm(null)}>
            <div class="dialog" role="alertdialog" aria-labelledby="storage-title">
              <h3 id="storage-title">{titles[k()]}</h3>
              <p>{bodies[k()]}</p>
              <div class="dialog-actions">
                <button class="dialog-cancel" onClick={() => setConfirm(null)}>取消</button>
                <button class="dialog-danger" ref={(el) => queueMicrotask(() => el.focus())} onClick={() => void go()}>继续</button>
              </div>
            </div>
          </div>
        )}
      </Show>
    </>
  );
}
