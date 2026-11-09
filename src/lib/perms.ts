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
  VOICE_MANAGE: 1 << 16,
  SPEAK: 1 << 23,
  // 服务器静音：对方听不到声音。已用测试账号核对。
  SERVER_DEAFEN: 1 << 24,
  // 服务器闭麦：对方不能说话。
  SERVER_MUTE: 1 << 25,
  KICK: 1 << 6,
  BAN: 1 << 7,
} as const;

// 拥有全部权限（服务器主或管理员）。位值超过 31 位，用 -1 表示。
export const ALL = -1;

export type Role = { id: number; name: string; color: number; position: number; hoist: boolean; permissions: number };
export type GuildAccess = { guildId: string; ownerId: string; master: boolean; myRoles: number[]; roles: Role[] };
export type Overwrite = { roleId: number; allow: number; deny: number };
export type UserOverwrite = { userId: string; allow: number; deny: number };

// 权限值可能超过 32 位，位运算一律经 BigInt，结果仍为安全整数。
const big = (n: number) => BigInt(Math.max(0, Math.trunc(n)));
const or = (a: number, b: number) => Number(big(a) | big(b));
const andNot = (a: number, b: number) => Number(big(a) & ~big(b));
export const bitOf = (n: number) => 2 ** n;

// 服务器基础权限：@全体成员（角色 0）与本人所有角色的并集。
export function basePerms(a: GuildAccess): number {
  if (a.master) return ALL;
  const byId = new Map(a.roles.map((r) => [r.id, r.permissions]));
  let p = byId.get(0) ?? 0;
  for (const id of a.myRoles) p = or(p, byId.get(id) ?? 0);
  return has(p, Perm.ADMIN) ? ALL : p;
}

// 频道有效权限：基础权限 → @全体成员覆盖 → 本人角色覆盖（合并后先拒绝再允许）→ 本人用户覆盖。
// 已用真实账号核对：算得无查看权限的频道历史接口返回 403，有权限的可读。
export function channelPerms(a: GuildAccess, meId: string, ow: Overwrite[] = [], uow: UserOverwrite[] = []): number {
  return checkPerms(a, meId, ow, uow).perms;
}

export const has = (p: number, bit: number) => p === ALL || (big(p) & big(bit)) === big(bit);

// KOOK 权限位名称，按官方文档 guild-role 权限说明。
export const PERM_NAMES: Record<number, string> = {
  0: "管理员", 1: "管理服务器", 2: "查看管理日志", 3: "创建服务器邀请", 4: "管理邀请",
  5: "频道管理", 6: "踢出用户", 7: "加入黑名单", 8: "管理自定义表情", 9: "修改服务器昵称",
  10: "管理角色权限", 11: "查看文字与语音频道", 12: "发布消息", 13: "管理消息", 14: "上传文件",
  15: "语音连接", 16: "语音管理", 17: "提及全体成员", 18: "添加反应", 19: "跟随添加反应",
  20: "被动连接语音频道", 21: "仅使用按键说话", 22: "使用自由麦", 23: "说话",
  24: "服务器静音", 25: "服务器闭麦", 26: "修改他人昵称", 27: "播放伴奏",
};

export type PermStep = { layer: "base" | "everyone" | "roles" | "user"; allow: number; deny: number; result: number };
export type PermCheck = {
  // ALL 表示服务器主或管理员。
  perms: number;
  all: boolean;
  reason: "master" | "admin" | null;
  // 按位列出结果，未在 PERM_NAMES 中的位名称为 null。
  bits: { bit: number; name: string | null; granted: boolean }[];
  steps: PermStep[];
};

const BITS = 53;
function listBits(p: number): PermCheck["bits"] {
  const known = Object.keys(PERM_NAMES).map(Number);
  const extra: number[] = [];
  for (let b = 0; b < BITS; b++) if (!(b in PERM_NAMES) && p !== ALL && has(p, bitOf(b))) extra.push(b);
  return [...known, ...extra].sort((a, b) => a - b).map((bit) => ({ bit, name: PERM_NAMES[bit] ?? null, granted: has(p, bitOf(bit)) }));
}

// 逐层计算频道权限，返回每层前后的结果。最终值与 channelPerms 一致。
export function checkPerms(a: GuildAccess, meId: string, ow: Overwrite[] = [], uow: UserOverwrite[] = []): PermCheck {
  const base = basePerms(a);
  if (base === ALL) {
    const reason = a.master ? "master" : "admin";
    return { perms: ALL, all: true, reason, bits: listBits(ALL), steps: [{ layer: "base", allow: 0, deny: 0, result: ALL }] };
  }
  const steps: PermStep[] = [{ layer: "base", allow: 0, deny: 0, result: base }];
  let p = base;
  const apply = (layer: PermStep["layer"], allow: number, deny: number) => {
    p = or(andNot(p, deny), allow);
    steps.push({ layer, allow, deny, result: p });
  };
  let allow = 0, deny = 0;
  for (const o of ow) if (o.roleId === 0) (allow = or(allow, o.allow), deny = or(deny, o.deny));
  apply("everyone", allow, deny);
  const mine = new Set(a.myRoles);
  allow = 0; deny = 0;
  for (const o of ow) if (o.roleId !== 0 && mine.has(o.roleId)) (allow = or(allow, o.allow), deny = or(deny, o.deny));
  apply("roles", allow, deny);
  allow = 0; deny = 0;
  for (const u of uow) if (u.userId === meId) (allow = or(allow, u.allow), deny = or(deny, u.deny));
  apply("user", allow, deny);
  return { perms: p, all: false, reason: null, bits: listBits(p), steps };
}
