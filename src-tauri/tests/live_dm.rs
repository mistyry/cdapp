use std::collections::HashSet;

use cdapp_lib::kook::api::Api;

#[tokio::test]
#[ignore = "需要真实账号"]
async fn lists_and_reads_dm() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let api = Api::new(Some(token)).unwrap();
    let first = api.dm_chats(1).await.unwrap();
    println!("chats {} pages {}", first.items.len(), first.page_total);
    let mut codes: HashSet<String> = first.items.iter().map(|c| c.code.clone()).collect();
    if first.page_total > 1 {
        let second = api.dm_chats(2).await.unwrap();
        for c in &second.items {
            assert!(codes.insert(c.code.clone()), "分页重叠");
        }
    }
    let Some(chat) = first.items.iter().find(|c| !c.target.system) else {
        return;
    };
    assert_eq!(chat.code.len(), 32);
    assert_ne!(chat.code, chat.target.id, "chat_code 不是用户 ID");
    let again = api.open_dm(&chat.target.id).await.unwrap();
    assert_eq!(again.code, chat.code, "已有会话返回同一 chat_code");
    let h = api.dm_messages(&chat.code, None).await.unwrap();
    println!("history {} end {}", h.items.len(), h.end);
    if let Some(oldest) = h.items.first() {
        let id = oldest["id"].as_str().unwrap();
        let older = api.dm_messages(&chat.code, Some(id)).await.unwrap();
        assert!(
            older.items.iter().all(|m| m["id"] != oldest["id"]),
            "before 不含游标"
        );
        println!("older {}", older.items.len());
    }
    assert!(
        api.messages(&chat.code, None).await.is_err(),
        "chat_code 不能当频道 ID"
    );
}
