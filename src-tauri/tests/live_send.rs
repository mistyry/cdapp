use cdapp_lib::error::ErrorKind;
use cdapp_lib::kook::api::{Api, msg_type};
use serde_json::Value;

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn ids(list: &Value) -> Vec<(String, String)> {
    let items = list.get("items").unwrap_or(list);
    items
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            (
                m["rong_id"].as_str().unwrap_or_default().to_owned(),
                m["content"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "需要真实账号"]
async fn send_confirm_and_reject() {
    let (Some(token), Some(channel)) = (env("CDAPP_LIVE_TOKEN"), env("CDAPP_LIVE_CHANNEL")) else {
        panic!("缺少 CDAPP_LIVE_TOKEN 或 CDAPP_LIVE_CHANNEL");
    };
    let api = Api::new(Some(token)).unwrap();
    let text = format!("cdapp M1-02 live {:x}", rand::random::<u32>());

    let r = api
        .send_message(&channel, msg_type::KMARKDOWN, &text, None)
        .await
        .unwrap();
    let id = r["msg_id"].as_str().expect("msg_id").to_owned();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let hist = ids(&Value::Array(
        api.messages(&channel, None).await.unwrap().items,
    ));
    let copies = hist.iter().filter(|(_, c)| *c == text).count();

    let bad = api
        .send_message("0", msg_type::KMARKDOWN, "x", None)
        .await
        .unwrap_err();

    let anon = Api::new(Some("invalid".into())).unwrap();
    let unauth = anon
        .send_message(&channel, msg_type::KMARKDOWN, "x", None)
        .await
        .unwrap_err();

    api.delete_message(&id).await.unwrap();
    assert_eq!(copies, 1, "历史中应只有一条");
    assert!(hist.iter().any(|(i, _)| *i == id));
    assert_eq!(bad.kind(), ErrorKind::Rejected, "{bad}");
    assert_eq!(unauth.kind(), ErrorKind::Unauthorized, "{unauth}");
    eprintln!("bad: {bad}\nunauth: {unauth}");
}
