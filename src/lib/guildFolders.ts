import { readAccount, writeAccount } from "./accountState";

export type FolderDto = { id: string; name: string; color: number | null; guildIds: string[] };
export type RailItem =
  | { kind: "guild"; id: string }
  | { kind: "folder"; id: string; name: string; color: number | null; guildIds: string[] };

export function parseFolders(raw: unknown): FolderDto[] {
  if (!Array.isArray(raw)) return [];
  const out: FolderDto[] = [];
  for (const it of raw) {
    if (!it || typeof it !== "object") continue;
    const o = it as Record<string, unknown>;
    if (!Array.isArray(o.guild_ids)) continue;
    const color = typeof o.color === "number" && Number.isFinite(o.color) ? o.color : null;
    out.push({
      id: o.id == null ? "" : String(o.id),
      name: typeof o.name === "string" ? o.name : "",
      color,
      guildIds: o.guild_ids.filter((x) => typeof x === "string" || typeof x === "number").map(String),
    });
  }
  return out;
}

export function buildRail(guildIds: readonly string[], folders: readonly FolderDto[] | null): RailItem[] {
  const known = new Set(guildIds);
  const placed = new Set<string>();
  const seenFolder = new Set<string>();
  const out: RailItem[] = [];
  const take = (id: string) => known.has(id) && !placed.has(id) && (placed.add(id), true);
  for (const f of folders ?? []) {
    if (!f.id) {
      for (const g of f.guildIds) if (take(g)) out.push({ kind: "guild", id: g });
      continue;
    }
    if (seenFolder.has(f.id)) continue;
    seenFolder.add(f.id);
    const members = f.guildIds.filter(take);
    if (members.length) out.push({ kind: "folder", id: f.id, name: f.name, color: f.color, guildIds: members });
  }
  for (const g of guildIds) if (take(g)) out.push({ kind: "guild", id: g });
  return out;
}

export function folderTint(color: number | null, alpha: number): string {
  if (color == null || color < 0 || color > 0xffffff) return `color-mix(in srgb, var(--c-brand) ${Math.round(alpha * 100)}%, transparent)`;
  return `rgba(${(color >> 16) & 255}, ${(color >> 8) & 255}, ${color & 255}, ${alpha})`;
}

export function parseOpen(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, 256) : [];
}

export async function readOpen(me: string): Promise<string[]> {
  return parseOpen(await readAccount(me, "openFolders"));
}

export function writeOpen(me: string, ids: readonly string[]) {
  writeAccount(me, "openFolders", [...ids]);
}
