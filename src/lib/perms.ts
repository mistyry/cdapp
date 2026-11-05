// KOOK 权限位（官方文档 guild-role 权限说明）。只列客户端使用的位。
export const Perm = {
  ADMIN: 1 << 0,
  VIEW: 1 << 11,
  SEND: 1 << 12,
  MANAGE_MESSAGES: 1 << 13,
  UPLOAD: 1 << 14,
  VOICE_CONNECT: 1 << 15,
  MENTION_ALL: 1 << 17,
  REACT: 1 << 18,
  SPEAK: 1 << 23,
} as const;

// 拥有全部权限（服务器主或管理员）。位值超过 31 位，用 -1 表示。
export const ALL = -1;

export type Role = { id: number; name: string; color: number; position: number; hoist: boolean; permissions: number };
export type GuildAccess = { guildId: string; ownerId: string; master: boolean; myRoles: number[]; roles: Role[] };
export type Overwrite = { roleId: number; allow: number; deny: number };
export type UserOverwrite = { userId: string; allow: number; deny: number };

// 服务器基础权限：@全体成员（角色 0）与本人所有角色的并集。
export function basePerms(a: GuildAccess): number {
  if (a.master) return ALL;
  const byId = new Map(a.roles.map((r) => [r.id, r.permissions]));
  let p = byId.get(0) ?? 0;
  for (const id of a.myRoles) p |= byId.get(id) ?? 0;
  return p & Perm.ADMIN ? ALL : p;
}

// 频道有效权限：基础权限 → @全体成员覆盖 → 本人角色覆盖（合并后先拒绝再允许）→ 本人用户覆盖。
// 已用真实账号核对：算得无查看权限的频道历史接口返回 403，有权限的可读。
export function channelPerms(a: GuildAccess, meId: string, ow: Overwrite[] = [], uow: UserOverwrite[] = []): number {
  let p = basePerms(a);
  if (p === ALL) return ALL;
  const apply = (allow: number, deny: number) => (p = ((p & ~deny) | allow) >>> 0);
  for (const o of ow) if (o.roleId === 0) apply(o.allow, o.deny);
  const mine = new Set(a.myRoles);
  let allow = 0;
  let deny = 0;
  for (const o of ow) if (o.roleId !== 0 && mine.has(o.roleId)) (allow |= o.allow, deny |= o.deny);
  apply(allow, deny);
  for (const u of uow) if (u.userId === meId) apply(u.allow, u.deny);
  return p;
}

export const has = (p: number, bit: number) => p === ALL || (p & bit) === bit;
