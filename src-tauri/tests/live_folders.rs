//! 真实账号服务器文件夹读取验证，只读。默认忽略：
//! `CDAPP_LIVE_TOKEN=... cargo test --test live_folders -- --ignored --nocapture`
//! 核对文件夹条目字段与服务器列表的覆盖关系。只输出计数，不输出 ID 与名称。

use std::collections::HashSet;

use cdapp_lib::kook::api::Api;

#[tokio::test]
#[ignore = "需要真实账号"]
async fn folders_cover_guilds() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let api = Api::new(Some(token)).unwrap();
    let folders = api.guild_folders().await.unwrap();
    let items = folders.as_array().expect("items 为数组");
    let guilds = api.guilds().await.unwrap();
    let ids: HashSet<&str> = guilds
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["id"].as_str())
        .collect();
    let mut seen = HashSet::new();
    let mut named = 0;
    for it in items {
        let members = it["guild_ids"].as_array().expect("guild_ids 为数组");
        if it["id"].as_str().is_some_and(|s| !s.is_empty()) {
            named += 1;
        } else {
            assert_eq!(members.len(), 1, "未归组条目只含一个服务器");
        }
        for g in members {
            seen.insert(g.as_str().expect("服务器 ID 为字符串"));
        }
    }
    println!(
        "条目 {}，文件夹 {named}，服务器 {}，覆盖 {}",
        items.len(),
        ids.len(),
        seen.intersection(&ids).count()
    );
    assert!(ids.is_subset(&seen), "文件夹应覆盖全部服务器");
}
