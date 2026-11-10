import { Match, Show, Switch, createEffect, on, onCleanup, onMount } from "solid-js";
import { ui } from "./stores/ui";
import { boot, diag, readCurrent, refreshAccess, session, setSession, skipRestore } from "./stores/session";
import GuildBar from "./components/layout/GuildBar";
import ChannelSidebar from "./components/layout/ChannelSidebar";
import ChatPane from "./components/layout/ChatPane";
import MemberPane from "./components/layout/MemberPane";
import Login from "./components/auth/Login";
import Settings from "./components/settings/Settings";
import PasswordPrompt from "./components/layout/PasswordPrompt";
import ScreenSharePicker from "./components/layout/ScreenSharePicker";
import ConnectionBar from "./components/common/ConnectionBar";
import { TipLayer } from "./components/common/Tip";
import { loadAudioPrefs } from "./stores/voice";
import { loadStorage, storage } from "./stores/storage";
import StorageProblem from "./components/common/StorageProblem";
import { dragRegion, titleInset, watchWindow } from "./stores/window";
import WindowControls from "./components/common/WindowControls";
import BootLoader from "./components/common/BootLoader";
import { soundsPending, syncSounds } from "./stores/sounds";
import { isWindows } from "./lib/platform";
import "./styles/theme.css";
import "./styles/layout.css";

export default function App() {
  onMount(() => {
    void loadStorage().then((info) => {
      if (info?.status !== "ready") return;
      void loadAudioPrefs();
      void syncSounds();
      void boot();
    });
  });
  const unwatch = watchWindow();
  onCleanup(() => void unwatch.then((f) => f(), () => {}));
  // 窗口重新获得焦点或变为可见、关闭设置页时，补标当前频道已读。
  // 同时节流刷新当前服务器的权限依据，覆盖变化不保证有推送。
  const back = (e?: Event) => {
    if (e?.type === "visibilitychange") diag(`窗口${document.hidden ? "隐藏" : "可见"}`);
    readCurrent();
    if (session.phase === "ready" && ui.view && session.access[ui.view]) void refreshAccess(ui.view);
  };
  window.addEventListener("focus", back);
  document.addEventListener("visibilitychange", back);
  onCleanup(() => {
    window.removeEventListener("focus", back);
    document.removeEventListener("visibilitychange", back);
  });
  createEffect(on(() => ui.settings, (v) => !v && readCurrent(), { defer: true }));
  return (
    <div class={`app ${titleInset() ? "titled" : ""} ${session.phase === "boot" && storage.info?.status !== "ready" && storage.info ? "" : session.phase === "boot" || soundsPending() ? "booting" : ""}`}>
      <Show when={titleInset()}>
        <div class="titlebar">
          {/* 加载页与数据目录异常页不显示应用标识。 */}
          <Show when={!(storage.info && storage.info.status !== "ready") && session.phase !== "boot" && !soundsPending()}>
            <img src="/icon.svg" alt="" width={20} height={20} draggable={false} />
            <span>cdapp</span>
          </Show>
        </div>
      </Show>
      <TipLayer />
      <Switch>
        <Match when={storage.info && storage.info.status !== "ready"}><div class="boot" data-tauri-drag-region={dragRegion()}><StorageProblem /></div></Match>
        <Match when={session.phase === "boot" || soundsPending()}><div class="boot" data-tauri-drag-region={dragRegion()}><BootLoader /></div></Match>
        <Match when={session.phase === "restore_failed"}>
          <div class="boot" data-tauri-drag-region={dragRegion()}>
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
          <PasswordPrompt />
          <ScreenSharePicker />
        </Match>
      </Switch>
      <Show when={session.error}>
        <div class="toast" onClick={() => setSession("error", null)}>{session.error}</div>
      </Show>
      {/* 拖拽层覆盖标题栏，位于设置页与对话框之上，任何页面都能拖动窗口。窗口按钮在拖拽层之上。 */}
      <Show when={titleInset()}><div class="title-drag" data-tauri-drag-region={dragRegion()} /></Show>
      <Show when={isWindows && titleInset()}><WindowControls /></Show>
    </div>
  );
}
