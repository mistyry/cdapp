import { createStore } from "solid-js/store";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type StorageInfo } from "../lib/api";

export const [storage, setStorage] = createStore({
  info: null as StorageInfo | null,
  busy: false,
  error: "",
});

export async function loadStorage(): Promise<StorageInfo | null> {
  try {
    const info = await api.storageInfo();
    setStorage({ info, error: "" });
    return info;
  } catch (e) {
    setStorage("error", errorText(e));
    return null;
  }
}

async function pick(title: string): Promise<string | null> {
  const r = await open({ directory: true, multiple: false, title });
  return typeof r === "string" ? r : null;
}

async function run(task: () => Promise<void>, restart: boolean) {
  if (storage.busy) return;
  setStorage({ busy: true, error: "" });
  try {
    await task();
    if (restart) await api.restart();
    else await loadStorage();
  } catch (e) {
    setStorage("error", errorText(e));
  } finally {
    setStorage("busy", false);
  }
}

export async function relocate() {
  const path = await pick("选择数据位置");
  if (path) await run(() => api.storageRelocate(path), true);
}

export async function locate() {
  const path = await pick("选择数据位置");
  if (path) await run(() => api.storageSwitch(path), true);
}

export async function relocateDefault() {
  const path = storage.info?.defaultRoot;
  if (path) await run(() => api.storageRelocate(path), true);
}

export const useDefault = () => run(() => api.storageSwitch(null), true);
export const clearCache = () => run(() => api.storageClearCache(), false);
export const restartApp = () => void api.restart();
