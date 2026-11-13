use serde::Deserialize;
use serde_json::{Value, json};

use super::api::Api;
use crate::error::{AppError, AppResult};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolcOptions {
    pub app_id: String,
    pub channel_name: String,
    pub token: String,
}

impl std::fmt::Debug for VolcOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolcOptions")
            .field("app_id", &self.app_id)
            .field("channel_name", &self.channel_name)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct Live {
    pub vendor: String,
    pub max_bitrate: u64,
    pub options: Option<VolcOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tier {
    pub resolution: u32,
    pub frame_rate: u32,
    pub mode: u8,
}

fn options_key(vendor: &str) -> &'static str {
    if vendor == "agora" {
        "agora_options"
    } else {
        "volc_options"
    }
}

fn parse_live(v: &Value) -> AppResult<Live> {
    let vendor = v
        .get("vendor")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let max_bitrate = v
        .get("max_bitrate")
        .and_then(Value::as_u64)
        .ok_or_else(|| AppError::Format("开播响应缺少码率".into()))?;
    let options = parse_options(v, options_key(&vendor))?;
    Ok(Live {
        vendor,
        max_bitrate,
        options,
    })
}

fn parse_options(v: &Value, key: &str) -> AppResult<Option<VolcOptions>> {
    match v.get(key) {
        Some(o) if o.get("enable").and_then(Value::as_bool) == Some(true) => {
            Ok(Some(serde_json::from_value(o.clone()).map_err(|e| {
                AppError::Format(format!("{key} 格式错误：{e}"))
            })?))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Clone)]
pub struct Watch {
    pub vendor: Vendor,
    pub options: VolcOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Vendor {
    #[default]
    Volc,
    Agora,
}

impl Vendor {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "volc" => Some(Self::Volc),
            "agora" => Some(Self::Agora),
            _ => None,
        }
    }
}

fn parse_watch(v: &Value) -> AppResult<Watch> {
    let vendor = match v.get("vendor").and_then(Value::as_str) {
        Some("volc") => Vendor::Volc,
        Some("agora") => Vendor::Agora,
        Some(_) => return Err(AppError::Other("暂不支持观看此分享服务".into())),
        None => return Err(AppError::Format("观看响应缺少分享服务".into())),
    };
    let key = match vendor {
        Vendor::Volc => "volc_options",
        Vendor::Agora => "agora_options",
    };
    let options =
        parse_options(v, key)?.ok_or_else(|| AppError::Format(format!("观看响应缺少 {key}")))?;
    Ok(Watch { vendor, options })
}

impl Api {
    /// # Errors
    ///
    /// 请求失败或响应缺少厂商时返回错误。
    pub async fn golive_vendor(
        &self,
        guild_id: &str,
        channel_id: &str,
        user_id: &str,
    ) -> AppResult<String> {
        let v = self
            .get(
                "/api/v3/gateway/check-join",
                &[
                    ("role", "live"),
                    ("guild_id", guild_id),
                    ("channel_id", channel_id),
                    ("check_module", "golive"),
                    ("live_user_id", user_id),
                    ("support_volc", "true"),
                    ("support_agora", "true"),
                ],
            )
            .await?;
        v.get("live_vendor")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| AppError::Format("检查结果缺少厂商".into()))
    }

    /// # Errors
    ///
    /// 不在语音中为 40000，档位超出会员为 40300。
    pub async fn go_live(
        &self,
        channel_id: &str,
        user_id: &str,
        tier: Tier,
        vendor: Vendor,
    ) -> AppResult<Live> {
        let v = self
            .post(
                "/api/v3/gateway/go-live",
                &json!({
                    "support_agora": vendor == Vendor::Agora,
                    "support_volc": vendor == Vendor::Volc,
                    "ip_discovery": [],
                    "role": "live",
                    "channel_id": channel_id,
                    "live_user_id": user_id,
                    "resolution": tier.resolution,
                    "frame_rate": tier.frame_rate,
                    "mode": tier.mode,
                }),
            )
            .await?;
        parse_live(&v)
    }

    /// # Errors
    ///
    /// 不在语音中为 40000，无连接权限为 HTTP 403，厂商未知或凭据缺失时返回错误。
    pub async fn go_live_watch(&self, channel_id: &str, liver: &str) -> AppResult<Watch> {
        let v = self
            .post(
                "/api/v3/gateway/go-live",
                &json!({
                    "support_agora": true,
                    "support_volc": true,
                    "ip_discovery": [],
                    "role": "audience",
                    "channel_id": channel_id,
                    "live_user_id": liver,
                    "resolution": 720,
                    "frame_rate": 30,
                }),
            )
            .await?;
        parse_watch(&v)
    }

    /// # Errors
    ///
    /// 不在语音中时返回错误。
    pub async fn go_live_update(
        &self,
        channel_id: &str,
        user_id: &str,
        tier: Tier,
    ) -> AppResult<u64> {
        let v = self
            .post(
                "/api/v3/gateway/go-live-update",
                &json!({
                    "channel_id": channel_id,
                    "live_user_id": user_id,
                    "resolution": tier.resolution,
                    "frame_rate": tier.frame_rate,
                    "mode": tier.mode,
                }),
            )
            .await?;
        v.get("max_bitrate")
            .and_then(Value::as_u64)
            .ok_or_else(|| AppError::Format("改档响应缺少码率".into()))
    }

    /// # Errors
    ///
    /// 未在推流中为 40000，或响应没有对应厂商的凭据。
    pub async fn go_live_refresh(
        &self,
        channel_id: &str,
        vendor: Vendor,
    ) -> AppResult<VolcOptions> {
        let v = self
            .post(
                "/api/v3/gateway/go-live-refresh",
                &json!({ "channel_id": channel_id }),
            )
            .await?;
        parse_refresh(&v, vendor)
    }
}

fn parse_refresh(v: &Value, vendor: Vendor) -> AppResult<VolcOptions> {
    let key = match vendor {
        Vendor::Volc => "volc_options",
        Vendor::Agora => "agora_options",
    };
    let o = v
        .get(key)
        .ok_or_else(|| AppError::Format(format!("续期响应缺少 {key}")))?;
    serde_json::from_value(o.clone()).map_err(|e| AppError::Format(format!("{key} 格式错误：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_live() {
        let v = json!({
            "vendor": "volc", "gateway_url": "", "max_bitrate": 2_850_000, "max_bitrate_h265": 2_280_000,
            "agora_options": {"enable": false},
            "volc_options": {"enable": true, "appId": "a", "channelName": "live.1.2", "token": "t"},
            "pull_sdk": 0,
        });
        let l = parse_live(&v).unwrap();
        assert_eq!(l.vendor, "volc");
        assert_eq!(l.max_bitrate, 2_850_000);
        let o = l.options.unwrap();
        assert_eq!(
            (o.app_id.as_str(), o.channel_name.as_str(), o.token.as_str()),
            ("a", "live.1.2", "t")
        );
    }

    #[test]
    fn parses_agora_go_live_and_refresh() {
        let v = json!({
            "vendor": "agora", "gateway_url": "", "max_bitrate": 2_850_000, "max_bitrate_h265": 2_280_000,
            "agora_options": {"enable": true, "appId": "a", "channelName": "live:1:2", "token": "t"},
            "volc_options": {"enable": false}, "pull_sdk": 0,
        });
        let l = parse_live(&v).unwrap();
        assert_eq!(l.vendor, "agora");
        assert_eq!(l.options.unwrap().channel_name, "live:1:2");
        let r = json!({"agora_options": {"enable": true, "appId": "a", "channelName": "live:1:2", "token": "n"}});
        assert_eq!(parse_refresh(&r, Vendor::Agora).unwrap().token, "n");
        assert!(parse_refresh(&r, Vendor::Volc).is_err());
        let kook = json!({"vendor": "kook", "max_bitrate": 1, "agora_options": {"enable": false}, "volc_options": {"enable": false}});
        assert!(parse_live(&kook).unwrap().options.is_none());
    }

    #[test]
    fn disabled_volc_is_none() {
        let v = json!({"vendor": "agora", "max_bitrate": 1, "volc_options": {"enable": false}});
        assert!(parse_live(&v).unwrap().options.is_none());
        assert!(parse_live(&json!({"vendor": "volc"})).is_err());
    }

    #[test]
    fn parses_watch_by_vendor() {
        let v = json!({"vendor": "agora", "pull_sdk": 0,
            "volc_options": {"enable": false},
            "agora_options": {"enable": true, "appId": "a", "channelName": "live.1.2", "token": "t"}});
        let w = parse_watch(&v).unwrap();
        assert_eq!(w.vendor, Vendor::Agora);
        assert_eq!(w.options.channel_name, "live.1.2");
        let v = json!({"vendor": "volc",
            "volc_options": {"enable": true, "appId": "b", "channelName": "c", "token": "t"}});
        assert_eq!(parse_watch(&v).unwrap().vendor, Vendor::Volc);
        assert!(
            parse_watch(&json!({"vendor": "agora", "agora_options": {"enable": false}})).is_err()
        );
        assert_eq!(
            parse_watch(&json!({"vendor": "kook", "gateway_url": "x", "agora_options": {"enable": false}, "volc_options": {"enable": false}}))
                .unwrap_err()
                .to_string(),
            "暂不支持观看此分享服务"
        );
    }

    #[test]
    fn debug_hides_token() {
        let o = VolcOptions {
            app_id: "a".into(),
            channel_name: "c".into(),
            token: "secret".into(),
        };
        assert!(!format!("{o:?}").contains("secret"));
    }
}
