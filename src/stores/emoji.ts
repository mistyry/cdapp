import { createStore } from "solid-js/store";
import { api, errorText } from "../lib/api";
import { loadUnicode, parseRecent, pushRecent, type CustomEmoji, type Pick, type UnicodeEmoji } from "../lib/emoji";
import { readAccount, writeAccount } from "../lib/accountState";
import { accountGen, session } from "./session";

type GuildSet = { guildId: string; name: string; icon: string; items: CustomEmoji[] };

const initial = () => ({
  status: "idle" as "idle" | "loading" | "ready" | "failed",
  error: "",
  guilds: [] as GuildSet[],
  personal: [] as CustomEmoji[],
  personalFailed: false,
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
  void readAccount(me, "emojiRecent").then((v) => {
    if (gen !== accountGen()) return;
    const stored = parseRecent(v == null ? null : JSON.stringify(v));
    const cur = emoji.recent;
    setEmoji("recent", cur.reduceRight((acc, p) => pushRecent(acc, p), stored));
    if (cur.length) writeAccount(me, "emojiRecent", emoji.recent);
  });
}

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

export function noteRecent(p: Pick) {
  syncAccount();
  const me = session.me?.id;
  if (!me || emoji.recentOwner !== me) return;
  const next = pushRecent(emoji.recent, p);
  setEmoji("recent", next);
  writeAccount(me, "emojiRecent", next);
}

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

export function shownGuilds(current: string | null): GuildSet[] {
  const byId = new Map(emoji.guilds.filter((g) => g.items.length).map((g) => [g.guildId, g]));
  const ids = emoji.joined ? emoji.joined.filter((id) => byId.has(id)) : [...byId.keys()];
  if (current && byId.has(current) && !ids.includes(current)) ids.unshift(current);
  return ids.map((id) => byId.get(id)!);
}
