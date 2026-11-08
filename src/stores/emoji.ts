import { createStore } from "solid-js/store";
import { api, errorText } from "../lib/api";
import { loadUnicode, parseRecent, pushRecent, type CustomEmoji, type Pick, type UnicodeEmoji } from "../lib/emoji";
import { readAccount, writeAccount } from "../lib/accountState";
import { accountGen, session } from "./session";

// 表情库。服务器与个人表情按账号读取一次，可手动重试；Unicode 数据首次打开选择器时加载。
// 迟到的结果按账号代次丢弃，不会写入后来登录的账号。
type GuildSet = { guildId: string; name: string; icon: string; items: CustomEmoji[] };

const initial = () => ({
  status: "idle" as "idle" | "loading" | "ready" | "failed",
  error: "",
  guilds: [] as GuildSet[],
  personal: [] as CustomEmoji[],
  personalFailed: false,
  // 已添加的服务器表情组，按用户排序；null 表示读取失败。
  joined: null as string[] | null,
  unicode: [] as UnicodeEmoji[],
  unicodeStatus: "idle" as "idle" | "loading" | "ready" | "failed",
  recent: [] as Pick[],
  recentOwner: "",
});

export const [emoji, setEmoji] = createStore(initial());

let loadedGen = -1;
let inflight: Promise<void> | null = null;

function syncAccount() {
  const gen = accountGen();
  if (gen === loadedGen) return;
  loadedGen = gen;
  inflight = null;
  setEmoji(initial());
  const me = session.me?.id ?? "";
  setEmoji({ recent: [], recentOwner: me });
  if (!me) return;
  // 读取完成前已有新的使用记录时，合并到读取结果之前。
  void readAccount(me, "emojiRecent").then((v) => {
    if (gen !== accountGen()) return;
    const stored = parseRecent(v == null ? null : JSON.stringify(v));
    const cur = emoji.recent;
    setEmoji("recent", cur.reduceRight((acc, p) => pushRecent(acc, p), stored));
    if (cur.length) writeAccount(me, "emojiRecent", emoji.recent);
  });
}

// 打开选择器时调用。已加载或正在加载时不重复请求；force 用于失败后的重试。
export function ensureEmojis(force = false): Promise<void> {
  syncAccount();
  if (emoji.unicodeStatus === "idle" || (force && emoji.unicodeStatus === "failed")) {
    const gen = accountGen();
    setEmoji("unicodeStatus", "loading");
    loadUnicode().then(
      (list) => gen === accountGen() && setEmoji({ unicode: list, unicodeStatus: "ready" }),
      () => gen === accountGen() && setEmoji("unicodeStatus", "failed"),
    );
  }
  if (!session.me) return Promise.resolve();
  if (inflight || emoji.status === "ready" || (emoji.status === "failed" && !force)) return inflight ?? Promise.resolve();
  const gen = accountGen();
  setEmoji({ status: "loading", error: "" });
  inflight = api.emojis().then(
    (r) => {
      if (gen !== accountGen()) return;
      setEmoji({
        status: "ready", personalFailed: r.personalFailed, personal: r.personal, joined: r.joined,
        guilds: r.guilds.map((g) => ({ guildId: g.guildId, name: g.name, icon: g.icon, items: g.items })),
      });
    },
    (e) => {
      if (gen !== accountGen()) return;
      setEmoji({ status: "failed", error: errorText(e) });
    },
  ).finally(() => {
    if (gen === accountGen()) inflight = null;
  });
  return inflight;
}

// 记录最近使用，按账号保存在数据目录，最多 24 个。
export function noteRecent(p: Pick) {
  syncAccount();
  const me = session.me?.id;
  if (!me || emoji.recentOwner !== me) return;
  const next = pushRecent(emoji.recent, p);
  setEmoji("recent", next);
  writeAccount(me, "emojiRecent", next);
}

// 查找自定义表情所属服务器；不在当前列表中的视为失效。
export function customOf(id: string): (CustomEmoji & { guildId: string }) | null {
  const p = emoji.personal.find((x) => x.id === id);
  if (p) return { ...p, guildId: "" };
  const gid = id.split("/")[0];
  const g = emoji.guilds.find((x) => x.guildId === gid);
  const e = g?.items.find((x) => x.id === id);
  return e ? { ...e, guildId: gid } : null;
}

export function allCustom(): Array<CustomEmoji & { guildId: string }> {
  return [...emoji.personal.map((x) => ({ ...x, guildId: "" })), ...emoji.guilds.flatMap((g) => g.items.map((x) => ({ ...x, guildId: g.guildId })))];
}

// 选择器中的服务器表情组：用户已添加的组按其顺序；当前服务器始终可用，未添加时排在最前。
// 已添加列表读取失败时退回为全部有表情的服务器。
export function shownGuilds(current: string | null): GuildSet[] {
  const byId = new Map(emoji.guilds.filter((g) => g.items.length).map((g) => [g.guildId, g]));
  const ids = emoji.joined ? emoji.joined.filter((id) => byId.has(id)) : [...byId.keys()];
  if (current && byId.has(current) && !ids.includes(current)) ids.unshift(current);
  return ids.map((id) => byId.get(id)!);
}
