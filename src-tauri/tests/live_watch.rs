#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cdapp_lib::kook::api::Api;
use cdapp_lib::share::sdk::{self, Status};
use cdapp_lib::share::watch::{Request, Session, StateSink, WatchState};

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

#[derive(Default)]
struct Got {
    frames: u32,
    keys: Vec<Duration>,
    bytes: u64,
    first: Option<Vec<u8>>,
    size: (u16, u16),
}

#[test]
#[ignore = "需要真实账号与正在推流的分享者"]
#[allow(clippy::too_many_lines)]
fn watches_live_share() {
    let secs: u64 = std::env::var("CDAPP_LIVE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let key_at: Option<u64> = std::env::var("CDAPP_LIVE_KEY_AT")
        .ok()
        .and_then(|v| v.parse().ok());
    let dir = std::env::temp_dir().join(format!("cdapp-live-watch-{}", std::process::id()));
    cdapp_lib::storage::init(&dir.join("fixed"), &dir.join("data"), &dir.join("fixed"));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let s = sdk::sync().await;
        assert!(matches!(s, Status::Ready { .. }), "{s:?}");
        cdapp_lib::share::bridge::set_log_dir(&dir.join("sdklog")).unwrap();
        cdapp_lib::share::bridge::agora_log(&dir.join("sdklog").join("agorasdk.log")).unwrap();

        let t0 = Instant::now();
        let got: Arc<Mutex<Got>> = Arc::default();
        let g = got.clone();
        let frames = Box::new(move |msg: Vec<u8>| {
            let mut g = g.lock().unwrap();
            g.frames += 1;
            g.bytes += msg.len() as u64;
            if msg[0] == 1 {
                g.keys.push(t0.elapsed());
            }
            g.size = (
                u16::from_le_bytes([msg[2], msg[3]]),
                u16::from_le_bytes([msg[4], msg[5]]),
            );
            if g.first.is_none() {
                g.first = Some(msg);
            }
            Ok(())
        });
        let states: Arc<Mutex<Vec<String>>> = Arc::default();
        let st = states.clone();
        let sink: StateSink = Arc::new(move |s: WatchState| {
            if !matches!(s, WatchState::Stats { .. }) {
                println!("状态 {:.1}s {s:?}", t0.elapsed().as_secs_f32());
            }
            st.lock().unwrap().push(format!("{s:?}"));
        });
        let (end_voice, voice_end) = tokio::sync::oneshot::channel::<()>();
        let session = Session::start(
            Api::new(Some(env("CDAPP_LIVE_TOKEN"))).unwrap(),
            Request {
                channel_id: env("CDAPP_LIVE_VOICE"),
                user_id: env("CDAPP_LIVE_USER"),
                liver: env("CDAPP_LIVE_LIVER"),
            },
            1,
            sink,
            frames,
            Box::pin(async move {
                let _ = voice_end.await;
            }),
        );
        let mut keyed = false;
        for _ in 0..secs * 10 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(
                !session.is_finished(),
                "观看提前结束：{:?}",
                states.lock().unwrap()
            );
            if let Some(at) = key_at
                && !keyed
                && t0.elapsed() >= Duration::from_secs(at)
            {
                keyed = true;
                println!("请求关键帧 {:.1}s", t0.elapsed().as_secs_f32());
                session.key_frame();
            }
        }
        session.stop().await;
        drop(end_voice);

        let g = got.lock().unwrap();
        let first = g.first.as_ref().expect("没有收到任何画面");
        println!(
            "收到 {} 帧 {} KB，{}x{}，关键帧时刻 {:?}",
            g.frames,
            g.bytes / 1000,
            g.size.0,
            g.size.1,
            g.keys
        );
        assert_eq!(first[0], 1, "首帧不是关键帧");
        let want: u8 = std::env::var("CDAPP_LIVE_CODEC")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        assert_eq!(first[1], want, "编码不符");
        if want != 3 {
            assert_eq!(&first[14..18], &[0, 0, 0, 1], "不是 Annex B 起始码");
        }
        let start = g.keys[0];
        let fps = f64::from(g.frames) / t0.elapsed().saturating_sub(start).as_secs_f64();
        println!("平均 {fps:.1} 帧每秒");
        assert!(fps > 15.0, "帧率过低");
        if let Some(at) = key_at {
            assert!(
                g.keys.iter().any(|t| *t > Duration::from_secs(at)),
                "请求后没有收到新的关键帧"
            );
        }
        let last = states.lock().unwrap().last().cloned().unwrap_or_default();
        assert!(last.contains("Ended { message: None }"), "{last}");
    });
    cdapp_lib::share::bridge::stop();
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
