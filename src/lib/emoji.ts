export type UnicodeEmoji = { char: string; name: string; tags: string[]; group: number };
export type CustomEmoji = { id: string; name: string; animated: boolean };

export type Pick =
  | { kind: "unicode"; char: string; name: string }
  | { kind: "custom"; id: string; name: string; animated: boolean; guildId: string };

export type Purpose =
  | { use: "send"; guildId: string | null }
  | { use: "react"; guildId: string | null };

export function unavailable(p: Pick, purpose: Purpose): string | null {
  if (p.kind === "unicode" || !p.guildId) return null;
  if (!purpose.guildId) return purpose.use === "send" ? "私信中不能发送服务器表情" : "私信中不能用服务器表情回应";
  return p.guildId === purpose.guildId ? null : "只能在所属服务器中使用";
}

export const pickKey = (p: Pick) => (p.kind === "unicode" ? `u:${p.char}` : `c:${p.id}`);

export const reactionOf = (p: Pick) => (p.kind === "unicode" ? { id: p.char, name: p.char } : { id: p.id, name: p.name });

export type Token = { t: "text"; v: string } | { t: "emj"; name: string; id: string };

const EMJ = /\(emj\)([^\n()[\]]{1,64})\(emj\)\[([A-Za-z0-9_-]{1,64}(?:\/[A-Za-z0-9_-]{1,64}){1,2})\]/g;

export function safeName(name: string): string {
  const s = name.replace(/[\n()[\]]/g, "").trim().slice(0, 32);
  return s || "表情";
}

export const emojiMarkup = (name: string, id: string) => `(emj)${safeName(name)}(emj)[${id}]`;

export const insertText = (p: Pick) => (p.kind === "unicode" ? p.char : emojiMarkup(p.name, p.id));

export function tokenize(src: string): Token[] {
  const out: Token[] = [];
  let last = 0;
  for (const m of src.matchAll(EMJ)) {
    if (m.index! > last) out.push({ t: "text", v: src.slice(last, m.index) });
    out.push({ t: "emj", name: m[1], id: m[2] });
    last = m.index! + m[0].length;
  }
  if (last < src.length) out.push({ t: "text", v: src.slice(last) });
  return out;
}

export const serialize = (tokens: Token[]) => tokens.map((x) => (x.t === "text" ? x.v : emojiMarkup(x.name, x.id))).join("");

const PICTO = /\p{Extended_Pictographic}|\p{Regional_Indicator}|⃣/u;
type Seg = { segment: (s: string) => Iterable<{ segment: string }> };
const SegCtor = (Intl as unknown as { Segmenter?: new () => Seg }).Segmenter;
const segmenter = SegCtor ? new SegCtor() : null;
const graphemes = (s: string) => (segmenter ? [...segmenter.segment(s)].map((x) => x.segment) : [...s]);

export function singleEmoji(src: string): boolean {
  const s = src.trim();
  if (!s || s.length > 200) return false;
  const t = tokenize(s);
  if (t.length !== 1) return false;
  if (t[0].t === "emj") return true;
  const g = graphemes(t[0].v);
  return g.length === 1 && PICTO.test(g[0]);
}

export const UNICODE_GROUPS: Array<{ id: number; name: string }> = [
  { id: 0, name: "笑脸与情感" }, { id: 1, name: "人与身体" }, { id: 3, name: "动物与自然" },
  { id: 4, name: "食物与饮料" }, { id: 5, name: "旅行与地点" }, { id: 6, name: "活动" },
  { id: 7, name: "物品" }, { id: 8, name: "符号" }, { id: 9, name: "旗帜" },
];

type Compact = { unicode: string; label: string; tags?: string[]; group?: number; order?: number };

export function fromCompact(list: Compact[]): UnicodeEmoji[] {
  const keep = new Set(UNICODE_GROUPS.map((g) => g.id));
  return list
    .filter((e) => e.group !== undefined && keep.has(e.group) && e.unicode)
    .sort((a, b) => (a.order ?? 0) - (b.order ?? 0))
    .map((e) => ({ char: e.unicode, name: e.label, tags: e.tags ?? [], group: e.group! }));
}

let unicodeCache: Promise<UnicodeEmoji[]> | null = null;
export function loadUnicode(): Promise<UnicodeEmoji[]> {
  unicodeCache ??= import("emojibase-data/zh/compact.json")
    .then((m) => fromCompact((m.default ?? m) as unknown as Compact[]))
    .catch((e) => {
      unicodeCache = null;
      throw e;
    });
  return unicodeCache;
}

export function search(unicode: UnicodeEmoji[], custom: Array<CustomEmoji & { guildId: string }>, query: string, limit = 160): Pick[] {
  const q = query.trim().toLowerCase().replace(/^:|:$/g, "");
  if (!q) return [];
  const scored: Array<[number, number, Pick]> = [];
  let i = 0;
  for (const c of custom) {
    const n = c.name.toLowerCase();
    const s = n === q ? 0 : n.startsWith(q) ? 1 : n.includes(q) ? 2 : -1;
    if (s >= 0) scored.push([s, i++, { kind: "custom", id: c.id, name: c.name, animated: c.animated, guildId: c.guildId }]);
  }
  for (const u of unicode) {
    const n = u.name.toLowerCase();
    const s = n === q || u.char === q ? 0 : n.startsWith(q) ? 1 : n.includes(q) ? 2 : u.tags.some((t) => t.toLowerCase().includes(q)) ? 3 : -1;
    if (s >= 0) scored.push([s, i++, { kind: "unicode", char: u.char, name: u.name }]);
  }
  return scored.sort((a, b) => a[0] - b[0] || a[1] - b[1]).slice(0, limit).map((x) => x[2]);
}

export const RECENT_MAX = 24;

export function pushRecent(list: Pick[], p: Pick, max = RECENT_MAX): Pick[] {
  const k = pickKey(p);
  return [p, ...list.filter((x) => pickKey(x) !== k)].slice(0, max);
}

export function parseRecent(raw: string | null): Pick[] {
  if (!raw) return [];
  try {
    const v = JSON.parse(raw);
    if (!Array.isArray(v)) return [];
    return v.filter((x): x is Pick =>
      (x?.kind === "unicode" && typeof x.char === "string" && x.char.length <= 32 && typeof x.name === "string")
      || (x?.kind === "custom" && typeof x.id === "string" && /^[A-Za-z0-9_-]{1,64}(\/[A-Za-z0-9_-]{1,64}){1,2}$/.test(x.id)
        && typeof x.name === "string" && typeof x.guildId === "string" && typeof x.animated === "boolean"),
    ).slice(0, RECENT_MAX);
  } catch {
    return [];
  }
}

export function gridMove(sections: Array<{ start: number; len: number }>, i: number, d: number, cols: number): number {
  if (Math.abs(d) !== cols) return i + d;
  const si = sections.findIndex((x) => i >= x.start && i < x.start + x.len);
  if (si < 0) return 0;
  const { start, len } = sections[si];
  const row = Math.floor((i - start) / cols);
  const col = (i - start) % cols;
  const lastRow = Math.floor((len - 1) / cols);
  if (d > 0) {
    if (row < lastRow) return Math.min(i + cols, start + len - 1);
    const n = sections.slice(si + 1).find((x) => x.len);
    return n ? n.start + Math.min(col, n.len - 1) : i;
  }
  if (row > 0) return i - cols;
  const prev = sections.slice(0, si).reverse().find((x) => x.len);
  if (!prev) return i;
  const rowStart = prev.start + Math.floor((prev.len - 1) / cols) * cols;
  return Math.min(rowStart + col, prev.start + prev.len - 1);
}
