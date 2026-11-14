import { createStore } from "solid-js/store";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api } from "../lib/api";
import type { ShareEvent, ShareSource } from "../lib/api";
import { readAccount, writeAccount } from "../lib/accountState";
import { voice } from "./voice";

export type AudioTab = "screen" | "window";
const AUDIO_OFF: Record<AudioTab, boolean> = { screen: false, window: false };
export const [shareAudio, setShareAudioState] = createStore<Record<AudioTab, boolean>>({ ...AUDIO_OFF });
let audioOwner = "";

export function parseShareAudio(v: unknown): Record<AudioTab, boolean> {
  const o = v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
  return { screen: o.screen === true, window: o.window === true };
}

export async function loadShareAudio(me: string) {
  audioOwner = me;
  const raw = await readAccount(me, "shareAudio");
  if (audioOwner === me) setShareAudioState(parseShareAudio(raw));
}

export function resetShareAudio() {
  audioOwner = "";
  setShareAudioState({ ...AUDIO_OFF });
}

export function setShareAudio(tab: AudioTab, on: boolean) {
  setShareAudioState(tab, on);
  if (audioOwner) writeAccount(audioOwner, "shareAudio", { ...shareAudio });
}

type Phase = "idle" | "starting" | "live";
type State = {
  phase: Phase;
  generation: number;
  channelId: string;
  source: ShareSource | null;
  resolution: number;
  frameRate: number;
  audio: boolean;
  width: number;
  height: number;
  kbps: number;
  fps: number;
  updating: boolean;
  watchable: boolean;
};

const IDLE: State = {
  phase: "idle", generation: 0, channelId: "", source: null,
  resolution: 1080, frameRate: 60, audio: false,
  width: 0, height: 0, kbps: 0, fps: 0, updating: false, watchable: false,
};

export const [share, setShare] = createStore<State>({ ...IDLE });
export const sharing = () => share.phase !== "idle";

export const RESOLUTIONS = [{ v: 720, name: "720p" }, { v: 1080, name: "1080p" }];
export const FRAME_RATES = [{ v: 30, name: "30 帧" }, { v: 60, name: "60 帧" }];

type Payload = { generation: number; event: ShareEvent };

let early: Payload[] = [];
let listening: Promise<UnlistenFn> | null = null;
function ensureListener() {
  listening ??= listen<Payload>("share://event", ({ payload }) => onShareEvent(payload)).catch((e) => {
    listening = null;
    throw e;
  });
  return listening;
}

function onShareEvent(p: Payload) {
  if (share.phase === "starting" && share.generation === 0) {
    early.push(p);
    return;
  }
  if (p.generation !== share.generation) return;
  const e = p.event;
  switch (e.kind) {
    case "live":
      setShare({ phase: "live", width: e.width, height: e.height, watchable: e.watchable });
      break;
    case "tier":
      setShare({ resolution: e.resolution, frameRate: e.frameRate, width: e.width, height: e.height });
      break;
    case "stats":
      setShare({ kbps: e.kbps, fps: e.fps });
      break;
    case "ended":
      setShare({ ...IDLE, resolution: share.resolution, frameRate: share.frameRate });
      break;
  }
}

export async function loadSources(): Promise<ShareSource[]> {
  const list = await api.shareSources();
  return list
    .filter((s) => s.kind === "window" || s.name.startsWith("\\\\.\\DISPLAY"))
    .sort((a, b) => Number(b.primary) - Number(a.primary));
}

export async function startShare(source: ShareSource, resolution: number, frameRate: number, audio: boolean) {
  if (sharing() || !voice.channelId) return;
  early = [];
  setShare({ ...IDLE, phase: "starting", channelId: voice.channelId, source, resolution, frameRate, audio });
  try {
    await ensureListener();
    const generation = await api.shareStart({
      channelId: voice.channelId, sourceKind: source.kind, sourceId: source.id, sourcePid: source.pid,
      resolution, frameRate, audio,
    });
    setShare("generation", generation);
    const buffered = early;
    early = [];
    for (const p of buffered) onShareEvent(p);
  } catch (e) {
    early = [];
    console.warn("屏幕共享开启失败", e);
    setShare({ ...IDLE, resolution, frameRate });
  }
}

export async function stopShare() {
  if (!sharing()) return;
  try {
    await api.shareStop();
  } finally {
    if (sharing()) setShare({ ...IDLE, resolution: share.resolution, frameRate: share.frameRate });
  }
}

export async function updateShare(resolution: number, frameRate: number) {
  if (share.phase !== "live" || share.updating) return;
  if (resolution === share.resolution && frameRate === share.frameRate) return;
  setShare("updating", true);
  try {
    await api.shareUpdate(share.generation, resolution, frameRate);
  } catch (e) {
    console.warn("屏幕共享改档失败", e);
  } finally {
    setShare("updating", false);
  }
}
