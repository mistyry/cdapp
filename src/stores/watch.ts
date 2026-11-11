import { createEffect, createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { Channel } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type WatchEvent } from "../lib/api";
import { ScreenDecoder } from "../lib/screenDecoder";
import { liveOf } from "./session";
import { voice, voiceActive } from "./voice";

// 观看他人的屏幕分享。同一时间只看一人。
// mode：stage 舞台内；app 软件内全屏；screen 全屏；pip 悬浮窗。
// phase：connecting 已发出请求；waiting 已进房，等待画面；playing 画面送达中。
// 结束与失败只回到未观看，不显示文字，原因由后端写入日志。
// 悬浮窗是独立窗口，自己接收画面并解码；切回主窗口时画面改发回主窗口的通道，观看不中断。
export type WatchMode = "stage" | "app" | "screen" | "pip";
type Phase = "idle" | "connecting" | "waiting" | "playing";
type State = {
  phase: Phase;
  generation: number;
  userId: string;
  channelId: string;
  mode: WatchMode;
  // 画面尺寸，未收到时为 0。
  width: number;
  height: number;
  kbps: number;
};

const IDLE: State = { phase: "idle", generation: 0, userId: "", channelId: "", mode: "stage", width: 0, height: 0, kbps: 0 };

export const [watch, setWatch] = createStore<State>({ ...IDLE });
export const watching = () => watch.phase !== "idle";

// 主窗口的解码器与画面通道跟随观看会话，画面承载位置变化时只重新绑定 canvas。
let decoder: ScreenDecoder | null = null;
// 当前承载画面的画布。解码器重建时绑定到它。
let canvasEl: HTMLCanvasElement | null = null;
export function attachCanvas(canvas: HTMLCanvasElement | null) {
  canvasEl = canvas;
  decoder?.attach(canvas);
}

// 悬浮窗与主窗口之间的通知：悬浮窗请求回到舞台或停止观看。
export const PIP_EVENT = "watch://pip";
export type PipMessage = { generation: number; action: "back" | "stop" };

type Payload = { generation: number; state: WatchEvent };
let listening: Promise<UnlistenFn[]> | null = null;
function ensureListener() {
  listening ??= Promise.all([
    listen<Payload>("watch://event", ({ payload }) => onWatchEvent(payload)),
    listen<PipMessage>(PIP_EVENT, ({ payload }) => {
      if (payload.generation !== watch.generation || !watching()) return;
      if (payload.action === "stop") void stopWatch();
      else void setWatchMode("stage");
    }),
  ]).catch((e) => {
    listening = null;
    throw e;
  });
  return listening;
}

// 开始命令返回代次之前到达的事件，确定代次后再处理。
let early: Payload[] = [];

function onWatchEvent(p: Payload) {
  if (watch.phase === "connecting" && watch.generation === 0) {
    early.push(p);
    return;
  }
  if (p.generation !== watch.generation || !watching()) return;
  const s = p.state;
  switch (s.kind) {
    case "waiting":
      setWatch("phase", "waiting");
      break;
    case "playing":
      setWatch("phase", "playing");
      break;
    case "stats":
      setWatch("kbps", s.kbps);
      break;
    case "ended":
      void leaveMode(watch.mode);
      reset();
      break;
  }
}

function reset() {
  decoder?.close();
  decoder = null;
  early = [];
  setWatch({ ...IDLE });
}

function newDecoder() {
  const dec = new ScreenDecoder(
    () => { if (watch.generation) void api.watchKeyFrame(watch.generation).catch(() => {}); },
    (width, height) => setWatch({ width, height }),
  );
  if (canvasEl) dec.attach(canvasEl);
  return dec;
}

// 开始观看。已在看他人时切换过去，观看方式回到舞台。
export async function startWatch(channelId: string, userId: string) {
  if (watching() && watch.userId === userId && watch.channelId === channelId) return;
  if (watching()) await stopWatch();
  const dec = newDecoder();
  decoder = dec;
  early = [];
  setWatch({ ...IDLE, phase: "connecting", userId, channelId });
  const ch = new Channel<ArrayBuffer>();
  ch.onmessage = (buf) => { if (decoder === dec) dec.push(buf); };
  try {
    await ensureListener();
    const generation = await api.watchStart(channelId, userId, ch);
    // 等待期间已停止或切换到他人。
    if (decoder !== dec) return;
    setWatch("generation", generation);
    const buffered = early;
    early = [];
    for (const p of buffered) onWatchEvent(p);
  } catch (e) {
    console.warn("观看屏幕分享失败", e);
    if (decoder === dec) reset();
  }
}

export async function stopWatch() {
  if (!watching()) return;
  const mode = watch.mode;
  reset();
  await leaveMode(mode);
  try {
    await api.watchStop();
  } catch (e) {
    console.warn("停止观看失败", e);
  }
}

// 离开某个观看方式时恢复窗口：退出全屏、关闭悬浮窗。失败只记日志。
async function leaveMode(mode: WatchMode) {
  try {
    if (mode === "screen") await getCurrentWindow().setFullscreen(false);
    if (mode === "pip") await api.watchPipClose();
  } catch (e) {
    console.warn("恢复观看窗口失败", e);
  }
}

// 切换观看方式。舞台、软件内全屏与全屏共用主窗口的解码器；悬浮窗由独立窗口接管画面，
// 回到主窗口时画面改发回主窗口的通道，主窗口解码器重建并等待关键帧。
export async function setWatchMode(mode: WatchMode) {
  if (!watching() || mode === watch.mode) return;
  const from = watch.mode;
  setWatch("mode", mode);
  try {
    if (from === "screen" && mode !== "screen") await getCurrentWindow().setFullscreen(false);
    if (mode === "screen") await getCurrentWindow().setFullscreen(true);
    if (mode === "pip") {
      await api.watchPipOpen(watch.generation);
    } else if (from === "pip") {
      await api.watchPipClose();
      if (decoder) {
        // 后端替换出口时会释放旧通道，前端随之注销其回调，因此每次接管都用新通道。
        // 悬浮窗期间主窗口没有收到帧，解码器从下一个关键帧开始。
        const dec = newDecoder();
        decoder.close();
        decoder = dec;
        const ch = new Channel<ArrayBuffer>();
        ch.onmessage = (buf) => { if (decoder === dec) dec.push(buf); };
        await api.watchRedirect(watch.generation, ch);
      }
    }
  } catch (e) {
    console.warn("切换观看方式失败", e);
  }
}

// 悬浮窗页面通知主窗口。
export function notifyMain(generation: number, action: PipMessage["action"]) {
  return emit(PIP_EVENT, { generation, action } satisfies PipMessage);
}

// 分享者停播、离开语音频道，或本人离开该语音频道时停止观看。
createRoot(() => {
  createEffect(() => {
    if (!watching()) return;
    const sharing = Boolean(liveOf(watch.channelId, watch.userId));
    const inVoice = voiceActive() && voice.channelId === watch.channelId;
    if (!sharing || !inVoice) void stopWatch();
  });
});
