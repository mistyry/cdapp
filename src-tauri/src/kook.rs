//! KOOK 网页端协议。依据网页端抓包结果实现，非官方接口。

pub mod api;
pub mod auth;
pub mod emoji;
pub mod emoji_cache;
pub mod gate;
pub mod gateway;
pub mod model;
pub mod perms;
pub mod saying;
pub mod sound;
pub mod voice;

pub const BASE: &str = "https://www.kookapp.cn";
pub const CLIENT_VERSION: &str = "0.112.0";
/// 桌面端 User-Agent。服务端据 UA 同时含 `KOOK` 与 `Electron` 判定为 `pc` 设备，
/// 浏览器 UA 会被判定为 `web`，部分账号因此没有发言权限。
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) KOOK/0.112.0 Chrome/138.0.7204.251 Electron/37.6.0 Safari/537.36";

#[cfg(test)]
mod tests {
    #[test]
    fn user_agent_is_desktop() {
        // 服务端据此判定为 pc 设备。
        assert!(super::USER_AGENT.contains("KOOK/"));
        assert!(super::USER_AGENT.contains("Electron/"));
    }
}
