import { createStore } from "solid-js/store";
import { api } from "../lib/api";

export const Effect = {
  USER_JOIN: 1,
  USER_LEAVE: 2,
  EXIT_CHANNEL: 3,
  MIC_MUTE: 4,
  MIC_UNMUTE: 5,
  DEAFEN: 8,
  UNDEAFEN: 9,
  MESSAGE: 10,
  MENTION: 13,
} as const;
export type EffectId = (typeof Effect)[keyof typeof Effect];

export const EFFECT_LIST: { id: EffectId; name: string }[] = [
  { id: Effect.USER_JOIN, name: "用户连接" },
  { id: Effect.USER_LEAVE, name: "用户离开" },
  { id: Effect.EXIT_CHANNEL, name: "语音连接断开" },
  { id: Effect.MIC_MUTE, name: "麦克风静音" },
  { id: Effect.MIC_UNMUTE, name: "麦克风取消静音" },
  { id: Effect.DEAFEN, name: "耳机静音" },
  { id: Effect.UNDEAFEN, name: "取消耳机静音" },
  { id: Effect.MESSAGE, name: "普通消息通知" },
  { id: Effect.MENTION, name: "提及我的消息通知" },
];

export function playJoin(item?: number) {
  if (item) void api.effectPlayItem(item).catch(() => {});
  else playEffect(Effect.USER_JOIN);
}

export function playEffect(id: EffectId) {
  void api.effectPlay(id).catch(() => {});
}

export const [effects, setEffects] = createStore({ volume: 30, off: [] as number[] });

export function applyEffectPrefs(volume: number, off: number[]) {
  setEffects({ volume, off: [...off] });
}

let volSeq = 0;
export async function setEffectVolume(percent: number) {
  const v = Math.max(0, Math.min(100, Math.round(percent)));
  const my = ++volSeq;
  const before = effects.volume;
  setEffects("volume", v);
  try {
    await api.setEffectVolume(v);
  } catch {
    if (my === volSeq) setEffects("volume", before);
  }
}

export async function setEffectEnabled(id: EffectId, on: boolean) {
  const before = [...effects.off];
  setEffects("off", (list) => (on ? list.filter((x) => x !== id) : [...list.filter((x) => x !== id), id]));
  try {
    await api.setEffectEnabled(id, on);
  } catch {
    setEffects("off", before);
  }
}

export const effectOn = (id: EffectId) => !effects.off.includes(id);
export const previewEffect = (id: EffectId) => void api.effectPlay(id, true).catch(() => {});
