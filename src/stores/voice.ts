import { createEffect, createRoot, createSignal } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api, errorText, type DenoiseMode, type InputDevice } from "../lib/api";
import { Effect, applyEffectPrefs, playEffect, playJoin } from "./effects";
import { readAccount, writeAccount } from "../lib/accountState";
import { channelById } from "./session/state";

export type VoicePeer = {
  id: string; name: string; avatar: string; device: string;
  micOff: boolean; headsetOff: boolean; permissionSay: boolean; serverMute: boolean; serverDeafen?: boolean;
  joinVoice?: number;
};
export type VoicePatch = Partial<Omit<VoicePeer, "avatar" | "device">> & { id: string };
export type VoicePhase = "idle" | "connecting" | "connected" | "reconnecting" | "disconnected";
export type EndReason = "left" | "replaced" | "exited" | "kicked" | "join_failed" | "lost" | "unauthorized" | "password" | "revoked";

export type LinkQuality = "good" | "fair" | "poor";
export type UpStats = { score: number; loss: number; packets: number; lost: number; bitrate: number };
export type DownStats = { packets: number; lost: number; late: number; overflow: number; underruns: number; depthMs: number };

export type VoiceEvent =
  | { type: "state"; phase: "connecting" | "connected" | "reconnecting" | "disconnected"; reason?: EndReason; message?: string;
      attempt?: number; max?: number; retry_in_ms?: number }
  | { type: "joined"; me: VoicePeer; peers: VoicePeer[] }
  | { type: "peer_joined"; peer: VoicePeer }
  | { type: "peer_left"; id: string }
  | { type: "peer_changed"; patch: VoicePatch }
  | { type: "mic"; status: MicStatus; muted: boolean; device?: string; message?: string }
  | { type: "mic_level"; level: number }
  | { type: "output"; deafened: boolean; active: boolean; device?: string; message?: string }
  | { type: "speaking"; ids: string[] }
  | { type: "link"; quality?: LinkQuality; up?: UpStats; rtt?: number; down: DownStats };
export type MicStatus = "off" | "starting" | "active" | "muted" | "denied" | "no_device" | "device_lost" | "error";
export type VoicePayload = { generation: number; channelId: string; event: VoiceEvent };

type State = {
  channelId: string;
  generation: number;
  phase: VoicePhase;
  reason: EndReason | null;
  message: string | null;
  me: VoicePeer | null;
  peers: VoicePeer[];
  attempt: number;
  maxAttempts: number;
  retryAt: number | null;
  quality: LinkQuality | null;
  rtt: number | null;
};

const IDLE: State = { channelId: "", generation: 0, phase: "idle", reason: null, message: null, me: null, peers: [], attempt: 0, maxAttempts: 0, retryAt: null, quality: null, rtt: null };

export const [voice, setVoice] = createStore<State>({ ...IDLE });

export const latencyLevel = (rtt: number) => (rtt >= 200 ? 1 : rtt >= 100 ? 2 : 3);

type Mic = {
  muted: boolean;
  status: MicStatus;
  device: string | null;
  message: string | null;
  level: number;
  busy: boolean;
};

const MIC_IDLE = { status: "off" as MicStatus, device: null, message: null, level: 0, busy: false };
export const [mic, setMic] = createStore<Mic>({ muted: false, ...MIC_IDLE });

export const DENOISE_MODES: { id: DenoiseMode; name: string }[] = [
  { id: "kook", name: "KOOK AI 降噪 2.0" },
  { id: "rnnoise", name: "RNNoise" },
];
export const [denoise, setDenoiseState] = createSignal(false);
export const [denoiseMode, setDenoiseModeState] = createSignal<DenoiseMode>("kook");
export const [kookFallback, setKookFallback] = createSignal(false);
let denoiseSeq = 0;
export async function applyDenoise(on: boolean, mode: DenoiseMode) {
  const my = ++denoiseSeq;
  const prev = [denoise(), denoiseMode()] as const;
  setDenoiseState(on);
  setDenoiseModeState(mode);
  setKookFallback(false);
  try {
    await api.voiceSetDenoise(on, mode);
  } catch (e) {
    if (my !== denoiseSeq) return;
    setDenoiseState(prev[0]);
    setDenoiseModeState(prev[1]);
    setMic("message", `降噪未开启：${errorText(e)}`);
  }
}
let denoiseFallbackListening: Promise<UnlistenFn> | null = null;
export function watchDenoiseFallback() {
  denoiseFallbackListening ??= listen<string>("denoise://fallback", () => {
    setKookFallback(true);
  }).catch((e) => {
    denoiseFallbackListening = null;
    throw e;
  });
  return denoiseFallbackListening;
}
export const setDenoise = (on: boolean) => applyDenoise(on, denoiseMode());
export const setDenoiseMode = (mode: DenoiseMode) => applyDenoise(denoise(), mode);

type Out = {
  deafened: boolean;
  active: boolean;
  device: string | null;
  message: string | null;
  busy: boolean;
  speaking: string[];
  volumes: Record<string, number>;
  shares: Record<string, number>;
  shareBeforeMute: Record<string, number>;
  denoised: Record<string, boolean>;
  master: number;
  gain: number;
};
const OUT_IDLE = { active: false, device: null, message: null, busy: false, speaking: [] as string[] };
export const [output, setOutput] = createStore<Out>({ deafened: false, ...OUT_IDLE, volumes: {}, shares: {}, shareBeforeMute: {}, denoised: {}, master: 100, gain: 100 });
const SELF_SPEAK_LEVEL = 23;
const SELF_SPEAK_HOLD_MS = 300;
const [selfSpeaking, setSelfSpeaking] = createSignal(false);
let selfSpeakTimer: ReturnType<typeof setTimeout> | undefined;
function onSelfLevel(level: number) {
  if (level < SELF_SPEAK_LEVEL || mic.muted || output.deafened) return;
  setSelfSpeaking(true);
  clearTimeout(selfSpeakTimer);
  selfSpeakTimer = setTimeout(() => setSelfSpeaking(false), SELF_SPEAK_HOLD_MS);
}
function clearSelfSpeaking() {
  clearTimeout(selfSpeakTimer);
  setSelfSpeaking(false);
}
export const isSpeaking = (id: string) => (id === voice.me?.id ? selfSpeaking() : output.speaking.includes(id));
export const volumeOf = (id: string) => output.volumes[id] ?? 100;

let seq = Date.now();
let early: VoicePayload[] = [];
let joining = false;
let wanted: boolean | null = null;
let wantedDeaf: boolean | null = null;
let micBeforeDeaf: boolean | null = null;

export const voiceActive = () => voice.phase === "connecting" || voice.phase === "connected" || voice.phase === "reconnecting";

const REASONS: Record<EndReason, string> = {
  left: "已退出语音",
  replaced: "已在其他设备加入语音",
  exited: "语音已结束",
  kicked: "已被管理员移出语音频道",
  join_failed: "加入语音失败",
  lost: "语音连接已断开",
  unauthorized: "登录已失效",
  password: "需要频道密码",
  revoked: "已失去该语音频道的权限",
};

export function voiceStatusText(): string {
  switch (voice.phase) {
    case "connecting": return "正在连接";
    case "connected": return "语音已连接";
    case "reconnecting": return voice.attempt ? `正在重连，第 ${voice.attempt}/${voice.maxAttempts} 次` : "正在重连";
    case "disconnected": return voice.reason ? REASONS[voice.reason] : "语音已断开";
    default: return "";
  }
}

let listening: Promise<UnlistenFn> | null = null;
function ensureListener() {
  listening ??= listen<VoicePayload>("voice://event", ({ payload }) => onVoiceEvent(payload)).catch((e) => {
    listening = null;
    throw e;
  });
  return listening;
}

export const [canGoLive, setCanGoLive] = createSignal(false);

export async function joinVoice(channelId: string, password: string | null = null) {
  if (voiceActive() && voice.channelId === channelId) return;
  if (micTest.on) { ++testSeq; setMicTest({ on: false, level: 0 }); }
  const my = ++seq;
  setCanGoLive(false);
  api.goliveAllowed().then((ok) => { if (my === seq) setCanGoLive(ok); }, () => {});
  setKookFallback(false);
  joining = true;
  wanted = null;
  wantedDeaf = null;
  early = [];
  selfJoinPending = false;
  setVoice({ ...IDLE, channelId, phase: "connecting" });
  setMic(MIC_IDLE);
  setOutput(OUT_IDLE);
  try {
    await ensureListener();
    const sent = mic.muted;
    const sentDeaf = output.deafened;
    const generation = await api.voiceJoin(channelId, my, sent, sentDeaf, sentDeaf ? micBeforeDeaf : null, password);
    if (my !== seq) return;
    joining = false;
    setVoice("generation", generation);
    const buffered = early;
    early = [];
    for (const p of buffered) onVoiceEvent(p);
    const w = wanted;
    const d = wantedDeaf;
    wanted = null;
    wantedDeaf = null;
    if (d !== null && d !== sentDeaf) await setDeafened(d);
    if (w !== null && w !== sent) await setMuted(w);
  } catch (e) {
    if (my !== seq) return;
    joining = false;
    early = [];
    setVoice({ phase: "disconnected", reason: "join_failed", message: errorText(e) });
  }
}

export async function leaveVoice() {
  const my = ++seq;
  setCanGoLive(false);
  joining = false;
  wanted = null;
  wantedDeaf = null;
  early = [];
  selfJoinPending = false;
  setVoice({ ...IDLE });
  setMic(MIC_IDLE);
  setOutput(OUT_IDLE);
  await api.voiceLeave(my).catch(() => {});
}

export async function disconnectVoice() {
  if (voiceActive()) playEffect(Effect.EXIT_CHANNEL);
  await leaveVoice();
}

const PEER_QUIET_MS = 1000;
const PEER_LIMIT = 30;
let quietUntil = 0;
let selfJoinPending = false;
const peerSoundOk = () => voice.phase === "connected" && Date.now() >= quietUntil;
const inRadio = () => Boolean(channelById(voice.channelId)?.channel.radio);

let onPassword: (channelId: string) => void = () => {};
export const setPasswordHandler = (f: (channelId: string) => void) => (onPassword = f);

export async function revokeVoice() {
  const p = leaveVoice();
  setVoice({ channelId: "", phase: "disconnected", reason: "revoked", message: null });
  await p;
}

export const canRejoin = () =>
  voice.phase === "disconnected" && !!voice.channelId && (voice.reason === "lost" || voice.reason === "exited" || voice.reason === "join_failed");

export function rejoinVoice() {
  if (canRejoin()) return joinVoice(voice.channelId);
}

export function voiceNetworkBack() {
  if ((voice.phase === "reconnecting" || voice.phase === "connected") && voice.generation) api.voiceWake().catch(() => {});
}

const MIC_SHORT: Partial<Record<MicStatus, string>> = {
  muted: "已静音", off: "未开启", starting: "正在启动", denied: "无权限", no_device: "无设备", device_lost: "已断开", error: "出错",
};
export function deviceSummary(): { text: string; title: string; ok: boolean } {
  const micOk = mic.status === "active" || mic.status === "muted" || mic.status === "starting";
  const micPart = mic.status === "active" ? mic.device ?? "麦克风" : MIC_SHORT[mic.status] ?? "";
  const outOk = output.deafened || output.active;
  const outPart = output.deafened ? "已关闭" : output.active ? output.device ?? "扬声器" : "未开启";
  const notes = [mic.status === "active" ? mic.message : null, output.message].filter(Boolean);
  return {
    text: `麦克风：${micPart} 扬声器：${outPart}`,
    title: [`麦克风：${micOk && mic.device ? mic.device : micText()}`, `扬声器：${outPart}`, ...notes].join("\n"),
    ok: micOk && outOk,
  };
}

const QUALITY_TEXT: Record<LinkQuality, string> = { good: "良好", fair: "一般", poor: "较差" };
export const qualityText = () => (voice.quality ? QUALITY_TEXT[voice.quality] : null);

export function dismissVoice() {
  if (voice.phase === "disconnected") setVoice({ ...IDLE });
}

export function resetVoice() {
  seq++;
  setCanGoLive(false);
  joining = false;
  wanted = null;
  wantedDeaf = null;
  early = [];
  selfJoinPending = false;
  setVoice({ ...IDLE });
  setMic({ muted: false, ...MIC_IDLE });
  setOutput({ deafened: false, ...OUT_IDLE, volumes: {}, shares: {}, shareBeforeMute: {}, denoised: {} });
  volumeOwner = "";
  for (const k of Object.keys(beforeMute)) delete beforeMute[k];
  micBeforeDeaf = null;
}

export const micFailed = () => ["denied", "no_device", "device_lost", "error"].includes(mic.status);

const MIC_TEXT: Record<MicStatus, string> = {
  off: "麦克风未开启",
  starting: "正在打开麦克风",
  active: "麦克风已开启",
  muted: "麦克风已静音",
  denied: "麦克风权限被拒绝",
  no_device: "没有可用的麦克风",
  device_lost: "麦克风已断开",
  error: "麦克风出错",
};

export function micText(): string {
  return MIC_TEXT[mic.status];
}

export function micHint(): string | null {
  switch (mic.status) {
    case "denied": return "请在系统设置的隐私与安全性中允许 cdapp 使用麦克风，然后点击重试";
    case "no_device": return "请连接麦克风后点击重试";
    case "device_lost": return mic.message ?? "麦克风已断开，请检查设备后点击重试";
    case "error": return mic.message;
    default: return null;
  }
}

export async function setMuted(muted: boolean) {
  switchesTouched = true;
  const generation = voice.generation;
  if (!voiceActive() || !generation || joining) {
    if (joining) wanted = muted;
    if (muted !== mic.muted) playEffect(muted ? Effect.MIC_MUTE : Effect.MIC_UNMUTE);
    setMic({ muted, status: muted ? "muted" : "off", message: null });
    if (!muted && output.deafened) setOutput("deafened", false);
    return;
  }
  setMic({ muted, busy: true, message: null, ...(muted ? { level: 0 } : {}) });
  try {
    await api.voiceSetMic(generation, muted);
    if (voice.generation === generation) playEffect(muted ? Effect.MIC_MUTE : Effect.MIC_UNMUTE);
  } catch (e) {
    if (voice.generation !== generation) return;
    setMic({ message: errorText(e) });
  } finally {
    if (voice.generation === generation) setMic("busy", false);
  }
}

export const toggleMic = () => setMuted(micFailed() ? false : !mic.muted);

export async function setDeafened(deafened: boolean) {
  switchesTouched = true;
  const generation = voice.generation;
  if (!voiceActive() || !generation || joining) {
    if (joining) wantedDeaf = deafened;
    if (deafened === output.deafened) return;
    playEffect(deafened ? Effect.DEAFEN : Effect.UNDEAFEN);
    if (deafened) {
      micBeforeDeaf = mic.muted;
      setMic({ muted: true, status: "muted", message: null });
    } else {
      const before = micBeforeDeaf ?? mic.muted;
      micBeforeDeaf = null;
      setMic({ muted: before, status: before ? "muted" : "off", message: null });
    }
    setOutput({ deafened, speaking: [] });
    return;
  }
  setOutput({ deafened, busy: true, message: null, ...(deafened ? { speaking: [] } : {}) });
  try {
    await api.voiceSetDeafen(generation, deafened);
    if (voice.generation === generation) playEffect(deafened ? Effect.DEAFEN : Effect.UNDEAFEN);
  } catch (e) {
    if (voice.generation !== generation) return;
    setOutput({ message: errorText(e) });
  } finally {
    if (voice.generation === generation) setOutput("busy", false);
  }
}

export const toggleDeafen = () => setDeafened(!output.deafened);

let volumeOwner = "";
export function parseVolumes(v: unknown): Record<string, number> {
  const out: Record<string, number> = {};
  if (!v || typeof v !== "object" || Array.isArray(v)) return out;
  for (const [id, n] of Object.entries(v)) {
    if (typeof n !== "number" || !Number.isFinite(n)) continue;
    const c = Math.max(0, Math.min(200, Math.round(n)));
    if (c !== 100) out[id] = c;
  }
  return out;
}

export function parseDenoised(v: unknown): string[] {
  return Array.isArray(v) ? [...new Set(v.filter((x): x is string => typeof x === "string" && x !== ""))] : [];
}

export async function loadPeerVolumes(me: string) {
  volumeOwner = me;
  const [rawVol, rawDn, rawShare, rawShareMute] = await Promise.all([readAccount(me, "peerVolumes"), readAccount(me, "peerDenoise"), readAccount(me, "shareVolumes"), readAccount(me, "shareBeforeMute")]);
  if (volumeOwner !== me) return;
  for (const [id, v] of Object.entries(parseVolumes(rawShare))) {
    if (!(id in output.shares)) setOutput("shares", id, v);
  }
  for (const [id, v] of Object.entries(parseVolumes(rawShareMute))) {
    if (v > 0 && !(id in output.shareBeforeMute)) setOutput("shareBeforeMute", id, v);
  }
  const stored = parseVolumes(rawVol);
  for (const id of parseDenoised(rawDn)) {
    if (id in output.denoised) continue;
    setOutput("denoised", id, true);
    void api.voiceSetPeerDenoise(id, true).catch(() => {});
  }
  for (const [id, v] of Object.entries(stored)) {
    if (id in output.volumes) continue;
    setOutput("volumes", id, v);
    await api.voiceSetVolume(id, v).catch(() => {});
  }
}

const beforeMute: Record<string, number> = {};
export async function togglePeerMute(id: string) {
  const v = volumeOf(id);
  if (v > 0) {
    beforeMute[id] = v;
    await setPeerVolume(id, 0);
  } else {
    const back = beforeMute[id] ?? 100;
    delete beforeMute[id];
    await setPeerVolume(id, back);
  }
}

export const peerDenoised = (id: string) => output.denoised[id] === true;

export async function setPeerDenoise(id: string, on: boolean) {
  setOutput("denoised", id, on);
  if (volumeOwner) writeAccount(volumeOwner, "peerDenoise", Object.keys(output.denoised).filter((k) => output.denoised[k]));
  await api.voiceSetPeerDenoise(id, on).catch((e) => {
    setOutput("denoised", id, !on);
    setOutput({ message: errorText(e) });
  });
}

export async function setPeerVolume(id: string, percent: number) {
  const v = Math.max(0, Math.min(200, Math.round(percent)));
  setOutput("volumes", id, v);
  if (volumeOwner) {
    const all = parseVolumes({ ...output.volumes });
    writeAccount(volumeOwner, "peerVolumes", all);
  }
  await api.voiceSetVolume(id, v).catch((e) => setOutput({ message: errorText(e) }));
}

export const clampVolume = (percent: number) => Math.max(0, Math.min(200, Math.round(Number.isFinite(percent) ? percent : 100)));

export const shareVolumeOf = (id: string) => output.shares[id] ?? 100;

export function storeShareVolume(id: string, percent: number) {
  const v = clampVolume(percent);
  setOutput("shares", id, v);
  if (volumeOwner) writeAccount(volumeOwner, "shareVolumes", parseVolumes({ ...output.shares }));
  return v;
}

export function shareMuteTarget(id: string) {
  const v = shareVolumeOf(id);
  const shares = { ...output.shareBeforeMute };
  let target: number;
  if (v > 0) {
    shares[id] = v;
    target = 0;
  } else {
    target = shares[id] ?? 100;
    delete shares[id];
  }
  setOutput("shareBeforeMute", reconcile(shares));
  if (volumeOwner) writeAccount(volumeOwner, "shareBeforeMute", parseVolumes(shares));
  return target;
}

let masterSeq = 0, masterOkSeq = 0;
let masterOk = 100;
export async function setMasterVolume(percent: number) {
  const v = clampVolume(percent);
  const my = ++masterSeq;
  setOutput({ master: v, message: null });
  try {
    await api.voiceSetMaster(v);
    if (my > masterOkSeq) { masterOk = v; masterOkSeq = my; }
  } catch (e) {
    if (my !== masterSeq) return;
    setOutput({ master: masterOk, message: `总音量未生效：${errorText(e)}` });
  }
}

let gainSeq = 0, gainOkSeq = 0;
let gainOk = 100;
export async function setMicGain(percent: number) {
  const v = clampVolume(percent);
  const my = ++gainSeq;
  setOutput("gain", v);
  try {
    const applied = await api.voiceSetGain(v);
    if (my > gainOkSeq) { gainOk = typeof applied === "number" ? applied : v; gainOkSeq = my; }
    if (my === gainSeq) setOutput("gain", gainOk);
  } catch (e) {
    if (my !== gainSeq) return;
    setOutput("gain", gainOk);
    setMic("message", `麦克风音量未生效：${errorText(e)}`);
  }
}

let switchesLoaded = false;
let switchesTouched = false;
let savedSwitches = "";
createRoot(() => createEffect(() => {
  const muted = mic.muted;
  const deafened = output.deafened;
  const keepMuted = deafened ? (micBeforeDeaf ?? muted) : muted;
  if (!switchesLoaded) return;
  const key = `${keepMuted}:${deafened}`;
  if (key === savedSwitches) return;
  savedSwitches = key;
  void api.setVoiceSwitches(keepMuted, deafened).catch(() => {});
}));

let devicesListening: Promise<UnlistenFn> | null = null;
export function watchDevices() {
  devicesListening ??= listen<{ input: boolean; output: boolean }>("audio://devices", ({ payload }) => {
    if (payload.input && inputs.loaded) void loadInputs();
    if (payload.output && outputs.loaded) void loadOutputs();
  }).catch((e) => {
    devicesListening = null;
    throw e;
  });
  return devicesListening;
}

export async function loadAudioPrefs() {
  void watchDenoiseFallback().catch(() => {});
  void watchDevices().catch(() => {});
  const my = [masterSeq, gainSeq, inSeq, outSeq, denoiseSeq];
  try {
    const p = await api.audioPrefs();
    if (!p) return;
    if (my[0] === masterSeq) { masterOk = clampVolume(p.master); setOutput("master", masterOk); }
    if (my[1] === gainSeq) { gainOk = clampVolume(p.gain); setOutput("gain", gainOk); }
    if (my[2] === inSeq) { inOk = p.input; setInputs("selected", p.input); }
    if (my[3] === outSeq) { outOk = p.output; setOutputs("selected", p.output); }
    if (my[4] === denoiseSeq) {
      setDenoiseState(Boolean(p.denoise));
      setDenoiseModeState(p.denoiseMode === "rnnoise" ? "rnnoise" : "kook");
    }
    if (typeof p.effectVolume === "number") applyEffectPrefs(p.effectVolume, p.effectsOff ?? []);
    if (!switchesTouched && !voiceActive() && !joining) {
      const deaf = Boolean(p.deafened);
      setOutput("deafened", deaf);
      setMic({ muted: deaf || Boolean(p.muted), status: deaf || p.muted ? "muted" : "off" });
      micBeforeDeaf = deaf ? Boolean(p.muted) : null;
    }
    switchesLoaded = true;
  } catch {
  }
}

export function onVoiceEvent(p: VoicePayload) {
  if (joining) {
    if (p.channelId === voice.channelId) early.push(p);
    return;
  }
  if (!voice.generation || p.generation !== voice.generation) return;
  const ev = p.event;
  switch (ev.type) {
    case "state":
      if (ev.phase === "disconnected") {
        setMic(MIC_IDLE);
        setOutput(OUT_IDLE);
        clearSelfSpeaking();
        if (ev.reason === "left") return void setVoice({ ...IDLE });
        if (ev.reason === "password") onPassword(p.channelId);
        setVoice({ phase: "disconnected", reason: ev.reason ?? "lost", message: ev.message ?? null, peers: [], attempt: 0, retryAt: null, quality: null, rtt: null });
        return;
      }
      if (ev.phase === "reconnecting") {
        setMic({ status: "off", level: 0 });
        setOutput({ active: false, speaking: [] });
        clearSelfSpeaking();
        setVoice({
          phase: "reconnecting", reason: null, message: ev.message ?? null, quality: null, rtt: null,
          attempt: ev.attempt ?? voice.attempt + 1, maxAttempts: ev.max ?? voice.maxAttempts,
          retryAt: ev.retry_in_ms !== undefined ? Date.now() + ev.retry_in_ms : null,
        });
        return;
      }
      if (ev.phase === "connected" && voice.phase === "connecting") {
        quietUntil = Date.now() + PEER_QUIET_MS;
        selfJoinPending = true;
        if (voice.me) { selfJoinPending = false; playJoin(voice.me.joinVoice); }
      } else if (ev.phase === "connected") {
        quietUntil = Date.now() + PEER_QUIET_MS;
      }
      setVoice({ phase: ev.phase, reason: null, message: null, retryAt: null, ...(ev.phase === "connected" ? { attempt: 0 } : {}) });
      return;
    case "link":
      setVoice({ quality: ev.quality ?? null, rtt: ev.rtt ?? null });
      return;
    case "joined":
      setVoice({ me: ev.me, peers: ev.peers.filter((x) => x.id !== ev.me.id) });
      if (selfJoinPending) { selfJoinPending = false; playJoin(ev.me.joinVoice); }
      return;
    case "peer_joined":
      if (ev.peer.id === voice.me?.id) return;
      if (peerSoundOk() && !voice.peers.some((x) => x.id === ev.peer.id)) {
        const crowded = voice.peers.length + 2 > PEER_LIMIT;
        if (!crowded || (inRadio() && ev.peer.joinVoice)) playJoin(ev.peer.joinVoice);
      }
      setVoice("peers", (list) => [...list.filter((x) => x.id !== ev.peer.id), ev.peer]);
      return;
    case "peer_left":
      if (peerSoundOk() && voice.peers.some((x) => x.id === ev.id) && voice.peers.length + 1 < PEER_LIMIT && !inRadio()) playEffect(Effect.USER_LEAVE);
      setVoice("peers", (list) => list.filter((x) => x.id !== ev.id));
      setOutput("speaking", (s) => s.filter((x) => x !== ev.id));
      return;
    case "output":
      setOutput({ deafened: ev.deafened, active: ev.active, device: ev.device ?? null, message: ev.message ?? null, ...(ev.deafened ? { speaking: [] } : {}) });
      return;
    case "speaking":
      setOutput("speaking", output.deafened ? [] : ev.ids);
      return;
    case "mic":
      if (ev.status !== "active" || ev.muted) clearSelfSpeaking();
      setMic({ status: ev.status, muted: ev.muted, device: ev.device ?? null, message: ev.message ?? null, ...(ev.status === "active" ? {} : { level: 0 }) });
      return;
    case "mic_level":
      if (mic.status === "active") { setMic("level", ev.level); onSelfLevel(ev.level); }
      return;
    case "peer_changed": {
      const { id, ...rest } = ev.patch;
      const patch = Object.fromEntries(Object.entries(rest).filter(([, v]) => v !== undefined));
      setVoice(produce((s) => {
        if (s.me?.id === id) Object.assign(s.me, patch);
        const peer = s.peers.find((x) => x.id === id);
        if (peer) Object.assign(peer, patch);
      }));
      return;
    }
  }
}

type DeviceList = {
  devices: InputDevice[];
  selected: string | null;
  loading: boolean;
  loaded: boolean;
  error: string | null;
};
const LIST0: DeviceList = { devices: [], selected: null, loading: false, loaded: false, error: null };

export const [inputs, setInputs] = createStore<DeviceList>({ ...LIST0 });
export const [outputs, setOutputs] = createStore<DeviceList>({ ...LIST0 });

let inLoad = 0, outLoad = 0, inSeq = 0, outSeq = 0;

export async function loadInputs() {
  const my = ++inLoad;
  const sel = inSeq;
  setInputs({ loading: true, error: null });
  try {
    const r = await api.audioInputs();
    if (my !== inLoad) return;
    if (sel === inSeq) inOk = r.selected;
    setInputs({ devices: r.devices, loading: false, loaded: true, ...(sel === inSeq ? { selected: r.selected } : {}) });
  } catch (e) {
    if (my === inLoad) setInputs({ loading: false, error: errorText(e) });
  }
}

export async function loadOutputs() {
  const my = ++outLoad;
  const sel = outSeq;
  setOutputs({ loading: true, error: null });
  try {
    const r = await api.audioOutputs();
    if (my !== outLoad) return;
    if (sel === outSeq) outOk = r.selected;
    setOutputs({ devices: r.devices, loading: false, loaded: true, ...(sel === outSeq ? { selected: r.selected } : {}) });
  } catch (e) {
    if (my === outLoad) setOutputs({ loading: false, error: errorText(e) });
  }
}

let inOk: string | null | undefined;
let outOk: string | null | undefined;
let inOkSeq = 0, outOkSeq = 0;

export async function selectInput(id: string | null) {
  const my = ++inSeq;
  if (inOk === undefined) inOk = inputs.selected;
  setInputs({ selected: id, error: null });
  try {
    await api.setAudioInput(id);
    if (my > inOkSeq) { inOk = id; inOkSeq = my; }
    if (my === inSeq && micTest.on) void startMicTest();
  } catch (e) {
    if (my === inSeq) setInputs({ selected: inOk ?? null, error: `麦克风切换失败：${errorText(e)}` });
  }
}

export async function selectOutput(id: string | null) {
  const my = ++outSeq;
  if (outOk === undefined) outOk = outputs.selected;
  setOutputs({ selected: id, error: null });
  try {
    await api.setAudioOutput(id);
    if (my > outOkSeq) { outOk = id; outOkSeq = my; }
    if (my === outSeq && micTest.on) void startMicTest();
  } catch (e) {
    if (my === outSeq) setOutputs({ selected: outOk ?? null, error: `扬声器切换失败：${errorText(e)}` });
  }
}

export type DeviceOption = { id: string; label: string; title: string; isDefault: boolean };

export function deviceOptions(list: DeviceList): DeviceOption[] {
  const count: Record<string, number> = {};
  for (const d of list.devices) count[d.name] = (count[d.name] ?? 0) + 1;
  const seen: Record<string, number> = {};
  return list.devices.map((d) => {
    const n = (seen[d.name] = (seen[d.name] ?? 0) + 1);
    const label = count[d.name] > 1 ? `${d.name} ${n}` : d.name;
    return { id: d.id, label, title: label, isDefault: d.default };
  });
}

export const selectionMissing = (list: DeviceList) =>
  list.loaded && !list.error && list.selected !== null && !list.devices.some((d) => d.id === list.selected);

export const shownSelection = (list: DeviceList) => (selectionMissing(list) ? null : list.selected);

export function defaultName(list: DeviceList): string {
  return list.devices.find((x) => x.default)?.name ?? "系统默认";
}

export const [micTest, setMicTest] = createStore({ on: false, level: 0, error: null as string | null });
let testUnlisten: UnlistenFn | null = null;
let testSeq = 0;

export async function startMicTest() {
  const my = ++testSeq;
  setMicTest({ on: true, level: 0, error: null });
  testUnlisten ??= await listen<{ kind: "level"; level: number } | { kind: "failed"; message: string }>("mic-test://event", (e) => {
    if (!micTest.on) return;
    if (e.payload.kind === "level") setMicTest("level", e.payload.level);
    else setMicTest({ on: false, level: 0, error: e.payload.message });
  });
  try {
    await api.micTest(true);
  } catch (e) {
    if (my === testSeq) setMicTest({ on: false, level: 0, error: errorText(e) });
  }
}

export async function stopMicTest() {
  ++testSeq;
  if (!micTest.on) return;
  setMicTest({ on: false, level: 0 });
  await api.micTest(false).catch(() => {});
}

export const toggleMicTest = () => (micTest.on ? stopMicTest() : startMicTest());
