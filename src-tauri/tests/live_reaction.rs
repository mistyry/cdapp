//! 真实账号个人表情回应验证。默认忽略：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_CHANNEL=<测试频道 ID> cargo test --test live_reaction -- --ignored --nocapture`
//! 在测试频道发送一条消息，经客户端接口用个人表情添加、重复添加、取消、重复取消回应，并读回核对。
//! 消息保留，回应全部撤销。只输出结论。

use cdapp_lib::kook::api::Api;

#[tokio::test]
#[ignore = "需要真实账号"]
async fn personal_emoji_reaction() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let channel = std::env::var("CDAPP_LIVE_CHANNEL").expect("CDAPP_LIVE_CHANNEL");
    let api = Api::new(Some(token)).unwrap();
    let library = api.emojis().await.unwrap();
    let Some(emoji) = library.personal.first() else {
        println!("账号没有个人表情，跳过");
        return;
    };
    let sent = api
        .send_message(&channel, 9, "personal reaction live", None)
        .await
        .unwrap();
    let mid = sent["msg_id"].as_str().unwrap().to_owned();
    let count = |v: &serde_json::Value| {
        v["reactions"].as_array().map_or(0, |a| {
            a.iter()
                .filter(|r| r["emoji"]["id"] == emoji.id.as_str())
                .count()
        })
    };
    api.reaction(&mid, &emoji.id, true).await.unwrap();
    api.reaction(&mid, &emoji.id, true).await.unwrap();
    let v = api
        .get("/api/v3/message/view", &[("msg_id", mid.as_str())])
        .await
        .unwrap();
    println!("添加后可见：{}", count(&v) == 1);
    assert_eq!(count(&v), 1);
    api.reaction(&mid, &emoji.id, false).await.unwrap();
    api.reaction(&mid, &emoji.id, false).await.unwrap();
    let v = api
        .get("/api/v3/message/view", &[("msg_id", mid.as_str())])
        .await
        .unwrap();
    println!("取消后清除：{}", count(&v) == 0);
    assert_eq!(count(&v), 0);
}
