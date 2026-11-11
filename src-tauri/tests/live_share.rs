//! 屏幕分享推流的真实环境测试，仅 Windows。需要会员账号已在语音频道中，例如先运行 poc 的 voice.py：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_USER=... CDAPP_LIVE_VOICE=... CDAPP_LIVE_SECS=60
//!  cargo test --test live_share -- --ignored --nocapture`
//! 下载并加载 SDK，以 1080p30 推指定显示器，持续指定秒数后停止。不输出令牌。
#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::golive::Tier;
use cdapp_lib::share::bridge::{self, SourceKind};
use cdapp_lib::share::sdk::{self, Status};
use cdapp_lib::share::session::{Request, Session, ShareEvent, Sink};

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

#[test]
#[ignore = "需要真实账号与语音频道"]
fn pushes_primary_screen() {
    let secs: u64 = std::env::var("CDAPP_LIVE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let dir = std::env::temp_dir().join(format!("cdapp-live-share-{}", std::process::id()));
    cdapp_lib::storage::init(&dir.join("fixed"), &dir.join("data"), &dir.join("fixed"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let s = sdk::sync().await;
        assert!(matches!(s, Status::Ready { .. }), "{s:?}");
        bridge::set_log_dir(&dir.join("sdklog")).unwrap();
        // CDAPP_LIVE_SCREEN 按名称选显示器，例如 \\.\DISPLAY1；省略时为主显示器。
        let want = std::env::var("CDAPP_LIVE_SCREEN").ok();
        let screen = bridge::sources()
            .unwrap()
            .into_iter()
            .find(|s| {
                s.kind == SourceKind::Screen && want.as_deref().map_or(s.primary, |n| s.name == n)
            })
            .expect("找不到显示器");
        println!("源 {} {}x{}", screen.name, screen.width, screen.height);

        let log: Arc<Mutex<Vec<ShareEvent>>> = Arc::default();
        let l = log.clone();
        let sink: Sink = Arc::new(move |e: ShareEvent| {
            println!("事件 {e:?}");
            l.lock().unwrap().push(e);
        });
        let session = Session::start(
            Api::new(Some(env("CDAPP_LIVE_TOKEN"))).unwrap(),
            Request {
                channel_id: env("CDAPP_LIVE_VOICE"),
                user_id: env("CDAPP_LIVE_USER"),
                source_kind: SourceKind::Screen,
                source_id: screen.id,
                tier: Tier {
                    resolution: 1080,
                    frame_rate: 30,
                    mode: 0,
                },
                audio: false,
            },
            1,
            sink,
        );
        tokio::time::sleep(Duration::from_secs(secs)).await;
        assert!(
            !session.is_finished(),
            "推流提前结束：{:?}",
            log.lock().unwrap()
        );
        session.stop().await;
        let events = log.lock().unwrap();
        assert!(
            events.iter().any(|e| matches!(e, ShareEvent::Live { .. })),
            "{events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, ShareEvent::Stats { .. })),
            "{events:?}"
        );
        assert!(
            matches!(events.last(), Some(ShareEvent::Ended { message: None })),
            "{events:?}"
        );
    });
    bridge::stop();
    // SDK 日志中不得出现令牌。
    let token = env("CDAPP_LIVE_TOKEN");
    for f in std::fs::read_dir(dir.join("sdklog"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let text = std::fs::read(f.path()).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&text).contains(&token),
            "SDK 日志含账号令牌"
        );
    }
}
