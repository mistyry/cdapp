import { Match, Show, Switch, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { diag, networkBack, reconnectNow, session, setSession } from "../../stores/session";

export default function ConnectionBar() {
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 1000);
  const online = () => {
    diag("系统网络恢复");
    networkBack();
  };
  const offline = () => diag("系统网络断开", "warn");
  onMount(() => {
    window.addEventListener("online", online);
    window.addEventListener("offline", offline);
  });
  onCleanup(() => {
    clearInterval(timer);
    window.removeEventListener("online", online);
    window.removeEventListener("offline", offline);
  });

  createEffect(() => {
    const r = session.recovery;
    if (!r || r === "partial" || session.gateway !== "online") return;
    const t = setTimeout(() => setSession("recovery", null), 3000);
    onCleanup(() => clearTimeout(t));
  });
  const recovered = () => session.recovery;

  const wait = () => Math.max(0, Math.ceil(((session.gatewayInfo?.retryAt ?? 0) - now()) / 1000));

  return (
    <Switch>
      <Match when={session.gateway === "connecting"}>
        <div class="conn-bar" role="status">正在连接…</div>
      </Match>
      <Match when={session.gateway === "reconnecting"}>
        <div class="conn-bar warn" role="status" data-tip={session.gatewayInfo?.reason} aria-description={session.gatewayInfo?.reason}>
          连接已断开，{wait() > 0 ? `${wait()} 秒后重连` : "正在重连…"}
          <Show when={wait() > 0}><button onClick={reconnectNow}>立即重连</button></Show>
        </div>
      </Match>
      <Match when={session.gateway === "syncing"}>
        <div class="conn-bar" role="status">已重新连接，正在同步消息…</div>
      </Match>
      <Match when={recovered() === "partial"}>
        <div class="conn-bar warn" role="status">
          已重新连接，部分频道同步失败，进入频道时重试
          <button onClick={() => setSession("recovery", null)}>关闭</button>
        </div>
      </Match>
      <Match when={recovered()}>
        <div class="conn-bar ok" role="status">{session.recovery === "resumed" ? "已恢复连接" : "已重新连接，消息已同步"}</div>
      </Match>
    </Switch>
  );
}
