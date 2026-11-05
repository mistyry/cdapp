import { expect, test } from "bun:test";
import { ALL, basePerms, channelPerms, has, Perm, type GuildAccess } from "../perms";

const role = (id: number, permissions: number) => ({ id, name: "", color: 0, position: id, hoist: false, permissions });
const g = (p: Partial<GuildAccess>): GuildAccess => ({ guildId: "g", ownerId: "o", master: false, myRoles: [], roles: [role(0, Perm.VIEW | Perm.SEND)], ...p });

test("服务器主与管理员拥有全部权限", () => {
  expect(basePerms(g({ master: true }))).toBe(ALL);
  expect(basePerms(g({ myRoles: [5], roles: [role(0, 0), role(5, Perm.ADMIN)] }))).toBe(ALL);
  expect(channelPerms(g({ master: true }), "me", [{ roleId: 0, allow: 0, deny: Perm.VIEW }])).toBe(ALL);
});

test("基础权限为全体成员与本人角色的并集，未持有的角色不计", () => {
  const a = g({ myRoles: [5], roles: [role(0, Perm.VIEW), role(5, Perm.SEND), role(6, Perm.MANAGE_MESSAGES)] });
  expect(basePerms(a)).toBe(Perm.VIEW | Perm.SEND);
});

test("覆盖顺序：全体成员 → 本人角色（合并）→ 本人用户", () => {
  const a = g({ myRoles: [5, 6] });
  // 全体成员拒绝查看，本人角色允许 → 可见。
  expect(has(channelPerms(a, "me", [{ roleId: 0, allow: 0, deny: Perm.VIEW }, { roleId: 5, allow: Perm.VIEW, deny: 0 }]), Perm.VIEW)).toBe(true);
  // 角色之间：一个拒绝一个允许，允许优先。
  expect(has(channelPerms(a, "me", [{ roleId: 5, allow: 0, deny: Perm.SEND }, { roleId: 6, allow: Perm.SEND, deny: 0 }]), Perm.SEND)).toBe(true);
  // 未持有的角色覆盖不生效。
  expect(has(channelPerms(a, "me", [{ roleId: 0, allow: 0, deny: Perm.VIEW }, { roleId: 9, allow: Perm.VIEW, deny: 0 }]), Perm.VIEW)).toBe(false);
  // 用户覆盖最后应用，且只对本人生效。
  const ow = [{ roleId: 5, allow: Perm.SEND, deny: 0 }];
  expect(has(channelPerms(a, "me", ow, [{ userId: "me", allow: 0, deny: Perm.SEND }]), Perm.SEND)).toBe(false);
  expect(has(channelPerms(a, "me", ow, [{ userId: "x", allow: 0, deny: Perm.SEND }]), Perm.SEND)).toBe(true);
});

test("高位权限不产生负数", () => {
  const p = channelPerms(g({ roles: [role(0, 2 ** 31 + Perm.VIEW)] }), "me", [{ roleId: 0, allow: 2 ** 30, deny: 0 }]);
  expect(p).toBeGreaterThan(0);
  expect(has(p, Perm.VIEW)).toBe(true);
});
