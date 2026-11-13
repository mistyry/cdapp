#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::golive::{Tier, Vendor};
use cdapp_lib::share::bridge::{self, SourceKind};
#[cfg(feature = "test-media")]
use cdapp_lib::share::fit::fit;
use cdapp_lib::share::sdk::{self, Status};
use cdapp_lib::share::session::{Request, Session, ShareEvent, Sink};

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

#[cfg(feature = "test-media")]
fn freq(key: &str) -> Option<f32> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

#[cfg(feature = "test-media")]
fn play_tone(f: f32) -> cpal::Stream {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let dev = cpal::default_host()
        .default_output_device()
        .expect("没有扬声器");
    let cfg = dev.default_output_config().unwrap();
    assert_eq!(
        cfg.sample_format(),
        cpal::SampleFormat::F32,
        "测试只支持 f32 输出"
    );
    let config: cpal::StreamConfig = cfg.config();
    #[allow(clippy::cast_precision_loss)]
    let (rate, ch) = (config.sample_rate as f32, usize::from(config.channels));
    let mut n = 0u64;
    let stream = dev
        .build_output_stream::<f32, _, _>(
            config,
            move |d: &mut [f32], _| {
                for frame in d.chunks_mut(ch) {
                    #[allow(clippy::cast_precision_loss)]
                    let v = (std::f32::consts::TAU * f * n as f32 / rate).sin() * 0.06;
                    frame.fill(v);
                    n += 1;
                }
            },
            |e| eprintln!("播放失败：{e}"),
            None,
        )
        .unwrap();
    stream.play().unwrap();
    stream
}

#[cfg(feature = "test-media")]
fn db(x: &[f32], f: f32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let (len, w) = (x.len() as f32, std::f32::consts::TAU * f / 48_000.0);
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (i, &v) in x.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let a = w * i as f32;
        re += v * a.cos();
        im -= v * a.sin();
    }
    20.0 * ((re * re + im * im).sqrt() / len + 1e-9).log10()
}

#[cfg(feature = "test-media")]
async fn lifecycle(
    session: &Session,
    secs: u64,
    source: (u32, u32),
    end_voice: tokio::sync::oneshot::Sender<()>,
    log: &Arc<Mutex<Vec<ShareEvent>>>,
) {
    let gap = Duration::from_secs((secs / 4).max(10));
    let t0 = std::time::Instant::now();
    let at = || t0.elapsed().as_secs();
    tokio::time::sleep(gap).await;
    session.renew().await.expect("续期失败");
    println!("步骤 {}s 已续期令牌", at());
    tokio::time::sleep(gap).await;
    let low = Tier {
        resolution: 720,
        frame_rate: 30,
        mode: 0,
    };
    session.update(low).await.expect("改为 720p 失败");
    println!("步骤 {}s 已改为 720p30", at());
    tokio::time::sleep(gap).await;
    let high = Tier {
        resolution: 1080,
        frame_rate: 30,
        mode: 0,
    };
    session.update(high).await.expect("改回 1080p 失败");
    println!("步骤 {}s 已改回 1080p30", at());
    tokio::time::sleep(gap).await;
    let _ = end_voice.send(());
    println!("步骤 {}s 模拟语音结束", at());
    for _ in 0..50 {
        if session.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(session.is_finished(), "语音结束后推流仍在");
    println!("步骤 {}s 推流已随语音结束", at());
    let events = log.lock().unwrap();
    let tiers: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ShareEvent::Tier { width, height, .. } => Some((*width, *height)),
            _ => None,
        })
        .collect();
    let want = [fit(720, source), fit(1080, source)];
    assert_eq!(tiers, want, "{events:?}");
    assert!(
        matches!(events.last(), Some(ShareEvent::Ended { message: Some(m) }) if m.contains("语音")),
        "{events:?}"
    );
}

#[test]
#[ignore = "需要真实账号与语音频道"]
#[allow(clippy::too_many_lines)]
fn pushes_primary_screen() {
    let secs: u64 = std::env::var("CDAPP_LIVE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let audio = std::env::var("CDAPP_LIVE_AUDIO").is_ok_and(|v| v == "1");
    let dir = std::env::temp_dir().join(format!("cdapp-live-share-{}", std::process::id()));
    cdapp_lib::storage::init(&dir.join("fixed"), &dir.join("data"), &dir.join("fixed"));

    #[cfg(feature = "test-media")]
    let pushed: Arc<Mutex<Vec<f32>>> = Arc::default();
    #[cfg(feature = "test-media")]
    {
        let p = pushed.clone();
        *cdapp_lib::share::loopback::TAP.lock().unwrap() = Some(Box::new(move |pcm: &[i16]| {
            let mut v = p.lock().unwrap();
            v.extend(pcm.iter().step_by(2).map(|&s| f32::from(s) / 32768.0));
            let over = v.len().saturating_sub(48_000);
            v.drain(..over);
        }));
    }
    #[cfg(feature = "test-media")]
    let _tone = freq("CDAPP_LIVE_SELF_TONE").map(play_tone);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let s = sdk::sync().await;
        assert!(matches!(s, Status::Ready { .. }), "{s:?}");
        bridge::set_log_dir(&dir.join("sdklog")).unwrap();
        let want_window = std::env::var("CDAPP_LIVE_WINDOW").ok();
        let want = std::env::var("CDAPP_LIVE_SCREEN").ok();
        let screen = bridge::sources()
            .unwrap()
            .into_iter()
            .find(|s| match &want_window {
                Some(t) => s.kind == SourceKind::Window && s.name == *t,
                None => {
                    s.kind == SourceKind::Screen
                        && want.as_deref().map_or(s.primary, |n| s.name == n)
                }
            })
            .expect("找不到共享源");
        println!(
            "源 {:?} {} {}x{} pid={}",
            screen.kind, screen.name, screen.width, screen.height, screen.pid
        );

        let log: Arc<Mutex<Vec<ShareEvent>>> = Arc::default();
        let l = log.clone();
        let sink: Sink = Arc::new(move |e: ShareEvent| {
            if !matches!(e, ShareEvent::Stats { .. }) {
                println!("事件 {e:?}");
            }
            l.lock().unwrap().push(e);
        });
        let (end_voice, voice_end) = tokio::sync::oneshot::channel::<()>();
        let voice = Box::pin(async move {
            let _ = voice_end.await;
        });
        let api = Api::new(Some(env("CDAPP_LIVE_TOKEN"))).unwrap();
        let vendor = api
            .golive_vendor(
                &env("CDAPP_LIVE_GUILD"),
                &env("CDAPP_LIVE_VOICE"),
                &env("CDAPP_LIVE_USER"),
            )
            .await
            .unwrap();
        println!("分享厂商 {vendor}");
        let vendor = Vendor::parse(&vendor).expect("未知分享厂商");
        if vendor == Vendor::Agora {
            cdapp_lib::share::agora_sdk::sync().await;
            assert!(cdapp_lib::share::agora_sdk::ready(), "Agora 组件不可用");
            bridge::agora_log(&dir.join("sdklog").join("agorasdk.log")).unwrap();
        }
        let session = Session::start(
            api,
            Request {
                channel_id: env("CDAPP_LIVE_VOICE"),
                user_id: env("CDAPP_LIVE_USER"),
                source_kind: screen.kind,
                source_id: screen.id,
                source_pid: screen.pid,
                tier: Tier {
                    resolution: 1080,
                    frame_rate: 30,
                    mode: 0,
                },
                audio,
                vendor,
            },
            1,
            sink,
            voice,
        );
        #[cfg(feature = "test-media")]
        if std::env::var("CDAPP_LIVE_LIFECYCLE").is_ok_and(|v| v == "1") {
            lifecycle(
                &session,
                secs,
                (screen.width, screen.height),
                end_voice,
                &log,
            )
            .await;
            return;
        }
        if std::env::var("CDAPP_LIVE_EXPECT_GONE").is_ok_and(|v| v == "1") {
            for _ in 0..secs * 10 {
                if session.is_finished() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let events = log.lock().unwrap();
            assert!(
                matches!(events.last(), Some(ShareEvent::Ended { message: Some(m) }) if m.contains("窗口")),
                "{events:?}"
            );
            println!("推流已随窗口关闭结束");
            drop(end_voice);
            return;
        }
        for _ in 0..secs / 5 {
            tokio::time::sleep(Duration::from_secs(5)).await;
            #[cfg(feature = "test-media")]
            if let (Some(me), Some(other)) =
                (freq("CDAPP_LIVE_SELF_TONE"), freq("CDAPP_LIVE_OTHER_TONE"))
            {
                let x = pushed.lock().unwrap().clone();
                if x.len() == 48_000 {
                    println!(
                        "推出声音 本进程 {me} Hz {:.1} dB，其他进程 {other} Hz {:.1} dB",
                        db(&x, me),
                        db(&x, other)
                    );
                }
            }
        }
        assert!(
            !session.is_finished(),
            "推流提前结束：{:?}",
            log.lock().unwrap()
        );
        session.stop().await;
        drop(end_voice);
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
