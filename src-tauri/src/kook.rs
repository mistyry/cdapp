pub mod api;
pub mod auth;
pub mod denoise_asset;
pub mod emoji;
pub mod emoji_cache;
pub mod gate;
pub mod gateway;
pub mod golive;
pub mod model;
pub mod perms;
pub mod saying;
pub mod sound;
pub mod voice;

pub const BASE: &str = "https://www.kookapp.cn";
pub const CLIENT_VERSION: &str = "0.112.0";
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) KOOK/0.112.0 Chrome/138.0.7204.251 Electron/37.6.0 Safari/537.36";

#[cfg(test)]
mod tests {
    #[test]
    fn user_agent_is_desktop() {
        assert!(super::USER_AGENT.contains("KOOK/"));
        assert!(super::USER_AGENT.contains("Electron/"));
    }
}
