import { createEffect, createRoot, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type WatchEvent } from "../lib/api";
import { PIP_EVENT, type PipMessage } from "../lib/pip";
import { ScreenDecoder } from "../lib/screenDecoder";
import { liveOf } from "./session";
import { share } from "./share";
import { setStageCollapsed } from "./ui";
import { shareMuteTarget, shareVolumeOf, storeShareVolume, voice, voiceActive } from "./voice";

export type WatchMode = "stage" | "app" | "screen" | "pip" | "kept";
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
  paused: boolean;
};

const IDLE: State = { phase: "idle", generation: 0, userId: "", channelId: "", mode: "stage", width: 0, height: 0, kbps: 0, paused: false };

export const [watch, setWatch] = createStore<State>({ ...IDLE });
export const watching = () => watch.phase !== "idle";
export const watchingSelf = () => watching() && watch.userId === voice.me?.id;
export const selfWatchable = () => share.phase === "live" && share.watchable && share.channelId === voice.channelId;

export async function setShareVolume(userId: string, percent: number) {
  const v = storeShareVolume(userId, percent);
  if (watching() && watch.userId === userId && watch.generation) await api.watchVolume(watch.generation, v).catch((e) => console.warn("调整屏幕共享音量失败", e));
}

export async function toggleShareMute(userId: string) {
  await setShareVolume(userId, shareMuteTarget(userId));
}

let decoder: ScreenDecoder | null = null;
let canvasEl: HTMLCanvasElement | null = null;
export function attachCanvas(canvas: HTMLCanvasElement) {
  canvasEl = canvas;
  decoder?.attach(canvas);
}

export function detachCanvas(canvas: HTMLCanvasElement) {
  if (canvasEl !== canvas) return;
  canvasEl = null;
  decoder?.attach(null);
}

export async function watchFromList(userId: string) {
  setStageCollapsed(false);
  await startWatch(voice.channelId, userId);
}

const PIP_CLOSED_EVENT = "watch://pip-closed";
export const PIP_READY_TIMEOUT = 8000;

let pipSeq = 0;
let pipOpen = 0;
let pipReady = 0;
const currentPip = (generation: number, seq: number) =>
  generation === watch.generation && seq === pipOpen && watching() && watch.mode === "pip";
let pipTimer: ReturnType<typeof setTimeout> | undefined;
function clearPipTimer() {
  clearTimeout(pipTimer);
  pipTimer = undefined;
}

type Payload = { generation: number; state: WatchEvent };
let listening: Promise<UnlistenFn[]> | null = null;
function ensureListener() {
  listening ??= Promise.all([
    listen<Payload>("watch://event", ({ payload }) => onWatchEvent(payload)),
    listen<PipMessage>(PIP_EVENT, ({ payload }) => {
      if (!currentPip(payload.generation, payload.seq)) return;
      if (payload.action === "ready") {
        pipReady = payload.seq;
        clearPipTimer();
        return;
      }
      void setWatchMode("stage");
    }),
    listen<{ generation: number; seq: number }>(PIP_CLOSED_EVENT, ({ payload }) => {
      if (currentPip(payload.generation, payload.seq)) void setWatchMode("stage");
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
    case "paused":
      setWatch("paused", s.paused);
      break;
    case "ended":
      void leaveMode(watch.mode);
      reset();
      break;
  }
}

let chain: Promise<unknown> = Promise.resolve();
function serial<T>(f: () => Promise<T>): Promise<T> {
  const r = chain.then(f);
  chain = r.catch(() => {});
  return r;
}

function reset() {
  clearPipTimer();
  pipOpen = 0;
  pipReady = 0;
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

function frameChannel(dec: ScreenDecoder) {
  const ch = new Channel<ArrayBuffer>();
  ch.onmessage = (buf) => { if (decoder === dec) dec.push(buf); };
  return ch;
}

function rewire() {
  if (!decoder || !watch.generation) return null;
  const dec = newDecoder();
  decoder.close();
  decoder = dec;
  return frameChannel(dec);
}

const [refreshing, setRefreshing] = createSignal(false);
export const refreshBusy = () => refreshing() || watch.phase === "connecting";

export async function refreshWatch() {
  if (!watching() || refreshBusy()) return;
  setRefreshing(true);
  try {
    const { channelId, userId } = watch;
    await stopWatch();
    await startWatch(channelId, userId);
  } finally {
    setRefreshing(false);
  }
}

export async function startWatch(channelId: string, userId: string) {
  if (watching() && watch.userId === userId && watch.channelId === channelId) {
    if (watch.mode === "kept" || watch.mode === "pip") await setWatchMode("stage");
    return;
  }
  if (watching()) await stopWatch();
  const dec = newDecoder();
  decoder = dec;
  early = [];
  setWatch({ ...IDLE, phase: "connecting", userId, channelId });
  const ch = frameChannel(dec);
  try {
    const sent = shareVolumeOf(userId);
    const generation = await serial(async () => {
      await ensureListener();
      return api.watchStart(channelId, userId, sent, ch);
    });
    if (decoder !== dec) return;
    setWatch("generation", generation);
    if (shareVolumeOf(userId) !== sent) await api.watchVolume(generation, shareVolumeOf(userId)).catch((e) => console.warn("调整屏幕共享音量失败", e));
    const buffered = early;
    early = [];
    for (const p of buffered) onWatchEvent(p);
  } catch (e) {
    console.warn("观看屏幕共享失败", e);
    if (decoder === dec) reset();
  }
}

export async function stopWatch() {
  if (!watching()) return;
  const mode = watch.mode;
  reset();
  const stopped = serial(() => api.watchStop());
  await leaveMode(mode);
  try {
    await stopped;
  } catch (e) {
    console.warn("停止观看失败", e);
  }
}

async function leaveMode(mode: WatchMode) {
  const closing = mode === "pip" ? serial(() => api.watchPipClose()) : null;
  try {
    if (mode === "screen") await getCurrentWindow().setFullscreen(false);
    await closing;
  } catch (e) {
    console.warn("恢复观看窗口失败", e);
  }
}

function backendFor(from: WatchMode, mode: WatchMode, generation: number) {
  if (mode === "pip") {
    const seq = ++pipSeq;
    pipOpen = seq;
    return { seq, done: serial(() => api.watchPipOpen(generation, seq)) };
  }
  if (from !== "pip") return { done: Promise.resolve() };
  const ch = rewire();
  return {
    done: serial(async () => {
      await api.watchPipClose();
      if (ch) await api.watchRedirect(generation, ch);
    }),
  };
}

export async function setWatchMode(mode: WatchMode) {
  if (!watching() || mode === watch.mode) return;
  if ((mode === "pip" || mode === "kept") && !watch.generation) return;
  const from = watch.mode;
  setWatch("mode", mode);
  if (from === "pip") {
    clearPipTimer();
    pipReady = 0;
    pipOpen = 0;
  }
  const generation = watch.generation;
  const { seq, done } = backendFor(from, mode, generation);
  try {
    if (from === "screen" && mode !== "screen") await getCurrentWindow().setFullscreen(false);
    if (mode === "screen") await getCurrentWindow().setFullscreen(true);
  } catch (e) {
    console.warn("切换观看方式失败", e);
  }
  try {
    await done;
    if (seq) armPipTimer(generation, seq);
  } catch (e) {
    if (seq && watch.mode === "pip" && pipOpen === seq) {
      pipOpen = 0;
      setWatch("mode", from === "screen" ? "stage" : from);
    }
    console.warn("切换观看方式失败", e);
  }
}

function armPipTimer(generation: number, seq: number) {
  clearPipTimer();
  if (pipReady === seq || !currentPip(generation, seq)) return;
  pipTimer = setTimeout(() => {
    pipTimer = undefined;
    if (currentPip(generation, seq) && pipReady !== seq) {
      console.warn("悬浮窗未能就绪");
      void setWatchMode("stage");
    }
  }, PIP_READY_TIMEOUT);
}

export function sourceGone() {
  if (!watching()) return false;
  const sharing = watch.userId === voice.me?.id ? selfWatchable() : Boolean(liveOf(watch.channelId, watch.userId));
  const inVoice = voiceActive() && voice.channelId === watch.channelId;
  return !sharing || !inVoice;
}

createRoot(() => {
  createEffect(() => {
    if (sourceGone()) void stopWatch();
  });
});
