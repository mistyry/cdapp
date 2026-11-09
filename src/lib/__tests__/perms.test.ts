import { expect, test } from "bun:test";
import { ALL, basePerms, channelPerms, checkPerms, has, Perm, type GuildAccess } from "../perms";

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

test("checkPerms 逐层结果与 channelPerms 一致", () => {
  const a = g({ myRoles: [5], roles: [role(0, Perm.VIEW | Perm.SEND), role(5, Perm.UPLOAD)] });
  const ow = [{ roleId: 0, allow: 0, deny: Perm.SEND }, { roleId: 5, allow: Perm.SEND, deny: Perm.UPLOAD }];
  const uow = [{ userId: "me", allow: 0, deny: Perm.VIEW }];
  const r = checkPerms(a, "me", ow, uow);
  expect(r.perms).toBe(channelPerms(a, "me", ow, uow));
  expect(r.steps.map((s) => s.layer)).toEqual(["base", "everyone", "roles", "user"]);
  expect(r.steps[1].result & Perm.SEND).toBe(0);
  expect(r.steps[2].result & Perm.SEND).toBe(Perm.SEND);
  expect(r.bits.find((b) => b.bit === 11)!.granted).toBe(false);
  expect(r.bits.find((b) => b.bit === 12)).toEqual({ bit: 12, name: "发布消息", granted: true });
});

test("checkPerms 区分服务器主与管理员", () => {
  expect(checkPerms(g({ master: true }), "me").reason).toBe("master");
  const r = checkPerms(g({ myRoles: [5], roles: [role(0, 0), role(5, Perm.ADMIN)] }), "me");
  expect(r.all && r.reason === "admin").toBe(true);
  expect(r.bits.every((b) => b.granted)).toBe(true);
});

test("checkPerms 列出未命名的已授予位", () => {
  const r = checkPerms(g({ roles: [role(0, Perm.VIEW | (1 << 29))] }), "me");
  expect(r.bits.find((b) => b.bit === 29)).toEqual({ bit: 29, name: null, granted: true });
});

test("超过 32 位的权限位不被截断", () => {
  const hi = 2 ** 32;
  const a = g({ roles: [role(0, hi + Perm.VIEW)] });
  expect(channelPerms(a, "me")).toBe(hi + Perm.VIEW);
  expect(has(channelPerms(a, "me", [{ roleId: 0, allow: 0, deny: hi }]), hi)).toBe(false);
  expect(has(channelPerms(a, "me", [{ roleId: 0, allow: 2 ** 33, deny: 0 }]), 2 ** 33)).toBe(true);
  expect(checkPerms(a, "me").bits.find((b) => b.bit === 32)!.granted).toBe(true);
});
