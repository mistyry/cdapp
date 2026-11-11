//! 屏幕分享接口：check-join、go-live、go-live-update、go-live-refresh。
//! 都要求本人已在该频道语音中。RTC 令牌与登录凭据同等保管，不写入日志。

use serde::Deserialize;
use serde_json::{Value, json};

use super::api::Api;
use crate::error::{AppError, AppResult};

/// 火山 RTC 凭据。`token` 绑定房间名与本人用户 ID。
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

/// 开播结果。
#[derive(Debug, Clone)]
pub struct Live {
    /// 分配的厂商，`volc` 或 `agora`。
    pub vendor: String,
    /// H.264 码率上限，单位 bps。
    pub max_bitrate: u64,
    pub volc: Option<VolcOptions>,
}

/// 档位。`resolution` 为 720、1080 等；`mode` 0 为标准模式，1 为文档模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tier {
    pub resolution: u32,
    pub frame_rate: u32,
    pub mode: u8,
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
    let volc = parse_volc(v)?;
    Ok(Live {
        vendor,
        max_bitrate,
        volc,
    })
}

/// `volc_options.enable` 为假或缺失时视为未分配火山。
fn parse_volc(v: &Value) -> AppResult<Option<VolcOptions>> {
    parse_options(v, "volc_options")
}

/// 指定厂商的凭据，`enable` 为假或缺失时为 None。两家字段名相同。
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

/// 观看凭据：厂商与对应的 RTC 凭据。两家的 appId、房间名、令牌字段相同。
#[derive(Debug, Clone)]
pub struct Watch {
    pub vendor: Vendor,
    pub options: VolcOptions,
}

/// 屏幕分享的 RTC 厂商，跟随分享者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Volc,
    Agora,
}

fn parse_watch(v: &Value) -> AppResult<Watch> {
    let vendor = match v.get("vendor").and_then(Value::as_str) {
        Some("volc") => Vendor::Volc,
        Some("agora") => Vendor::Agora,
        other => {
            return Err(AppError::Other(format!(
                "分享者使用的分享服务暂不支持：{}",
                other.unwrap_or("未知")
            )));
        }
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
    /// 分配给本人的屏幕分享厂商，`volc` 或 `agora`。没有会员时同样返回，不能用于判断能否开播。
    ///
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

    /// 开播，取 RTC 凭据与码率上限。只声明支持火山。
    /// 推流后服务端才登记分享，只调用本接口不会出现在成员列表中。
    ///
    /// # Errors
    ///
    /// 不在语音中为 40000，档位超出会员为 40300。
    pub async fn go_live(&self, channel_id: &str, user_id: &str, tier: Tier) -> AppResult<Live> {
        let v = self
            .post(
                "/api/v3/gateway/go-live",
                &json!({
                    "support_agora": false,
                    "support_volc": true,
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

    /// 观看他人的分享，取观众身份的 RTC 凭据与厂商。厂商跟随分享者：只声明火山时，分到 Agora 的分享者会被拒绝，
    /// 返回 40000「屏幕分享技术升级中」，因此同时声明两家。分享者尚未推流时服务端返回 41904。
    /// 档位为必填项，观众身份不影响画质，固定传 720p30。
    ///
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

    /// 推流中改档，返回新的码率上限，单位 bps。服务端不校验会员，调用方只能传本人拥有的档位。
    ///
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

    /// 推流中续期令牌。
    ///
    /// # Errors
    ///
    /// 未在推流中为 40000，或响应没有火山凭据。
    pub async fn go_live_refresh(&self, channel_id: &str) -> AppResult<VolcOptions> {
        let v = self
            .post(
                "/api/v3/gateway/go-live-refresh",
                &json!({ "channel_id": channel_id }),
            )
            .await?;
        // 续期响应的 volc_options 可能不带 enable，有令牌即可。
        let o = v
            .get("volc_options")
            .ok_or_else(|| AppError::Format("续期响应缺少火山凭据".into()))?;
        serde_json::from_value(o.clone())
            .map_err(|e| AppError::Format(format!("火山凭据格式错误：{e}")))
    }
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
        let o = l.volc.unwrap();
        assert_eq!(
            (o.app_id.as_str(), o.channel_name.as_str(), o.token.as_str()),
            ("a", "live.1.2", "t")
        );
    }

    #[test]
    fn disabled_volc_is_none() {
        let v = json!({"vendor": "agora", "max_bitrate": 1, "volc_options": {"enable": false}});
        assert!(parse_live(&v).unwrap().volc.is_none());
        assert!(parse_live(&json!({"vendor": "volc"})).is_err());
    }

    #[test]
    fn parses_watch_by_vendor() {
        // 实测：分享者分到 Agora 时 volc_options 为空，agora_options 带凭据。
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
        assert!(parse_watch(&json!({"vendor": "kook"})).is_err());
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
