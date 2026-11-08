import { api } from "./api";

// 按账号保存的本地状态，存于数据目录 accounts/<id>/state.json。
// 旧版保存在 localStorage，首次读取时导入并删除。
export type AccountKey = "emojiRecent" | "openFolders";

const legacyKey: Record<AccountKey, (me: string) => string> = {
  emojiRecent: (me) => `cdapp.emoji.recent.${me}`,
  openFolders: (me) => `cdapp.folders.open.${me}`,
};

const cache = new Map<string, Promise<Record<string, unknown>>>();

function takeLegacy(key: AccountKey, me: string): unknown {
  try {
    const ls = globalThis.localStorage;
    const raw = ls?.getItem(legacyKey[key](me));
    if (raw == null) return undefined;
    ls?.removeItem(legacyKey[key](me));
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

async function load(me: string): Promise<Record<string, unknown>> {
  let state: Record<string, unknown>;
  try {
    state = (await api.accountState(me)) ?? {};
  } catch {
    return {};
  }
  for (const key of Object.keys(legacyKey) as AccountKey[]) {
    if (key in state) continue;
    const v = takeLegacy(key, me);
    if (v === undefined) continue;
    state[key] = v;
    void api.setAccountState(me, key, v).catch(() => {});
  }
  return state;
}

export async function readAccount(me: string, key: AccountKey): Promise<unknown> {
  let p = cache.get(me);
  if (!p) {
    p = load(me);
    cache.set(me, p);
  }
  return (await p)[key];
}

// 写入失败只保留在内存中。
export function writeAccount(me: string, key: AccountKey, value: unknown) {
  const p = cache.get(me);
  if (p) void p.then((s) => (s[key] = value));
  void api.setAccountState(me, key, value).catch(() => {});
}

export function forgetAccountCache() {
  cache.clear();
}
