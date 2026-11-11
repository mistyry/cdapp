//! 屏幕分享推流的真实环境测试，仅 Windows。需要会员账号已在语音频道中，例如先运行 poc 的 voice.py：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_USER=... CDAPP_LIVE_VOICE=... CDAPP_LIVE_SECS=60
//!  cargo test --features test-media --test live_share -- --ignored --nocapture`
//! 下载并加载 SDK，以 1080p30 推指定显示器，持续指定秒数后停止。不输出令牌。
//!
//! - `CDAPP_LIVE_SCREEN`：按名称选显示器，例如 `\\.\DISPLAY1`，省略时为主显示器。
//! - `CDAPP_LIVE_AUDIO=1`：带屏幕声音，排除本进程。
//! - `CDAPP_LIVE_SELF_TONE=<频率>`：本进程播放正弦波，并测量推出的声音中该频率与
//!   `CDAPP_LIVE_OTHER_TONE` 频率的能量，检验是否排除本进程。
//! - `CDAPP_LIVE_LIFECYCLE=1`：生命周期，需 `test-media`。推流中续期令牌，改为 720p30 再改回 1080p30，
//!   最后模拟语音结束，确认推流随之结束。各步之间间隔 `CDAPP_LIVE_SECS` 的四分之一。
#![cfg(windows)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::golive::Tier;
use cdapp_lib::share::bridge::{self, SourceKind};
#[cfg(feature = "test-media")]
use cdapp_lib::share::fit::fit;
use cdapp_lib::share::sdk::{self, Status};
use cdapp_lib::share::session::{Request, Session, ShareEvent, Sink, VoiceAlive};

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

/// 在默认扬声器播放正弦波，返回的流释放时停止。
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
                    // 振幅约 -24 dBFS，与 poc 的 tone.py 相同。
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

/// 单频能量，dB，按 Goertzel 算法。
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

/// 生命周期：续期、改档两次、模拟语音结束。每步打印时间点，供观看端日志对照。
/// `source` 为显示器尺寸，预期编码尺寸按宽高比换算，不写死 16:9。
#[cfg(feature = "test-media")]
async fn lifecycle(
    session: &Session,
    secs: u64,
    source: (u32, u32),
    in_voice: &AtomicBool,
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
    in_voice.store(false, Ordering::SeqCst);
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

    // 推出的声音最近 1 秒，左声道。
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
            if !matches!(e, ShareEvent::Stats { .. }) {
                println!("事件 {e:?}");
            }
            l.lock().unwrap().push(e);
        });
        // 测试进程没有 cdapp 的语音会话。语音实际由 voice.py 维持，这里用开关模拟其状态。
        let in_voice = Arc::new(AtomicBool::new(true));
        let v = in_voice.clone();
        let voice: VoiceAlive = Arc::new(move || {
            let alive = v.load(Ordering::SeqCst);
            Box::pin(async move { alive })
        });
        let session = Session::start(
            Api::new(Some(env("CDAPP_LIVE_TOKEN"))).unwrap(),
            Request {
                channel_id: env("CDAPP_LIVE_VOICE"),
                user_id: env("CDAPP_LIVE_USER"),
                source_kind: SourceKind::Screen,
                source_id: screen.id,
                source_pid: 0,
                tier: Tier {
                    resolution: 1080,
                    frame_rate: 30,
                    mode: 0,
                },
                audio,
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
                &in_voice,
                &log,
            )
            .await;
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
