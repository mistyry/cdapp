import { Match, Show, Switch, createEffect, on, onCleanup, onMount } from "solid-js";
import { ui } from "./stores/ui";
import { boot, readCurrent, session, setSession, skipRestore } from "./stores/session";
import GuildBar from "./components/layout/GuildBar";
import ChannelSidebar from "./components/layout/ChannelSidebar";
import ChatPane from "./components/layout/ChatPane";
import MemberPane from "./components/layout/MemberPane";
import Login from "./components/auth/Login";
import Settings from "./components/settings/Settings";
import ConnectionBar from "./components/common/ConnectionBar";
import "./styles/theme.css";
import "./styles/layout.css";

export default function App() {
  onMount(boot);
  // 窗口重新获得焦点或变为可见、关闭设置页时，补标当前频道已读。
  const back = () => readCurrent();
  window.addEventListener("focus", back);
  document.addEventListener("visibilitychange", back);
  onCleanup(() => {
    window.removeEventListener("focus", back);
    document.removeEventListener("visibilitychange", back);
  });
  createEffect(on(() => ui.settings, (v) => !v && readCurrent(), { defer: true }));
  return (
    <div class="app">
      <Switch>
        <Match when={session.phase === "boot"}><div class="boot">加载中</div></Match>
        <Match when={session.phase === "restore_failed"}>
          <div class="boot">
            <p>无法连接到 KOOK，登录信息已保留</p>
            <div>
              <button class="login-retry" onClick={boot}>重试</button>
              <button class="login-alt" onClick={skipRestore}>扫码登录</button>
            </div>
          </div>
        </Match>
        <Match when={session.phase === "login"}><Login /></Match>
        <Match when={session.phase === "ready"}>
          <GuildBar />
          <div class="shell">
            <ConnectionBar />
            <ChannelSidebar />
            <ChatPane />
            <Show when={ui.showMembers}><MemberPane /></Show>
          </div>
          <Show when={ui.settings}><Settings /></Show>
        </Match>
      </Switch>
      <Show when={session.error}>
        <div class="toast" onClick={() => setSession("error", null)}>{session.error}</div>
      </Show>
    </div>
  );
}
