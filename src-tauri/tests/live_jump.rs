use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::model::ChannelKind;

fn ts(m: &serde_json::Value) -> i64 {
    m["create_at"].as_i64().unwrap_or_default()
}

#[tokio::test]
#[ignore = "需要真实账号"]
async fn jump_loads_around_target() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let api = Api::new(Some(token)).unwrap();
    let guilds = api.guilds().await.unwrap();
    let guilds = guilds
        .get("items")
        .unwrap_or(&guilds)
        .as_array()
        .unwrap()
        .clone();
    let mut found = None;
    'outer: for g in &guilds {
        for c in api.channels(g["id"].as_str().unwrap()).await.unwrap() {
            if c.kind != ChannelKind::Text {
                continue;
            }
            let Ok(latest) = api.messages(&c.id, None).await else {
                continue;
            };
            if latest.end {
                continue;
            }
            let Ok(older) = api
                .messages(&c.id, latest.items[0]["rong_id"].as_str())
                .await
            else {
                continue;
            };
            if older.items.len() > 10 {
                found = Some((c.id, older.items[5].clone()));
                break 'outer;
            }
        }
    }
    let (channel, target) = found.expect("没有两页以上历史的频道");
    let id = target["rong_id"].as_str().unwrap();

    let loc = api.locate_message(id).await.unwrap().expect("目标应可查询");
    assert_eq!(loc.channel_id, channel);
    assert_eq!(loc.id, id);

    let after = api.messages_after(&channel, id).await.unwrap();
    assert_ne!(after.items.len(), 0);
    assert!(
        after.items.iter().all(|m| ts(m) >= ts(&target)),
        "after 只含更新的消息"
    );
    assert!(
        after
            .items
            .iter()
            .all(|m| m["rong_id"] != target["rong_id"]),
        "after 不含游标"
    );
    let before = api
        .messages(&channel, after.items[0]["rong_id"].as_str())
        .await
        .unwrap();
    assert!(
        before
            .items
            .iter()
            .any(|m| m["rong_id"] == target["rong_id"]),
        "片段包含目标"
    );
    let last = before.items.last().unwrap();
    let first_after = &after.items[0];
    assert!(ts(last) <= ts(first_after), "前后两页衔接");
    println!(
        "before {} after {} end {}",
        before.items.len(),
        after.items.len(),
        after.end
    );

    assert!(
        api.locate_message("00000000-0000-0000-0000-000000000000")
            .await
            .unwrap()
            .is_none()
    );
}
