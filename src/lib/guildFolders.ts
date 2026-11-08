import { readAccount, writeAccount } from "./accountState";
// 服务器栏结构：独立服务器与服务器文件夹。与服务器内的频道分组无关。
// 服务器业务数据仍按 ID 存放在 session.guilds，这里只保存顺序与归属。

export type FolderDto = { id: string; name: string; color: number | null; guildIds: string[] };
export type RailItem =
  | { kind: "guild"; id: string }
  | { kind: "folder"; id: string; name: string; color: number | null; guildIds: string[] };

// 解析接口条目，字段缺失或类型不符的条目跳过，不影响其他条目。
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

// 按文件夹顺序排列服务器。未归组条目展开为独立服务器；不在服务器列表中的引用与重复引用忽略；
// 没有有效成员的文件夹不显示；文件夹未覆盖的服务器按服务器列表顺序追加在末尾，保证不丢失。
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

// 文件夹底色。服务端颜色为 0xRRGGBB，缺失时使用品牌色。
export function folderTint(color: number | null, alpha: number): string {
  if (color == null || color < 0 || color > 0xffffff) return `color-mix(in srgb, var(--c-brand) ${Math.round(alpha * 100)}%, transparent)`;
  return `rgba(${(color >> 16) & 255}, ${(color >> 8) & 255}, ${color & 255}, ${alpha})`;
}

// 展开状态按账号保存在数据目录。只保存展开的文件夹，新文件夹默认折叠。
export function parseOpen(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, 256) : [];
}

export async function readOpen(me: string): Promise<string[]> {
  return parseOpen(await readAccount(me, "openFolders"));
}

export function writeOpen(me: string, ids: readonly string[]) {
  writeAccount(me, "openFolders", [...ids]);
}
