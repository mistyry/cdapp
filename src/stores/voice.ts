import { createStore, produce } from "solid-js/store";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api, errorText, type InputDevice } from "../lib/api";

// 语音会话状态。后端在后台完成连接与重建，经 voice://event 推送事件，事件按会话代次过滤。
// 静音、耳机与成员音量在会话之间保留，退出登录时重置。
// 设备、输入增益与总音量属于本机偏好，由后端保存，退出登录时保留。

export type VoicePeer = {
  id: string; name: string; avatar: string; device: string;
  micOff: boolean; headsetOff: boolean; permissionSay: boolean; serverMute: boolean;
};
export type VoicePatch = Partial<Omit<VoicePeer, "avatar" | "device">> & { id: string };
export type VoicePhase = "idle" | "connecting" | "connected" | "reconnecting" | "disconnected";
export type EndReason = "left" | "replaced" | "exited" | "join_failed" | "lost" | "unauthorized" | "password" | "revoked";

export type LinkQuality = "good" | "fair" | "poor";
export type UpStats = { score: number; loss: number; packets: number; lost: number; bitrate: number };
export type DownStats = { packets: number; lost: number; late: number; overflow: number; underruns: number; depthMs: number };

export type VoiceEvent =
  | { type: "state"; phase: "connecting" | "connected" | "reconnecting" | "disconnected"; reason?: EndReason; message?: string;
      // 重连时附带当前次数、上限与等待时长。
      attempt?: number; max?: number; retry_in_ms?: number }
  | { type: "joined"; me: VoicePeer; peers: VoicePeer[] }
  | { type: "peer_joined"; peer: VoicePeer }
  | { type: "peer_left"; id: string }
  | { type: "peer_changed"; patch: VoicePatch }
  | { type: "mic"; status: MicStatus; muted: boolean; device?: string; message?: string }
  | { type: "mic_level"; level: number }
  | { type: "output"; deafened: boolean; active: boolean; device?: string; message?: string }
  | { type: "speaking"; ids: string[] }
  | { type: "link"; quality?: LinkQuality; up?: UpStats; down: DownStats };
// 麦克风状态。denied、no_device、device_lost 与 error 为失败状态，
// 取消静音即重试。
export type MicStatus = "off" | "starting" | "active" | "muted" | "denied" | "no_device" | "device_lost" | "error";
export type VoicePayload = { generation: number; channelId: string; event: VoiceEvent };

type State = {
  // 当前或最近一次加入的频道，断开后保留以便重新加入。
  channelId: string;
  generation: number;
  phase: VoicePhase;
  reason: EndReason | null;
  // 重连或断开的原因。
  message: string | null;
  me: VoicePeer | null;
  // 其他成员，不含本人。
  peers: VoicePeer[];
  // 重连次数、上限与下次重试时间。
  attempt: number;
  maxAttempts: number;
  retryAt: number | null;
  // 链路质量，未连接时为 null。
  quality: LinkQuality | null;
};

const IDLE: State = { channelId: "", generation: 0, phase: "idle", reason: null, message: null, me: null, peers: [], attempt: 0, maxAttempts: 0, retryAt: null, quality: null };

export const [voice, setVoice] = createStore<State>({ ...IDLE });

type Mic = {
  // 用户选择的静音状态，加入前设置也会保留。
  muted: boolean;
  status: MicStatus;
  device: string | null;
  // 麦克风失败或操作失败的说明。
  message: string | null;
  // 输入电平，范围 0 至 100。
  level: number;
  // 请求在途，按钮暂不可用。
  busy: boolean;
};

const MIC_IDLE = { status: "off" as MicStatus, device: null, message: null, level: 0, busy: false };
export const [mic, setMic] = createStore<Mic>({ muted: false, ...MIC_IDLE });

type Out = {
  // 用户选择的耳机关闭状态。
  deafened: boolean;
  // 播放设备已打开。
  active: boolean;
  device: string | null;
  message: string | null;
  busy: boolean;
  // 正在说话的成员 ID。
  speaking: string[];
  // 成员音量百分比，100 不记录。
  volumes: Record<string, number>;
  master: number;
  // 麦克风输入增益百分比。
  gain: number;
};
const OUT_IDLE = { active: false, device: null, message: null, busy: false, speaking: [] as string[] };
export const [output, setOutput] = createStore<Out>({ deafened: false, ...OUT_IDLE, volumes: {}, master: 100, gain: 100 });
export const isSpeaking = (id: string) => output.speaking.includes(id);
export const volumeOf = (id: string) => output.volumes[id] ?? 100;

// 请求序号。加入与退出各取一个，后端据此丢弃被取代的加入，前端据此丢弃迟到的结果。
// 以时间为初值，重新加载页面后仍大于后端已处理的序号。
let seq = Date.now();
// 加入命令返回代次之前收到的事件，代次确定后按代次过滤再处理。
let early: VoicePayload[] = [];
let joining = false;
// 加入期间修改的静音状态，加入完成后发送。
let wanted: boolean | null = null;
// 加入期间修改的耳机状态，加入完成后发送。
let wantedDeaf: boolean | null = null;
// 未连接时关闭耳机前的麦克风静音状态，打开耳机时恢复。
let micBeforeDeaf: boolean | null = null;

export const voiceActive = () => voice.phase === "connecting" || voice.phase === "connected" || voice.phase === "reconnecting";

const REASONS: Record<EndReason, string> = {
  left: "已退出语音",
  replaced: "已在其他设备加入语音",
  exited: "语音已结束",
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

// 加入语音频道。已在该频道时不重复加入，在其他频道时由后端先退出旧频道。
// 权限由调用方（session/access.ts 的 enterVoice）先行判断；password 只用于本次请求。
export async function joinVoice(channelId: string, password: string | null = null) {
  if (voiceActive() && voice.channelId === channelId) return;
  const my = ++seq;
  joining = true;
  wanted = null;
  wantedDeaf = null;
  early = [];
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
    // 加入时已发送的状态不重复发送。
    if (d !== null && d !== sentDeaf) await setDeafened(d);
    if (w !== null && w !== sent) await setMuted(w);
  } catch (e) {
    if (my !== seq) return;
    joining = false;
    early = [];
    setVoice({ phase: "disconnected", reason: "join_failed", message: errorText(e) });
  }
}

// 退出语音。界面立即复位，不等待后端释放完成。
export async function leaveVoice() {
  const my = ++seq;
  joining = false;
  wanted = null;
  wantedDeaf = null;
  early = [];
  setVoice({ ...IDLE });
  setMic(MIC_IDLE);
  setOutput(OUT_IDLE);
  await api.voiceLeave(my).catch(() => {});
}

// 需要频道密码时的处理，由会话层注册。
let onPassword: (channelId: string) => void = () => {};
export const setPasswordHandler = (f: (channelId: string) => void) => (onPassword = f);

// 权限撤销：结束当前会话并说明原因，不提供重新加入。
// 先同步写入结束状态再等待后端，期间新的加入可以正常覆盖。
export async function revokeVoice() {
  const p = leaveVoice();
  setVoice({ channelId: "", phase: "disconnected", reason: "revoked", message: null });
  await p;
}

// 连接中断、正常结束或加入失败后可以重新加入。被替换与登录失效时不提供。
export const canRejoin = () =>
  voice.phase === "disconnected" && !!voice.channelId && (voice.reason === "lost" || voice.reason === "exited" || voice.reason === "join_failed");

// 以当前的静音与耳机状态重新加入原频道。
export function rejoinVoice() {
  if (canRejoin()) return joinVoice(voice.channelId);
}

// 系统唤醒或网络恢复时通知后端。已连接时也通知，媒体可能已断开而阶段仍为 connected。
// 未加入、已断开或加入尚未返回时不通知。
export function voiceNetworkBack() {
  if ((voice.phase === "reconnecting" || voice.phase === "connected") && voice.generation) api.voiceWake().catch(() => {});
}

// 设备摘要中的麦克风状态。
const MIC_SHORT: Partial<Record<MicStatus, string>> = {
  muted: "已静音", off: "未开启", starting: "正在启动", denied: "无权限", no_device: "无设备", device_lost: "已断开", error: "出错",
};
export function deviceSummary(): { text: string; title: string; ok: boolean } {
  const micOk = mic.status === "active" || mic.status === "muted" || mic.status === "starting";
  const micPart = mic.status === "active" ? mic.device ?? "麦克风" : MIC_SHORT[mic.status] ?? "";
  const outOk = output.deafened || output.active;
  const outPart = output.deafened ? "已关闭" : output.active ? output.device ?? "扬声器" : "未开启";
  // 麦克风失败的说明已单独显示，这里只附加设备切换等提示。
  const notes = [mic.status === "active" ? mic.message : null, output.message].filter(Boolean);
  return {
    text: `麦克风：${micPart} 扬声器：${outPart}`,
    title: [`麦克风：${micText()}${mic.device ? `，${mic.device}` : ""}`, `扬声器：${outPart}`, ...notes].join("\n"),
    ok: micOk && outOk,
  };
}

const QUALITY_TEXT: Record<LinkQuality, string> = { good: "良好", fair: "一般", poor: "较差" };
export const qualityText = () => (voice.quality ? QUALITY_TEXT[voice.quality] : null);

// 关闭断开提示。
export function dismissVoice() {
  if (voice.phase === "disconnected") setVoice({ ...IDLE });
}

// 退出登录时重置全部语音状态与偏好。
export function resetVoice() {
  seq++;
  joining = false;
  wanted = null;
  wantedDeaf = null;
  early = [];
  setVoice({ ...IDLE });
  setMic({ muted: false, ...MIC_IDLE });
  setOutput({ deafened: false, ...OUT_IDLE, volumes: {} });
  micBeforeDeaf = null;
}

// 麦克风处于失败状态，取消静音即重试。
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

// 麦克风失败时的处理建议。
export function micHint(): string | null {
  switch (mic.status) {
    case "denied": return "请在系统设置的隐私与安全性中允许 cdapp 使用麦克风，然后点击重试";
    case "no_device": return "请连接麦克风后点击重试";
    case "device_lost": return mic.message ?? "麦克风已断开，请检查设备后点击重试";
    case "error": return mic.message;
    default: return null;
  }
}

// 设置麦克风静音。未连接时只更新本地状态，加入时随请求发送。
export async function setMuted(muted: boolean) {
  const generation = voice.generation;
  if (!voiceActive() || !generation || joining) {
    if (joining) wanted = muted;
    setMic({ muted, status: muted ? "muted" : "off", message: null });
    // 取消静音时同时打开耳机，与后端一致。
    if (!muted && output.deafened) setOutput("deafened", false);
    return;
  }
  setMic({ muted, busy: true, message: null, ...(muted ? { level: 0 } : {}) });
  try {
    await api.voiceSetMic(generation, muted);
  } catch (e) {
    if (voice.generation !== generation) return;
    // 静音状态以后端随后的事件为准。
    setMic({ message: errorText(e) });
  } finally {
    if (voice.generation === generation) setMic("busy", false);
  }
}

export const toggleMic = () => setMuted(micFailed() ? false : !mic.muted);

// 设置耳机关闭。关闭时同时静音麦克风，打开时恢复之前的静音状态。
// 未连接时只更新本地状态，加入时随请求发送。
export async function setDeafened(deafened: boolean) {
  const generation = voice.generation;
  if (!voiceActive() || !generation || joining) {
    if (joining) wantedDeaf = deafened;
    if (deafened === output.deafened) return;
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
  } catch (e) {
    if (voice.generation !== generation) return;
    setOutput({ message: errorText(e) });
  } finally {
    if (voice.generation === generation) setOutput("busy", false);
  }
}

export const toggleDeafen = () => setDeafened(!output.deafened);

// 设置成员音量，范围 0 至 200。
export async function setPeerVolume(id: string, percent: number) {
  const v = Math.max(0, Math.min(200, Math.round(percent)));
  setOutput("volumes", id, v);
  await api.voiceSetVolume(id, v).catch((e) => setOutput({ message: errorText(e) }));
}

export const clampVolume = (percent: number) => Math.max(0, Math.min(200, Math.round(Number.isFinite(percent) ? percent : 100)));

// 总音量与输入增益：连续拖动只采用最新一次的结果，失败时恢复后端最后确认的值。
// 后端确认的值只由更新的成功请求覆盖：较早请求成功后被较新请求的失败回滚时，恢复到该较早值。
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

// 启动时读取后端保存的本机音频偏好。读取失败时保持默认值。
export async function loadAudioPrefs() {
  const my = [masterSeq, gainSeq, inSeq, outSeq];
  try {
    const p = await api.audioPrefs();
    if (!p) return;
    // 读取期间用户已修改的项以用户为准。
    if (my[0] === masterSeq) { masterOk = clampVolume(p.master); setOutput("master", masterOk); }
    if (my[1] === gainSeq) { gainOk = clampVolume(p.gain); setOutput("gain", gainOk); }
    if (my[2] === inSeq) { inOk = p.input; setInputs("selected", p.input); }
    if (my[3] === outSeq) { outOk = p.output; setOutputs("selected", p.output); }
  } catch {
    // 使用默认值。
  }
}

export function onVoiceEvent(p: VoicePayload) {
  if (joining) {
    // 代次确定前暂存本频道的事件。
    if (p.channelId === voice.channelId) early.push(p);
    return;
  }
  if (!voice.generation || p.generation !== voice.generation) return;
  const ev = p.event;
  switch (ev.type) {
    case "state":
      if (ev.phase === "disconnected") {
        // 会话结束时清除设备状态，静音与耳机偏好保留。
        setMic(MIC_IDLE);
        setOutput(OUT_IDLE);
        if (ev.reason === "left") return void setVoice({ ...IDLE });
        if (ev.reason === "password") onPassword(p.channelId);
        setVoice({ phase: "disconnected", reason: ev.reason ?? "lost", message: ev.message ?? null, peers: [], attempt: 0, retryAt: null, quality: null });
        return;
      }
      if (ev.phase === "reconnecting") {
        setMic({ status: "off", level: 0 });
        setOutput({ active: false, speaking: [] });
        setVoice({
          phase: "reconnecting", reason: null, message: ev.message ?? null, quality: null,
          attempt: ev.attempt ?? voice.attempt + 1, maxAttempts: ev.max ?? voice.maxAttempts,
          retryAt: ev.retry_in_ms !== undefined ? Date.now() + ev.retry_in_ms : null,
        });
        return;
      }
      // 连接成功后重置重连次数。
      setVoice({ phase: ev.phase, reason: null, message: null, retryAt: null, ...(ev.phase === "connected" ? { attempt: 0 } : {}) });
      return;
    case "link":
      setVoice("quality", ev.quality ?? null);
      return;
    case "joined":
      // 成员列表不含本人。
      setVoice({ me: ev.me, peers: ev.peers.filter((x) => x.id !== ev.me.id) });
      return;
    case "peer_joined":
      if (ev.peer.id === voice.me?.id) return;
      setVoice("peers", (list) => [...list.filter((x) => x.id !== ev.peer.id), ev.peer]);
      return;
    case "peer_left":
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
      setMic({ status: ev.status, muted: ev.muted, device: ev.device ?? null, message: ev.message ?? null, ...(ev.status === "active" ? {} : { level: 0 }) });
      return;
    case "mic_level":
      if (mic.status === "active") setMic("level", ev.level);
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
  // 期望的选择，null 为跟随系统默认。
  selected: string | null;
  loading: boolean;
  // 已读取过一次列表。
  loaded: boolean;
  error: string | null;
};
const LIST0: DeviceList = { devices: [], selected: null, loading: false, loaded: false, error: null };

// 输入设备列表。
export const [inputs, setInputs] = createStore<DeviceList>({ ...LIST0 });
// 输出设备列表。
export const [outputs, setOutputs] = createStore<DeviceList>({ ...LIST0 });

// 列表请求与选择各自按序号只采用最新结果。
let inLoad = 0, outLoad = 0, inSeq = 0, outSeq = 0;

export async function loadInputs() {
  const my = ++inLoad;
  const sel = inSeq;
  setInputs({ loading: true, error: null });
  try {
    const r = await api.audioInputs();
    if (my !== inLoad) return;
    // 列表在途期间用户已重新选择时，保留新的选择。
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

// 后端最后确认的选择，失败时恢复。
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
  } catch (e) {
    if (my === outSeq) setOutputs({ selected: outOk ?? null, error: `扬声器切换失败：${errorText(e)}` });
  }
}

export type DeviceOption = { id: string; label: string; title: string; isDefault: boolean };

// 设备选项。同名设备追加序号区分；选择以设备 ID 为准。
export function deviceOptions(list: DeviceList): DeviceOption[] {
  const count: Record<string, number> = {};
  for (const d of list.devices) count[d.name] = (count[d.name] ?? 0) + 1;
  const seen: Record<string, number> = {};
  return list.devices.map((d) => {
    const n = (seen[d.name] = (seen[d.name] ?? 0) + 1);
    const label = count[d.name] > 1 ? `${d.name} (${n})` : d.name;
    return { id: d.id, label, title: d.default ? `${label}，当前系统默认` : label, isDefault: d.default };
  });
}

// 所选设备已不在列表中。只在列表读取成功后判断。
export const selectionMissing = (list: DeviceList) =>
  list.loaded && !list.error && list.selected !== null && !list.devices.some((d) => d.id === list.selected);

// 系统默认项的说明，附带当前默认设备名称。
export function defaultLabel(list: DeviceList): string {
  const d = list.devices.find((x) => x.default);
  return d ? `系统默认 (${d.name})` : "系统默认";
}
