use cdapp_lib::error::ErrorKind;
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::model::{Channel, ChannelKind, GuildAccess};

const ADMIN: u64 = 1;
const VIEW: u64 = 1 << 11;

fn can_view(a: &GuildAccess, me: &str, c: &Channel) -> bool {
    if a.master {
        return true;
    }
    let perm = |id: i64| {
        a.roles
            .iter()
            .find(|r| r.id == id)
            .map_or(0, |r| r.permissions)
    };
    let mut p = a.my_roles.iter().fold(perm(0), |p, r| p | perm(*r));
    if p & ADMIN != 0 {
        return true;
    }
    for o in c.overwrites.iter().filter(|o| o.role_id == 0) {
        p = (p & !o.deny) | o.allow;
    }
    let (mut allow, mut deny) = (0, 0);
    for o in c
        .overwrites
        .iter()
        .filter(|o| o.role_id != 0 && a.my_roles.contains(&o.role_id))
    {
        allow |= o.allow;
        deny |= o.deny;
    }
    p = (p & !deny) | allow;
    for u in c.user_overwrites.iter().filter(|u| u.user_id == me) {
        p = (p & !u.deny) | u.allow;
    }
    p & VIEW != 0
}

#[tokio::test]
#[ignore = "需要真实账号"]
async fn members_and_view_permission() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let api = Api::new(Some(token)).unwrap();
    let me = api.me().await.unwrap()["id"].as_str().unwrap().to_owned();
    let guilds = api.guilds().await.unwrap();
    let guilds = guilds
        .get("items")
        .unwrap_or(&guilds)
        .as_array()
        .unwrap()
        .clone();
    let (mut checked, mut agree, mut hidden) = (0, 0, 0);
    for g in guilds.iter().take(8) {
        let id = g["id"].as_str().unwrap();
        let a = api.guild_access(id).await.unwrap();
        let p = api.members(id, None, 1).await.unwrap();
        assert!(p.items.len() <= 50);
        println!(
            "成员 total {} online {} offline {}",
            p.total, p.online, p.offline
        );
        let first_offline = p
            .items
            .iter()
            .position(|m| !m.online)
            .unwrap_or(p.items.len());
        assert!(
            p.items[first_offline..].iter().all(|m| !m.online),
            "在线在前"
        );
        let chans = api.channels(id).await.unwrap();
        let texts: Vec<_> = chans
            .iter()
            .filter(|c| c.kind == ChannelKind::Text)
            .collect();
        hidden += texts.iter().filter(|c| !can_view(&a, &me, c)).count();
        for want in [true, false] {
            for c in texts
                .iter()
                .filter(|c| can_view(&a, &me, c) == want)
                .take(2)
            {
                let r = api.messages(&c.id, None).await;
                let readable = match r {
                    Ok(_) => true,
                    Err(e) if e.kind() == ErrorKind::Rejected => false,
                    Err(e) => panic!("{e}"),
                };
                checked += 1;
                agree += usize::from(readable == want);
            }
        }
    }
    println!("抽样 {checked}，一致 {agree}，算得不可见文字频道 {hidden}");
    assert_eq!(checked, agree);
    let e = api.members("0000000000000000", None, 1).await.unwrap_err();
    println!("无效服务器：{:?}", e.kind());
}
