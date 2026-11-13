use std::collections::HashMap;

use super::model::{Channel, GuildAccess};
use super::perms;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Read,
    Send,
    Connect,
}

impl Need {
    fn bits(self) -> u64 {
        match self {
            Self::Read => perms::VIEW,
            Self::Send => perms::VIEW | perms::SEND,
            Self::Connect => perms::VIEW | perms::VOICE_CONNECT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    Channel,
    Access(String),
}

#[derive(Default)]
pub struct Gate {
    pub me: Option<String>,
    access: HashMap<String, GuildAccess>,
    channels: HashMap<String, (String, Channel)>,
    seq: u64,
    access_seq: HashMap<String, u64>,
    channels_seq: HashMap<String, u64>,
}

impl Gate {
    pub fn ticket(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    pub fn set_access(&mut self, guild_id: &str, a: GuildAccess, ticket: u64) {
        if !fresher(&mut self.access_seq, guild_id, ticket) {
            return;
        }
        self.access.insert(guild_id.to_owned(), a);
    }

    pub fn set_channels(&mut self, guild_id: &str, list: &[Channel], ticket: u64) {
        if !fresher(&mut self.channels_seq, guild_id, ticket) {
            return;
        }
        self.channels.retain(|_, (g, _)| g != guild_id);
        for c in list {
            self.channels
                .insert(c.id.clone(), (guild_id.to_owned(), c.clone()));
        }
    }

    pub fn retain_guilds(&mut self, ids: &std::collections::HashSet<String>) {
        self.access.retain(|g, _| ids.contains(g));
        self.channels.retain(|_, (g, _)| ids.contains(g));
    }

    #[must_use]
    pub fn knows(&self, channel_id: &str) -> bool {
        self.channels.contains_key(channel_id)
    }

    #[must_use]
    pub fn guild_of(&self, channel_id: &str) -> Option<&str> {
        self.channels.get(channel_id).map(|(g, _)| g.as_str())
    }

    #[must_use]
    pub fn missing(&self, channel_id: &str) -> Option<Missing> {
        match self.channels.get(channel_id) {
            None => Some(Missing::Channel),
            Some((g, _)) if !self.access.contains_key(g) => Some(Missing::Access(g.clone())),
            Some(_) => None,
        }
    }

    /// # Errors
    ///
    /// 依据缺失或权限不足时返回本地错误，请求不会发出。
    pub fn check(&self, channel_id: &str, need: Need) -> AppResult<()> {
        let unknown = || AppError::Other("无法取得频道权限".into());
        let (guild, channel) = self.channels.get(channel_id).ok_or_else(unknown)?;
        let access = self.access.get(guild).ok_or_else(unknown)?;
        let me = self.me.as_deref().ok_or_else(unknown)?;
        let p = perms::channel(access, me, channel);
        if perms::has(p, need.bits()) {
            return Ok(());
        }
        let msg = if !perms::has(p, perms::VIEW) {
            "没有查看该频道的权限"
        } else if need == Need::Send {
            "没有在该频道发言的权限"
        } else {
            "没有连接该语音频道的权限"
        };
        Err(AppError::Other(msg.into()))
    }
}

fn fresher(map: &mut HashMap<String, u64>, guild_id: &str, ticket: u64) -> bool {
    let cur = map.entry(guild_id.to_owned()).or_default();
    if ticket <= *cur {
        return false;
    }
    *cur = ticket;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kook::model::{Overwrite, Role};
    use serde_json::json;

    fn gate(deny: u64, everyone: u64) -> Gate {
        let mut g = Gate {
            me: Some("me".into()),
            ..Gate::default()
        };
        let t = g.ticket();
        g.set_access(
            "g",
            GuildAccess {
                guild_id: "g".into(),
                owner_id: "o".into(),
                master: false,
                my_roles: vec![],
                roles: vec![Role {
                    id: 0,
                    name: String::new(),
                    color: 0,
                    position: 0,
                    hoist: false,
                    permissions: everyone,
                }],
            },
            t,
        );
        let mut c = Channel::from_raw(&json!({"id": "c", "type": 2})).unwrap();
        c.overwrites = vec![Overwrite {
            role_id: 0,
            allow: 0,
            deny,
        }];
        let t = g.ticket();
        g.set_channels("g", &[c], t);
        g
    }

    const ALLP: u64 = perms::VIEW | perms::SEND | perms::VOICE_CONNECT;

    #[test]
    fn denies_without_view_or_connect() {
        assert!(gate(0, ALLP).check("c", Need::Connect).is_ok());
        assert!(gate(perms::VIEW, ALLP).check("c", Need::Connect).is_err());
        assert!(gate(perms::VIEW, ALLP).check("c", Need::Send).is_err());
        assert!(gate(perms::VIEW, ALLP).check("c", Need::Read).is_err());
        assert!(
            gate(perms::VOICE_CONNECT, ALLP)
                .check("c", Need::Connect)
                .is_err()
        );
        assert!(gate(perms::SEND, ALLP).check("c", Need::Send).is_err());
        assert!(gate(perms::SEND, ALLP).check("c", Need::Read).is_ok());
    }

    #[test]
    fn unknown_basis_is_denied() {
        let g = gate(0, ALLP);
        assert!(g.check("other", Need::Read).is_err());
        let mut g = gate(0, ALLP);
        g.me = None;
        assert!(g.check("c", Need::Read).is_err());
        let mut g = gate(0, ALLP);
        let t = g.ticket();
        g.set_channels("g", &[], t);
        assert!(!g.knows("c"));
        assert!(g.check("c", Need::Send).is_err());
    }

    #[test]
    fn late_older_result_does_not_restore_permission() {
        let mut g = gate(0, ALLP);
        let old = g.ticket();
        let new = g.ticket();
        let mut revoked = Channel::from_raw(&json!({"id": "c", "type": 2})).unwrap();
        revoked.overwrites = vec![Overwrite {
            role_id: 0,
            allow: 0,
            deny: perms::VIEW,
        }];
        g.set_channels("g", &[revoked], new);
        let stale = Channel::from_raw(&json!({"id": "c", "type": 2})).unwrap();
        g.set_channels("g", &[stale], old);
        assert!(g.check("c", Need::Connect).is_err());
    }

    #[test]
    fn missing_reports_what_to_load() {
        let mut g = Gate::default();
        assert_eq!(g.missing("c"), Some(Missing::Channel));
        let c = Channel::from_raw(&json!({"id": "c", "type": 2})).unwrap();
        let t = g.ticket();
        g.set_channels("g", &[c], t);
        assert_eq!(g.missing("c"), Some(Missing::Access("g".into())));
        assert_eq!(gate(0, ALLP).missing("c"), None);
    }

    #[test]
    fn retain_guilds_drops_left_guild() {
        let mut g = gate(0, ALLP);
        g.retain_guilds(&std::collections::HashSet::new());
        assert!(!g.knows("c"));
        assert!(g.check("c", Need::Read).is_err());
    }
}
