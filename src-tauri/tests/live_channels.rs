//! 真实账号频道分页验证（只读）。默认忽略：
//! `CDAPP_LIVE_TOKEN=... cargo test --test live_channels -- --ignored`
//! 对每个服务器读取全部频道，确认无重复且数量等于 meta.total。只输出数量。

use cdapp_lib::error::ErrorKind;
use cdapp_lib::kook::api::Api;
use serde_json::Value;

#[tokio::test]
#[ignore = "需要真实账号"]
async fn reads_all_pages() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let api = Api::new(Some(token)).unwrap();
    let guilds = api.guilds().await.unwrap();
    let guilds = guilds
        .get("items")
        .unwrap_or(&guilds)
        .as_array()
        .unwrap()
        .clone();
    let mut multi_page = false;
    for g in &guilds {
        let id = g["id"].as_str().unwrap();
        let list = api.channels(id).await.unwrap();
        let meta: Value = api
            .get("/api/v3/channel/list", &[("guild_id", id)])
            .await
            .unwrap();
        let total = usize::try_from(meta["meta"]["total"].as_u64().unwrap()).unwrap();
        multi_page |= meta["meta"]["page_total"].as_u64().unwrap() > 1;
        let unique: std::collections::HashSet<_> = list.iter().map(|c| &c.id).collect();
        println!("频道 {} / total {total}", list.len());
        assert_eq!(unique.len(), list.len());
        assert_eq!(list.len(), total);
    }
    println!("包含多页服务器：{multi_page}");
    let e = api.channels("0000000000000000").await.unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Rejected);
}
