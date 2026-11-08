import { createStore } from "solid-js/store";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type StorageInfo } from "../lib/api";

// 数据目录状态。启动时先确认数据目录可用，再恢复登录。
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

// 把数据迁移到新位置后重启。
export async function relocate() {
  const path = await pick("选择数据位置");
  if (path) await run(() => api.storageRelocate(path), true);
}

// 数据目录丢失时重新指定已有数据目录。
export async function locate() {
  const path = await pick("选择数据位置");
  if (path) await run(() => api.storageSwitch(path), true);
}

// 把数据迁回默认位置后重启。
export async function relocateDefault() {
  const path = storage.info?.defaultRoot;
  if (path) await run(() => api.storageRelocate(path), true);
}

export const useDefault = () => run(() => api.storageSwitch(null), true);
export const clearCache = () => run(() => api.storageClearCache(), false);
export const restartApp = () => void api.restart();
