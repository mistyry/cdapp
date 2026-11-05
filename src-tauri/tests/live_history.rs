//! 真实账号历史翻页验证（只读）。默认忽略：
//! `CDAPP_LIVE_TOKEN=... cargo test --test live_history -- --ignored`
//! 在第一个有满页历史的文字频道连续上翻，确认无重复、无断层、时间单调，并能读到结束。只输出数量。

use std::collections::HashSet;

use cdapp_lib::error::ErrorKind;
use cdapp_lib::kook::api::{Api, HISTORY_PAGE};

fn ts(m: &serde_json::Value) -> i64 {
    m["create_at"].as_i64().unwrap_or_default()
}

#[tokio::test]
#[ignore = "需要真实账号"]
async fn pages_back_without_gaps() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let api = Api::new(Some(token)).unwrap();
    let guilds = api.guilds().await.unwrap();
    let guilds = guilds
        .get("items")
        .unwrap_or(&guilds)
        .as_array()
        .unwrap()
        .clone();
    let mut busy = None;
    'outer: for g in &guilds {
        for c in api.channels(g["id"].as_str().unwrap()).await.unwrap() {
            if c.kind == cdapp_lib::kook::model::ChannelKind::Text
                && !api.messages(&c.id, None).await.unwrap().end
            {
                busy = Some(c.id);
                break 'outer;
            }
        }
    }
    let channel = busy.expect("没有满页历史的频道");
    let (mut seen, mut cursor, mut last, mut pages) = (HashSet::new(), None::<String>, i64::MAX, 0);
    loop {
        let h = api.messages(&channel, cursor.as_deref()).await.unwrap();
        for m in &h.items {
            assert!(
                seen.insert(m["rong_id"].as_str().unwrap().to_owned()),
                "重复消息"
            );
        }
        if let (Some(first), Some(newest)) = (h.items.first(), h.items.last()) {
            assert!(ts(newest) <= last, "页间时间倒退");
            last = ts(first);
            cursor = Some(first["id"].as_str().unwrap().to_owned());
        }
        pages += 1;
        if h.end || pages >= 30 {
            println!("页数 {pages} 消息 {} 结束 {}", seen.len(), h.end);
            assert!(h.items.len() < HISTORY_PAGE || pages >= 30);
            break;
        }
    }
    let bogus = api.messages(&channel, Some("bogus")).await.unwrap();
    assert!(bogus.end && bogus.items.is_empty());
    let e = api.messages("0000000000000000", None).await.unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Rejected);
}
