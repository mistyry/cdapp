#[tokio::test]
#[ignore = "访问 KOOK"]
async fn live_saying() {
    let api = cdapp_lib::kook::api::Api::new(None).unwrap();
    let list = cdapp_lib::kook::saying::list(api).await.unwrap();
    assert!(list.len() > 10);
    println!("{} 条，例：{:?}", list.len(), list[0]);
}
