use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::model::{Page, merge_channels};
use std::time::Instant;

#[tokio::test]
#[ignore = "需要真实账号"]
async fn view_matches_pages() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let api = Api::new(Some(token)).unwrap();
    let guilds = api.guilds().await.unwrap();
    let guilds = guilds
        .get("items")
        .unwrap_or(&guilds)
        .as_array()
        .unwrap()
        .clone();
    for g in &guilds {
        let id = g["id"].as_str().unwrap();
        let t = Instant::now();
        let view = api.channels(id).await.unwrap();
        let tv = t.elapsed();
        let t = Instant::now();
        let mut pages = Vec::new();
        let mut page = 1u32;
        loop {
            let n = page.to_string();
            let data = api
                .get(
                    "/api/v3/channel/list",
                    &[("guild_id", id), ("page", n.as_str()), ("page_size", "50")],
                )
                .await
                .unwrap();
            let p = Page::from_raw(data).unwrap();
            pages.push(p.items);
            if page >= p.page_total {
                break;
            }
            page += 1;
        }
        let tl = t.elapsed();
        let (mut list, _) = merge_channels(pages);
        let mut v: Vec<_> = view
            .iter()
            .cloned()
            .map(|mut c| {
                c.users.clear();
                c
            })
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        v.sort_by(|a, b| a.id.cmp(&b.id));
        let voice_users: usize = view.iter().map(|c| c.users.len()).sum();
        println!(
            "频道 {} 页 {page} view {tv:?} list {tl:?} 语音在线 {voice_users}",
            view.len()
        );
        assert_eq!(v, list);
    }
}
