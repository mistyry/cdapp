//! 频道有效权限。与前端 `src/lib/perms.ts` 的计算一致，用于命令边界拦截。

use super::model::{Channel, GuildAccess};

pub const ADMIN: u64 = 1 << 0;
pub const VIEW: u64 = 1 << 11;
pub const SEND: u64 = 1 << 12;
pub const VOICE_CONNECT: u64 = 1 << 15;

/// 拥有全部权限（服务器主或管理员）。
pub const ALL: u64 = u64::MAX;

/// 服务器基础权限：@全体成员（角色 0）与本人所有角色的并集。
#[must_use]
pub fn base(a: &GuildAccess) -> u64 {
    if a.master {
        return ALL;
    }
    let of = |id: i64| {
        a.roles
            .iter()
            .find(|r| r.id == id)
            .map_or(0, |r| r.permissions)
    };
    let p = a.my_roles.iter().fold(of(0), |p, id| p | of(*id));
    if p & ADMIN == 0 { p } else { ALL }
}

/// 频道有效权限：基础权限、@全体成员覆盖、本人角色覆盖合并、本人用户覆盖，依次先拒绝再允许。
/// 同步分组的频道由服务端复制了完整覆盖，不再叠加父分组。
#[must_use]
pub fn channel(a: &GuildAccess, me: &str, c: &Channel) -> u64 {
    let mut p = base(a);
    if p == ALL {
        return ALL;
    }
    let apply = |p: u64, allow: u64, deny: u64| (p & !deny) | allow;
    for o in c.overwrites.iter().filter(|o| o.role_id == 0) {
        p = apply(p, o.allow, o.deny);
    }
    let (allow, deny) = c
        .overwrites
        .iter()
        .filter(|o| o.role_id != 0 && a.my_roles.contains(&o.role_id))
        .fold((0, 0), |(al, de), o| (al | o.allow, de | o.deny));
    p = apply(p, allow, deny);
    for u in c.user_overwrites.iter().filter(|u| u.user_id == me) {
        p = apply(p, u.allow, u.deny);
    }
    p
}

#[must_use]
pub fn has(p: u64, bits: u64) -> bool {
    p & bits == bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kook::model::{Overwrite, Role, UserOverwrite};

    fn access(everyone: u64, mine: &[(i64, u64)], master: bool) -> GuildAccess {
        let mut roles = vec![role(0, everyone)];
        roles.extend(mine.iter().map(|(id, p)| role(*id, *p)));
        GuildAccess {
            guild_id: "g".into(),
            owner_id: "o".into(),
            master,
            my_roles: mine.iter().map(|(id, _)| *id).collect(),
            roles,
        }
    }

    fn role(id: i64, permissions: u64) -> Role {
        Role {
            id,
            name: String::new(),
            color: 0,
            position: 0,
            hoist: false,
            permissions,
        }
    }

    fn chan(ow: &[(i64, u64, u64)], uow: &[(&str, u64, u64)]) -> Channel {
        let v = serde_json::json!({"id": "c", "type": 1});
        let mut c = Channel::from_raw(&v).unwrap();
        c.overwrites = ow
            .iter()
            .map(|&(role_id, allow, deny)| Overwrite {
                role_id,
                allow,
                deny,
            })
            .collect();
        c.user_overwrites = uow
            .iter()
            .map(|&(u, allow, deny)| UserOverwrite {
                user_id: u.into(),
                allow,
                deny,
            })
            .collect();
        c
    }

    #[test]
    fn overwrite_order_matches_server() {
        let a = access(VIEW | SEND, &[(5, 0)], false);
        // @全体成员拒绝发送，角色允许可发。
        let p = channel(&a, "me", &chan(&[(0, 0, SEND), (5, SEND, 0)], &[]));
        assert!(has(p, VIEW | SEND));
        // 角色拒绝，用户允许可发。
        let p = channel(&a, "me", &chan(&[(5, 0, SEND)], &[("me", SEND, 0)]));
        assert!(has(p, SEND));
        // 角色允许，用户拒绝不可发。
        let p = channel(&a, "me", &chan(&[(5, SEND, 0)], &[("me", 0, SEND)]));
        assert!(!has(p, SEND));
        // 他人的用户覆盖与非本人角色不生效。
        let p = channel(&a, "me", &chan(&[(9, 0, VIEW)], &[("x", 0, VIEW)]));
        assert!(has(p, VIEW));
    }

    #[test]
    fn user_allow_overrides_everyone_deny() {
        let a = access(VIEW, &[], false);
        let c = chan(&[(0, 0, VIEW)], &[("me", VIEW, 0)]);
        assert!(has(channel(&a, "me", &c), VIEW));
        assert!(!has(channel(&a, "other", &c), VIEW));
    }

    #[test]
    fn master_and_admin_bypass() {
        let c = chan(&[(0, 0, VIEW | VOICE_CONNECT)], &[]);
        assert_eq!(channel(&access(0, &[], true), "me", &c), ALL);
        assert_eq!(channel(&access(0, &[(3, ADMIN)], false), "me", &c), ALL);
        assert!(!has(channel(&access(VIEW, &[], false), "me", &c), VIEW));
    }
}
