import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { renderSVG } from "uqr";
import { api, errorText } from "../../lib/api";
import { enter, session } from "../../stores/session";
import { dragRegion } from "../../stores/window";

const TEXT = {
  loading: "正在获取二维码",
  waiting: "使用 KOOK 手机端扫码登录",
  scanned: "已扫码，请在手机上确认",
  confirmed: "登录中",
  unknown: "二维码已失效",
  failed: "",
};
type Status = keyof typeof TEXT;

const POLL_MS = 1000;
const QR_TTL_MS = 300_000;

export default function Login() {
  const [content, setContent] = createSignal("");
  const [status, setStatus] = createSignal<Status>("loading");
  const [error, setError] = createSignal("");
  let run = 0;
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  const cancel = (code: string) => code && api.loginQrCancel(code).catch(() => {});

  const start = async () => {
    const my = ++run;
    cancel(content());
    setContent("");
    setError("");
    setStatus("loading");
    let code: string;
    try {
      code = await api.loginQrCode();
    } catch (e) {
      if (my === run) {
        setStatus("failed");
        setError(`获取二维码失败：${errorText(e)}`);
      }
      return;
    }
    if (my !== run) return void cancel(code);
    setContent(code);
    setStatus("waiting");
    const deadline = Date.now() + QR_TTL_MS;
    let failures = 0;
    while (my === run) {
      await sleep(POLL_MS);
      if (my !== run) return;
      if (Date.now() > deadline) {
        cancel(code);
        setStatus("unknown");
        return;
      }
      try {
        const { status: s } = await api.loginQrPoll(code);
        if (s === "confirmed" && session.phase === "login") {
          run++;
          setStatus("confirmed");
          await enter();
          if ((session.phase as string) !== "ready") setStatus("failed");
          return;
        }
        if (my !== run) return;
        failures = 0;
        setError("");
        setStatus(s);
        if (s === "unknown") return;
      } catch (e) {
        if (my !== run) return;
        setError(`查询扫码状态失败：${errorText(e)}`);
        if (++failures >= 5) {
          setStatus("failed");
          return;
        }
      }
    }
  };

  onMount(start);
  onCleanup(() => {
    run++;
    if (status() !== "confirmed") cancel(content());
  });

  const canRefresh = () => status() === "unknown" || status() === "failed" || Boolean(error());

  return (
    <div class="login" data-tauri-drag-region={dragRegion()}>
      <div class="login-card">
        <img class="login-logo" src="/icon.svg" alt="" width={64} height={64} draggable={false} />
        <h1>登录 cdapp</h1>
        <Show when={session.notice}><p class="login-error">{session.notice}</p></Show>
        <div class="qr" innerHTML={content() && status() !== "unknown" ? renderSVG(content(), { border: 2 }) : ""} />
        <Show when={TEXT[status()]}><p>{TEXT[status()]}</p></Show>
        <Show when={error()}><p class="login-error">{error()}</p></Show>
        <Show when={canRefresh()}>
          <button class="login-retry" onClick={start}>刷新二维码</button>
        </Show>
      </div>
    </div>
  );
}
