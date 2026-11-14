import { beforeEach, expect, jest, mock, test } from "bun:test";

type Call = { cmd: string; args: any };
const calls: Call[] = [];
let stored: Record<string, unknown> = {};
let pipFails = false;
let holdStart: ((g: number) => void) | null = null;
let stopGate: Promise<void> | null = null;
let fullscreenGate: Promise<void> | null = null;
let pipCloseGate: Promise<void> | null = null;
const pushes: ArrayBuffer[] = [];
const attached: (HTMLCanvasElement | null)[] = [];
const handlers = new Map<string, (e: { payload: unknown }) => void>();
mock.module("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: any) => {
    calls.push({ cmd, args });
    if (cmd === "account_state") return Promise.resolve(stored);
    if (cmd === "watch_start") return holdStart ? new Promise((r) => (holdStart = r)) : Promise.resolve(7);
    if (cmd === "watch_stop" && stopGate) return stopGate.then(() => null);
    if (cmd === "watch_pip_close" && pipCloseGate) return pipCloseGate.then(() => null);
    if (cmd === "watch_pip_open" && pipFails) return Promise.reject(new Error("pip"));
    return Promise.resolve(null);
  },
  Channel: class { onmessage: ((b: unknown) => void) | null = null; },
}));
mock.module("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    handlers.set(name, cb);
    return () => {};
  },
  emit: async () => {},
}));
mock.module("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setFullscreen: async () => { if (fullscreenGate) await fullscreenGate; } }),
}));
mock.module("../../lib/screenDecoder", () => ({
  ScreenDecoder: class {
    attach(c: HTMLCanvasElement | null) { attached.push(c); }
    push(b: ArrayBuffer) { pushes.push(b); }
    close() {}
  },
}));

mock.module("../session", () => ({ liveOf: () => ({}) }));

const { loadPeerVolumes, resetVoice, setVoice, storeShareVolume } = await import("../voice");
const { forgetAccountCache } = await import("../../lib/accountState");
const { setShareVolume, startWatch, setWatchMode, sourceGone, stopWatch, refreshWatch, refreshBusy, PIP_READY_TIMEOUT, watch, watchingSelf, selfWatchable, attachCanvas, detachCanvas, watchFromList } = await import("../watch");
const { setUi, ui, loadStageCollapsed, setStageCollapsed, resetUi } = await import("../ui");
const { setShare, loadShareAudio, setShareAudio, shareAudio, resetShareAudio } = await import("../share");
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(async () => {
  await stopWatch();
  calls.length = 0;
  stored = {};
  pipFails = false;
  holdStart = null;
  stopGate = null;
  fullscreenGate = null;
  pipCloseGate = null;
  forgetAccountCache();
  resetVoice();
  setVoice({ channelId: "c1", phase: "connected" });
  setShare({ phase: "idle", channelId: "", watchable: false });
});

const me = { id: "me", name: "我自己", avatar: "", micOff: false, serverMute: false } as any;

test("自己共享且可观看时才算可观看自己", () => {
  setVoice({ me });
  expect(selfWatchable()).toBe(false);
  setShare({ phase: "live", channelId: "c1", watchable: false });
  expect(selfWatchable()).toBe(false);
  setShare({ watchable: true });
  expect(selfWatchable()).toBe(true);
  setShare({ channelId: "c2" });
  expect(selfWatchable()).toBe(false);
});

test("结束共享时观看自己的来源失效", async () => {
  setVoice({ me });
  setShare({ phase: "live", channelId: "c1", watchable: true });
  await startWatch("c1", "me");
  expect(watchingSelf()).toBe(true);
  expect(calls.find((c) => c.cmd === "watch_start")!.args.liver).toBe("me");
  expect(sourceGone()).toBe(false);
  setShare({ phase: "idle", watchable: false });
  expect(sourceGone()).toBe(true);
});

test("后端结束观看自己时前端回到空闲", async () => {
  setVoice({ me });
  setShare({ phase: "live", channelId: "c1", watchable: true });
  await startWatch("c1", "me");
  expect(watchingSelf()).toBe(true);
  handlers.get("watch://event")!({ payload: { generation: 7, state: { kind: "ended", message: null } } });
  await flush();
  expect(watch.phase).toBe("idle");
  expect(watchingSelf()).toBe(false);
});

test("观看他人时自己结束共享不影响观看", async () => {
  setVoice({ me });
  setShare({ phase: "live", channelId: "c1", watchable: true });
  await startWatch("c1", "u8");
  setShare({ phase: "idle", watchable: false });
  expect(sourceGone()).toBe(false);
});

test("观看自己与观看他人之间切换会先停止上一路", async () => {
  setVoice({ me });
  setShare({ phase: "live", channelId: "c1", watchable: true });
  await startWatch("c1", "u8");
  expect(watchingSelf()).toBe(false);
  calls.length = 0;
  await startWatch("c1", "me");
  expect(calls.map((c) => c.cmd).filter((c) => c.startsWith("watch_"))).toEqual(["watch_stop", "watch_start"]);
  expect(watchingSelf()).toBe(true);
  calls.length = 0;
  await startWatch("c1", "u8");
  expect(calls.map((c) => c.cmd).filter((c) => c.startsWith("watch_"))).toEqual(["watch_stop", "watch_start"]);
  expect(watch.userId).toBe("u8");
});

test("开始观看时把该分享者保存的音量传给后端", async () => {
  stored = { shareVolumes: { u8: 30 } };
  await loadPeerVolumes("me");
  await startWatch("c1", "u8");
  const start = calls.find((c) => c.cmd === "watch_start")!;
  expect(start.args.volume).toBe(30);
  expect(calls.some((c) => c.cmd === "watch_volume")).toBe(false);
  await stopWatch();
  calls.length = 0;
  storeShareVolume("u9", 150);
  await startWatch("c1", "u9");
  expect(calls.find((c) => c.cmd === "watch_start")!.args.volume).toBe(150);
});

test("开始观看返回前调整的音量在拿到观看编号后补发", async () => {
  holdStart = () => {};
  const started = startWatch("c1", "u8");
  await flush();
  await setShareVolume("u8", 40);
  expect(calls.some((c) => c.cmd === "watch_volume")).toBe(false);
  holdStart(7);
  await started;
  const v = calls.find((c) => c.cmd === "watch_volume")!;
  expect(v.args).toEqual({ generation: 7, percent: 40 });
});

test("外部悬浮窗打开失败时回到原方式", async () => {
  await startWatch("c1", "u8");
  pipFails = true;
  await setWatchMode("pip");
  expect(watch.mode).toBe("stage");
});

test("外部悬浮窗被关闭时回到 stage", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("pip");
  expect(watch.mode).toBe("pip");
  const closed = handlers.get("watch://pip-closed")!;
  const seq = calls.find((c) => c.cmd === "watch_pip_open")!.args.seq;
  closed({ payload: { generation: 99, seq } });
  await flush();
  expect(watch.mode).toBe("pip");
  closed({ payload: { generation: 7, seq: seq - 1 } });
  await flush();
  expect(watch.mode).toBe("pip");
  closed({ payload: { generation: 7, seq } });
  await flush();
  expect(watch.mode).toBe("stage");
  expect(calls.some((c) => c.cmd === "watch_redirect")).toBe(true);
});

test("分享者最小化时进入暂停并在恢复后退出", async () => {
  await startWatch("c1", "u8");
  const ev = handlers.get("watch://event")!;
  expect(watch.paused).toBe(false);
  ev({ payload: { generation: 7, state: { kind: "paused", paused: true } } });
  expect(watch.paused).toBe(true);
  ev({ payload: { generation: 6, state: { kind: "paused", paused: false } } });
  expect(watch.paused).toBe(true);
  ev({ payload: { generation: 7, state: { kind: "paused", paused: false } } });
  expect(watch.paused).toBe(false);
});

test("停止观看时清除暂停状态", async () => {
  await startWatch("c1", "u8");
  handlers.get("watch://event")!({ payload: { generation: 7, state: { kind: "paused", paused: true } } });
  expect(watch.paused).toBe(true);
  await stopWatch();
  expect(watch.paused).toBe(false);
});

test("等待中收到画面后回到播放", async () => {
  await startWatch("c1", "u8");
  const ev = handlers.get("watch://event")!;
  ev({ payload: { generation: 7, state: { kind: "playing" } } });
  ev({ payload: { generation: 7, state: { kind: "waiting" } } });
  expect(watch.phase).toBe("waiting");
  ev({ payload: { generation: 7, state: { kind: "playing" } } });
  expect(watch.phase).toBe("playing");
});

const watchCmds = () => calls.map((c) => c.cmd).filter((c) => c.startsWith("watch_"));
const lastPipSeq = () => {
  const opens = calls.filter((c) => c.cmd === "watch_pip_open");
  return opens[opens.length - 1]!.args.seq as number;
};

test("保持观看时立即返回观看不发送后端命令", async () => {
  await startWatch("c1", "u8");
  calls.length = 0;
  const keeping = setWatchMode("kept");
  const back = startWatch("c1", "u8");
  await keeping;
  await back;
  await flush();
  expect(watchCmds()).toEqual([]);
  expect(watch.mode).toBe("stage");
});

test("旧悬浮窗的关闭与就绪不影响新打开的悬浮窗", async () => {
  await startWatch("c1", "u8");
  jest.useFakeTimers();
  try {
    await setWatchMode("pip");
    const first = lastPipSeq();
    await setWatchMode("stage");
    await setWatchMode("pip");
    const second = lastPipSeq();
    expect(second).toBeGreaterThan(first);
    handlers.get("watch://pip-closed")!({ payload: { generation: 7, seq: first } });
    handlers.get("watch://pip")!({ payload: { generation: 7, seq: first, action: "ready" } });
    handlers.get("watch://pip")!({ payload: { generation: 7, seq: first, action: "back" } });
    await Promise.resolve();
    expect(watch.mode).toBe("pip");
    calls.length = 0;
    jest.advanceTimersByTime(PIP_READY_TIMEOUT);
    await Promise.resolve();
    await Promise.resolve();
    expect(watch.mode).toBe("stage");
    expect(watchCmds()[0]).toBe("watch_pip_close");
  } finally {
    jest.useRealTimers();
  }
  await flush();
});

test("悬浮窗返回消息回到 stage 并改发画面", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("pip");
  const seq = lastPipSeq();
  calls.length = 0;
  handlers.get("watch://pip")!({ payload: { generation: 7, seq, action: "back" } });
  await flush();
  expect(watch.mode).toBe("stage");
  expect(watchCmds()).toEqual(["watch_pip_close", "watch_redirect"]);
});

test("悬浮窗存在时再次观看同一人等同于返回", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("pip");
  calls.length = 0;
  await startWatch("c1", "u8");
  expect(watch.mode).toBe("stage");
  expect(watchCmds()).toEqual(["watch_pip_close", "watch_redirect"]);
});

test("返回列表后保持观看并可返回观看", async () => {
  await startWatch("c1", "u8");
  calls.length = 0;
  await setWatchMode("kept");
  expect(watch.mode).toBe("kept");
  expect(watch.phase).not.toBe("idle");
  expect(watchCmds()).toEqual([]);
  await startWatch("c1", "u8");
  expect(watch.mode).toBe("stage");
  expect(watchCmds()).toEqual([]);
});

test("保持观看期间画面继续解码并画到卡片画布", async () => {
  pushes.length = 0;
  attached.length = 0;
  await startWatch("c1", "u8");
  const ch = calls.find((c) => c.cmd === "watch_start")!.args.frames;
  const stageCanvas = { id: "stage" } as unknown as HTMLCanvasElement;
  const cardCanvas = { id: "card" } as unknown as HTMLCanvasElement;
  attachCanvas(stageCanvas);
  await setWatchMode("kept");
  attachCanvas(cardCanvas);
  detachCanvas(stageCanvas);
  ch.onmessage(new ArrayBuffer(4));
  expect(pushes.length).toBe(1);
  expect(attached[attached.length - 1]).toBe(cardCanvas);
  await startWatch("c1", "u8");
  attachCanvas(stageCanvas);
  detachCanvas(cardCanvas);
  ch.onmessage(new ArrayBuffer(4));
  expect(pushes.length).toBe(2);
  expect(attached[attached.length - 1]).toBe(stageCanvas);
  detachCanvas(stageCanvas);
  expect(attached[attached.length - 1]).toBe(null);
});

test("从成员列表观看会展开 stage 并开始观看", async () => {
  setUi("stageCollapsed", true);
  await watchFromList("u8");
  expect(ui.stageCollapsed).toBe(false);
  expect(watch.userId).toBe("u8");
  expect(watch.channelId).toBe("c1");
  await setWatchMode("kept");
  setUi("stageCollapsed", true);
  calls.length = 0;
  await watchFromList("u8");
  expect(ui.stageCollapsed).toBe(false);
  expect(watch.mode).toBe("stage");
  expect(watchCmds()).toEqual([]);
});

test("从悬浮窗以外的方式回到画面时不发送保持", async () => {
  await startWatch("c1", "u8");
  calls.length = 0;
  await setWatchMode("app");
  await setWatchMode("stage");
  expect(watchCmds()).toEqual([]);
});

test("保持观看时观看他人会先结束当前观看", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("kept");
  calls.length = 0;
  await startWatch("c1", "u9");
  expect(watchCmds()).toEqual(["watch_stop", "watch_start"]);
  expect(watch.userId).toBe("u9");
  expect(watch.mode).toBe("stage");
});

test("保持观看时对方结束共享会回到空闲", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("kept");
  handlers.get("watch://event")!({ payload: { generation: 7, state: { kind: "ended", message: null } } });
  await flush();
  expect(watch.phase).toBe("idle");
  expect(watch.mode).toBe("stage");
});

test("拿到观看编号前不能进入保持或悬浮窗", async () => {
  holdStart = () => {};
  const started = startWatch("c1", "u8");
  await flush();
  await setWatchMode("kept");
  await setWatchMode("pip");
  expect(watch.mode).toBe("stage");
  holdStart(7);
  await started;
});

test("刷新会停止后重新观看同一人并沿用音量", async () => {
  storeShareVolume("u8", 60);
  await startWatch("c1", "u8");
  handlers.get("watch://event")!({ payload: { generation: 7, state: { kind: "playing" } } });
  calls.length = 0;
  await refreshWatch();
  expect(watchCmds()).toEqual(["watch_stop", "watch_start"]);
  const start = calls.find((c) => c.cmd === "watch_start")!;
  expect(start.args.liver).toBe("u8");
  expect(start.args.channelId).toBe("c1");
  expect(start.args.volume).toBe(60);
  expect(watch.userId).toBe("u8");
  expect(watch.mode).toBe("stage");
});

test("连接中与刷新进行中不能再次刷新", async () => {
  await startWatch("c1", "u8");
  expect(refreshBusy()).toBe(true);
  handlers.get("watch://event")!({ payload: { generation: 7, state: { kind: "playing" } } });
  expect(refreshBusy()).toBe(false);
  holdStart = () => {};
  const first = refreshWatch();
  await flush();
  expect(refreshBusy()).toBe(true);
  calls.length = 0;
  await refreshWatch();
  expect(watchCmds()).toEqual([]);
  holdStart(8);
  await first;
  handlers.get("watch://event")!({ payload: { generation: 8, state: { kind: "playing" } } });
  expect(refreshBusy()).toBe(false);
});

test("停止与开始观看按调用顺序到达后端", async () => {
  await startWatch("c1", "u8");
  let release!: () => void;
  stopGate = new Promise<void>((r) => (release = r));
  calls.length = 0;
  const stopping = stopWatch();
  const starting = startWatch("c1", "u9");
  await flush();
  expect(watchCmds()).toEqual(["watch_stop"]);
  release();
  await stopping;
  await starting;
  expect(watchCmds()).toEqual(["watch_stop", "watch_start"]);
  expect(watch.userId).toBe("u9");
});

test("退出全屏进行中立即返回观看时不发送后端命令", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("screen");
  let release!: () => void;
  fullscreenGate = new Promise<void>((r) => (release = r));
  calls.length = 0;
  const keeping = setWatchMode("kept");
  const back = startWatch("c1", "u8");
  await flush();
  release();
  await keeping;
  await back;
  expect(watchCmds()).toEqual([]);
  expect(watch.mode).toBe("stage");
});

test("关闭悬浮窗进行中立即切到保持时命令按顺序到达后端", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("pip");
  let release!: () => void;
  pipCloseGate = new Promise<void>((r) => (release = r));
  calls.length = 0;
  const back = setWatchMode("stage");
  const keeping = setWatchMode("kept");
  await flush();
  expect(watchCmds()).toEqual(["watch_pip_close"]);
  release();
  await back;
  await keeping;
  expect(watchCmds()).toEqual(["watch_pip_close", "watch_redirect"]);
  expect(watch.mode).toBe("kept");
});

test("从保持直接打开悬浮窗再返回会改发画面", async () => {
  await startWatch("c1", "u8");
  await setWatchMode("kept");
  calls.length = 0;
  await setWatchMode("pip");
  expect(watchCmds()).toEqual(["watch_pip_open"]);
  await setWatchMode("stage");
  expect(watchCmds()).toEqual(["watch_pip_open", "watch_pip_close", "watch_redirect"]);
});

test("快速重开悬浮窗时旧序号的消息被忽略", async () => {
  await startWatch("c1", "u8");
  const opening = setWatchMode("pip");
  const back = setWatchMode("stage");
  const reopening = setWatchMode("pip");
  await Promise.all([opening, back, reopening]);
  const opens = calls.filter((c) => c.cmd === "watch_pip_open").map((c) => c.args.seq as number);
  const [first, second] = opens;
  expect(second).toBeGreaterThan(first!);
  expect(watchCmds().slice(-4)).toEqual(["watch_pip_open", "watch_pip_close", "watch_redirect", "watch_pip_open"]);
  handlers.get("watch://pip-closed")!({ payload: { generation: 7, seq: first } });
  handlers.get("watch://pip")!({ payload: { generation: 7, seq: first, action: "back" } });
  await flush();
  expect(watch.mode).toBe("pip");
  handlers.get("watch://pip")!({ payload: { generation: 7, seq: second, action: "back" } });
  await flush();
  expect(watch.mode).toBe("stage");
});

test("悬浮窗未就绪时超时回到 stage", async () => {
  await startWatch("c1", "u8");
  jest.useFakeTimers();
  try {
    await setWatchMode("pip");
    expect(watch.mode).toBe("pip");
    calls.length = 0;
    jest.advanceTimersByTime(PIP_READY_TIMEOUT);
    await Promise.resolve();
    await Promise.resolve();
    expect(watch.mode).toBe("stage");
    expect(watchCmds()[0]).toBe("watch_pip_close");
  } finally {
    jest.useRealTimers();
  }
  await flush();
});

test("悬浮窗就绪后不再超时", async () => {
  await startWatch("c1", "u8");
  jest.useFakeTimers();
  try {
    await setWatchMode("pip");
    handlers.get("watch://pip")!({ payload: { generation: 7, seq: lastPipSeq(), action: "ready" } });
    jest.advanceTimersByTime(PIP_READY_TIMEOUT * 2);
    await Promise.resolve();
    expect(watch.mode).toBe("pip");
  } finally {
    jest.useRealTimers();
  }
});

test("共享声音开关按账号保存并在登录时读回", async () => {
  stored = { shareAudio: { screen: true, window: "x" } };
  await loadShareAudio("me");
  expect({ ...shareAudio }).toEqual({ screen: true, window: false });
  calls.length = 0;
  setShareAudio("window", true);
  await flush();
  const w = calls.find((c) => c.cmd === "set_account_state")!;
  expect(w.args).toEqual({ user: "me", key: "shareAudio", value: { screen: true, window: true } });
  resetShareAudio();
  expect({ ...shareAudio }).toEqual({ screen: false, window: false });
});

test("stage 折叠状态按账号保存、登出复位、损坏值默认展开", async () => {
  forgetAccountCache();
  stored = { stageCollapsed: true };
  await loadStageCollapsed("me");
  expect(ui.stageCollapsed).toBe(true);
  calls.length = 0;
  setStageCollapsed(false);
  await flush();
  expect(calls.find((c) => c.cmd === "set_account_state")!.args).toEqual({ user: "me", key: "stageCollapsed", value: false });
  setStageCollapsed(true);
  resetUi();
  expect(ui.stageCollapsed).toBe(false);
  calls.length = 0;
  setStageCollapsed(true);
  await watchFromList("u8");
  await flush();
  expect(calls.some((c) => c.cmd === "set_account_state")).toBe(false);
  forgetAccountCache();
  stored = { stageCollapsed: "yes" };
  setUi("stageCollapsed", true);
  await loadStageCollapsed("me2");
  expect(ui.stageCollapsed).toBe(false);
  calls.length = 0;
  setStageCollapsed(true);
  await watchFromList("u8");
  await flush();
  expect(calls.filter((c) => c.cmd === "set_account_state").map((c) => c.args)).toEqual([
    { user: "me2", key: "stageCollapsed", value: true },
    { user: "me2", key: "stageCollapsed", value: false },
  ]);
  resetUi();
});
