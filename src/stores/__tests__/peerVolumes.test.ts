import { beforeEach, expect, mock, test } from "bun:test";

type Call = { cmd: string; args: any };
const calls: Call[] = [];
let stored: Record<string, unknown> = {};
mock.module("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: any) => {
    calls.push({ cmd, args });
    if (cmd === "account_state") return Promise.resolve(stored);
    return Promise.resolve(null);
  },
}));
mock.module("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

const { parseVolumes, loadPeerVolumes, setPeerVolume, resetVoice, volumeOf } = await import("../voice");
const { forgetAccountCache } = await import("../../lib/accountState");
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  calls.length = 0;
  stored = {};
  forgetAccountCache();
  resetVoice();
});

test("parseVolumes 只保留有效的非默认值并限幅", () => {
  expect(parseVolumes({ a: 50, b: 100, c: 300, d: "x", e: -5 })).toEqual({ a: 50, c: 200, e: 0 });
  expect(parseVolumes(null)).toEqual({});
  expect(parseVolumes([1, 2])).toEqual({});
});

test("登录后读取保存的成员音量并同步到后端", async () => {
  stored = { peerVolumes: { u1: 40 } };
  await loadPeerVolumes("me");
  expect(volumeOf("u1")).toBe(40);
  expect(calls.some((c) => c.cmd === "voice_set_volume" && c.args.userId === "u1" && c.args.percent === 40)).toBe(true);
});

test("修改成员音量按账号写入，恢复 100 时移除", async () => {
  await loadPeerVolumes("me");
  await setPeerVolume("u2", 150);
  await flush();
  let w = calls.filter((c) => c.cmd === "set_account_state").pop()!;
  expect(w.args.key).toBe("peerVolumes");
  expect(w.args.value).toEqual({ u2: 150 });
  await setPeerVolume("u2", 100);
  await flush();
  w = calls.filter((c) => c.cmd === "set_account_state").pop()!;
  expect(w.args.value).toEqual({});
});

test("退出登录后不再写入旧账号", async () => {
  await loadPeerVolumes("me");
  resetVoice();
  await setPeerVolume("u3", 80);
  await flush();
  expect(calls.some((c) => c.cmd === "set_account_state")).toBe(false);
});

test("静音切换恢复之前的音量", async () => {
  const { togglePeerMute } = await import("../voice");
  await setPeerVolume("u4", 60);
  await togglePeerMute("u4");
  expect(volumeOf("u4")).toBe(0);
  await togglePeerMute("u4");
  expect(volumeOf("u4")).toBe(60);
  await togglePeerMute("u5");
  await togglePeerMute("u5");
  expect(volumeOf("u5")).toBe(100);
});

test("成员降噪按账号保存并在登录后恢复", async () => {
  const { setPeerDenoise, peerDenoised, parseDenoised } = await import("../voice");
  expect(parseDenoised(["a", "a", 1, ""])).toEqual(["a"]);
  stored = { peerDenoise: ["u6"] };
  await loadPeerVolumes("me");
  expect(peerDenoised("u6")).toBe(true);
  expect(calls.some((c) => c.cmd === "voice_set_peer_denoise" && c.args.userId === "u6" && c.args.on)).toBe(true);
  await setPeerDenoise("u7", true);
  await setPeerDenoise("u6", false);
  await flush();
  const w = calls.filter((c) => c.cmd === "set_account_state" && c.args.key === "peerDenoise").pop()!;
  expect(w.args.value).toEqual(["u7"]);
});

test("所选设备不可用时显示为系统默认，保存的选择不变", async () => {
  const { shownSelection } = await import("../voice");
  const base = { devices: [{ id: "a", name: "A", default: true }], loaded: true, loading: false, error: null };
  expect(shownSelection({ ...base, selected: "a" } as any)).toBe("a");
  expect(shownSelection({ ...base, selected: "gone" } as any)).toBeNull();
  expect(shownSelection({ ...base, selected: null } as any)).toBeNull();
  expect(shownSelection({ ...base, loaded: false, selected: "gone" } as any)).toBe("gone");
});

test("修改降噪模型不改变开关，设置页与语音栏共用同一状态", async () => {
  const { setDenoiseMode, denoiseMode, denoise, setDenoise } = await import("../voice");
  await setDenoise(false);
  await setDenoiseMode("rnnoise");
  expect(denoiseMode()).toBe("rnnoise");
  expect(denoise()).toBe(false);
  const c = calls.filter((x) => x.cmd === "voice_set_denoise").pop()!;
  expect(c.args).toEqual({ on: false, mode: "rnnoise" });
  await setDenoiseMode("kook");
});
