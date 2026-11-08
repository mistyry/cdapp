import { Show } from "solid-js";
import { locate, restartApp, storage, useDefault } from "../../stores/storage";

// 数据目录不可用时的启动页。不自动新建空目录，由用户决定。
export default function StorageProblem() {
  const info = () => storage.info!;
  const title = () => {
    switch (info().status) {
      case "missing": return "找不到数据文件夹";
      case "locked": return "cdapp 已在运行";
      default: return "数据文件夹无法写入";
    }
  };
  return (
    <div class="storage-problem">
      <p>{title()}</p>
      <Show when={info().status !== "locked"}><p class="storage-path">{info().root}</p></Show>
      <Show when={info().message}>{(m) => <p class="storage-path">{m()}</p>}</Show>
      <Show when={storage.error}>{(m) => <p class="storage-path">{m()}</p>}</Show>
      <div>
        <Show
          when={info().status !== "locked"}
          fallback={<button class="login-retry" onClick={restartApp}>重试</button>}
        >
          <button class="login-retry" disabled={storage.busy} onClick={() => void locate()}>重新选择位置</button>
          <button class="login-alt" disabled={storage.busy} onClick={() => void useDefault()}>使用默认位置</button>
          <button class="login-alt" disabled={storage.busy} onClick={restartApp}>重试</button>
        </Show>
      </div>
    </div>
  );
}
