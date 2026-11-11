import { createStore } from "solid-js/store";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api } from "../lib/api";
import type { ShareEvent, ShareSource } from "../lib/api";
import { voice } from "./voice";

// 屏幕分享状态。idle 未分享；starting 已发出开播，等待发布；live 推流中。
// 结束、开播失败与改档失败只回到对应状态，不显示文字，原因由后端写入日志。
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
  // 改档请求进行中。
  updating: boolean;
};

const IDLE: State = {
  phase: "idle", generation: 0, channelId: "", source: null,
  resolution: 720, frameRate: 30, audio: false,
  width: 0, height: 0, kbps: 0, fps: 0, updating: false,
};

export const [share, setShare] = createStore<State>({ ...IDLE });
export const sharing = () => share.phase !== "idle";

// 标准版档位。只提供账号拥有的档位，服务端在改档时不校验。
export const RESOLUTIONS = [{ v: 720, name: "720p" }, { v: 1080, name: "1080p" }];
export const FRAME_RATES = [{ v: 30, name: "30 帧" }, { v: 60, name: "60 帧" }];

type Payload = { generation: number; event: ShareEvent };

// 开播命令返回代次之前到达的事件，确定代次后再处理。
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
      setShare({ phase: "live", width: e.width, height: e.height });
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

// 列出可共享的源。主显示器排在最前。
export async function loadSources(): Promise<ShareSource[]> {
  const list = await api.shareSources();
  // VirtualDesktop 为全部屏幕拼接，不提供。
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
    console.warn("屏幕分享开启失败", e);
    setShare({ ...IDLE, resolution, frameRate });
  }
}

export async function stopShare() {
  if (!sharing()) return;
  try {
    await api.shareStop();
  } finally {
    // 结束事件可能先于命令返回到达，这里只在仍未回到空闲时复位。
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
    console.warn("屏幕分享改档失败", e);
  } finally {
    setShare("updating", false);
  }
}
