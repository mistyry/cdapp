import { emit } from "@tauri-apps/api/event";

export const PIP_EVENT = "watch://pip";
export type PipMessage = { generation: number; seq: number; action: "back" | "ready" };

export function notifyMain(generation: number, seq: number, action: PipMessage["action"]) {
  return emit(PIP_EVENT, { generation, seq, action } satisfies PipMessage);
}
