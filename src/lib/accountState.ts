import { api } from "./api";

export type AccountKey = "emojiRecent" | "openFolders" | "peerVolumes" | "peerDenoise" | "shareVolumes" | "shareBeforeMute";

const legacyKey: Partial<Record<AccountKey, (me: string) => string>> = {
  emojiRecent: (me) => `cdapp.emoji.recent.${me}`,
  openFolders: (me) => `cdapp.folders.open.${me}`,
};

const cache = new Map<string, Promise<Record<string, unknown>>>();

function takeLegacy(key: AccountKey, me: string): unknown {
  try {
    const ls = globalThis.localStorage;
    const name = legacyKey[key]?.(me);
    if (!name) return undefined;
    const raw = ls?.getItem(name);
    if (raw == null) return undefined;
    ls?.removeItem(name);
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

export function writeAccount(me: string, key: AccountKey, value: unknown) {
  const p = cache.get(me);
  if (p) void p.then((s) => (s[key] = value));
  void api.setAccountState(me, key, value).catch(() => {});
}

export function forgetAccountCache() {
  cache.clear();
}
