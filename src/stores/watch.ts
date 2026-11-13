import { createEffect, createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { Channel } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type WatchEvent } from "../lib/api";
import { ScreenDecoder } from "../lib/screenDecoder";
import { liveOf } from "./session";
import { voice, voiceActive } from "./voice";

export type WatchMode = "stage" | "app" | "screen" | "pip";
type Phase = "idle" | "connecting" | "waiting" | "playing";
type State = {
  phase: Phase;
  generation: number;
  userId: string;
  channelId: string;
  mode: WatchMode;
  width: number;
  height: number;
  kbps: number;
};

const IDLE: State = { phase: "idle", generation: 0, userId: "", channelId: "", mode: "stage", width: 0, height: 0, kbps: 0 };

export const [watch, setWatch] = createStore<State>({ ...IDLE });
export const watching = () => watch.phase !== "idle";

let decoder: ScreenDecoder | null = null;
let canvasEl: HTMLCanvasElement | null = null;
export function attachCanvas(canvas: HTMLCanvasElement | null) {
  canvasEl = canvas;
  decoder?.attach(canvas);
}

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

async function leaveMode(mode: WatchMode) {
  try {
    if (mode === "screen") await getCurrentWindow().setFullscreen(false);
    if (mode === "pip") await api.watchPipClose();
  } catch (e) {
    console.warn("恢复观看窗口失败", e);
  }
}

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

export function notifyMain(generation: number, action: PipMessage["action"]) {
  return emit(PIP_EVENT, { generation, action } satisfies PipMessage);
}

createRoot(() => {
  createEffect(() => {
    if (!watching()) return;
    const sharing = Boolean(liveOf(watch.channelId, watch.userId));
    const inVoice = voiceActive() && voice.channelId === watch.channelId;
    if (!sharing || !inVoice) void stopWatch();
  });
});
