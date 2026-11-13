use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::gate::{Gate, Need};

#[tokio::test]
#[ignore = "需要真实账号"]
async fn gate_matches_server() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let spec = std::env::var("CDAPP_LIVE_CHANNELS").expect("CDAPP_LIVE_CHANNELS");
    let api = Api::new(Some(token)).unwrap();
    let me = api.me().await.unwrap();
    let mut gate = Gate::default();
    gate.me = me.get("id").and_then(|v| v.as_str()).map(str::to_owned);
    let pairs: Vec<(String, String)> = spec
        .split(',')
        .filter_map(|p| p.split_once('=').map(|(a, b)| (a.into(), b.into())))
        .collect();
    let guilds = api.guilds().await.unwrap();
    let guilds = guilds
        .get("items")
        .unwrap_or(&guilds)
        .as_array()
        .cloned()
        .unwrap_or_default();
    for g in guilds {
        let id = g["id"].as_str().unwrap_or_default().to_owned();
        let list = api.channels(&id).await.unwrap();
        if !list
            .iter()
            .any(|c| pairs.iter().any(|(_, cid)| *cid == c.id))
        {
            continue;
        }
        let t = gate.ticket();
        gate.set_access(&id, api.guild_access(&id).await.unwrap(), t);
        let t = gate.ticket();
        gate.set_channels(&id, &list, t);
        for (name, cid) in &pairs {
            let Some(c) = list.iter().find(|c| c.id == *cid) else {
                continue;
            };
            let read = gate.check(cid, Need::Read).is_ok();
            let send = gate.check(cid, Need::Send).is_ok();
            let join = gate.check(cid, Need::Connect).is_ok();
            let history = api.messages(cid, None).await.is_ok();
            println!(
                "{name}: kind={:?} password={} sync={} read={read} send={send} connect={join} history_ok={history}",
                c.kind, c.has_password, c.permission_sync
            );
        }
    }
}

#[tokio::test]
#[ignore = "需要真实账号"]
async fn password_channel_reports_40310() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let id = std::env::var("CDAPP_LIVE_PW_CHANNEL").expect("CDAPP_LIVE_PW_CHANNEL");
    let api = Api::new(Some(token)).unwrap();
    match api.voice_gateway(&id, "").await {
        Err(cdapp_lib::error::AppError::Api { code, .. }) => {
            println!("code={code}");
            assert_eq!(code, 40310);
        }
        Ok(_) => println!("服务端已记住密码"),
        Err(e) => panic!("未识别的错误：{e}"),
    }
}
