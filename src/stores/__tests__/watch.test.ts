import { beforeEach, expect, mock, test } from "bun:test";

type Call = { cmd: string; args: any };
const calls: Call[] = [];
let stored: Record<string, unknown> = {};
let pipFails = false;
let holdStart: ((g: number) => void) | null = null;
const handlers = new Map<string, (e: { payload: unknown }) => void>();
mock.module("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: any) => {
    calls.push({ cmd, args });
    if (cmd === "account_state") return Promise.resolve(stored);
    if (cmd === "watch_start") return holdStart ? new Promise((r) => (holdStart = r)) : Promise.resolve(7);
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
mock.module("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setFullscreen: async () => {} }) }));
mock.module("../../lib/screenDecoder", () => ({
  ScreenDecoder: class { attach() {} push() {} close() {} },
}));

mock.module("../session", () => ({ liveOf: () => ({}) }));

const { loadPeerVolumes, resetVoice, setVoice, storeShareVolume } = await import("../voice");
const { forgetAccountCache } = await import("../../lib/accountState");
const { setShareVolume, startWatch, setWatchMode, sourceGone, stopWatch, watch, watchingSelf, selfWatchable } = await import("../watch");
const { setShare } = await import("../share");
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(async () => {
  await stopWatch();
  calls.length = 0;
  stored = {};
  pipFails = false;
  holdStart = null;
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
  closed({ payload: 99 });
  await flush();
  expect(watch.mode).toBe("pip");
  closed({ payload: 7 });
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
