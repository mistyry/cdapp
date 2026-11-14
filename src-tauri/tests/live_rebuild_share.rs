#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cdapp_lib::audio::capture::Source;
use cdapp_lib::audio::output::Sink as Out;
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::golive::{Tier, Vendor};
use cdapp_lib::kook::voice::{Fault, MicPrefs, Phase, Sink, VoiceEvent, VoiceSession};
use cdapp_lib::share::bridge::{SourceKind, volc};
use cdapp_lib::share::push::{Request, Session, ShareEvent};
use cdapp_lib::share::volc_sdk::{self, Status};

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

fn connected(log: &Mutex<Vec<VoiceEvent>>) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                e,
                VoiceEvent::State {
                    phase: Phase::Connected,
                    ..
                }
            )
        })
        .count()
}

async fn wait(secs: u64, f: impl Fn() -> bool) -> bool {
    for _ in 0..secs * 10 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn rebuilds(log: &Mutex<Vec<VoiceEvent>>) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                e,
                VoiceEvent::State {
                    phase: Phase::Reconnecting,
                    ..
                }
            )
        })
        .count()
}

fn party(
    token: &str,
    channel: &str,
    tag: &'static str,
    gain: u16,
) -> (VoiceSession, Arc<Mutex<Vec<VoiceEvent>>>) {
    let log: Arc<Mutex<Vec<VoiceEvent>>> = Arc::default();
    let l = log.clone();
    let sink: Sink = Arc::new(move |e: VoiceEvent| {
        if let VoiceEvent::State { phase, message, .. } = &e {
            println!("{tag} 语音 {phase:?} {message:?}");
        }
        l.lock().unwrap().push(e);
    });
    let g = cdapp_lib::audio::dsp::Gain::default();
    g.set(gain);
    let s = VoiceSession::start_with(
        Api::new(Some(token.to_owned())).unwrap(),
        channel.to_owned(),
        1,
        sink,
        MicPrefs {
            source: Source::Tone(440.0),
            gain: g,
            output: Out::Null,
            ..MicPrefs::default()
        },
    );
    (s, log)
}

#[test]
#[ignore = "需要真实账号与语音频道"]
fn quiet_call_with_share_does_not_rebuild() {
    let minutes: u64 = std::env::var("CDAPP_QUIET_MINUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let dir = std::env::temp_dir().join(format!("cdapp-quiet-{}", std::process::id()));
    cdapp_lib::storage::init(&dir.join("fixed"), &dir.join("data"), &dir.join("fixed"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let s = volc_sdk::sync().await;
        assert!(matches!(s, Status::Ready { .. }), "{s:?}");
        let channel = env("CDAPP_LIVE_VOICE");
        let user = env("CDAPP_LIVE_USER");
        let (a, alog) = party(&env("CDAPP_LIVE_TOKEN"), &channel, "分享者", 100);
        assert!(wait(20, || connected(&alog) >= 1).await, "分享者语音未连接");
        let (b, blog) = party(&env("CDAPP_LIVE_TOKEN2"), &channel, "静音者", 0);
        assert!(wait(20, || connected(&blog) >= 1).await, "静音者语音未连接");
        let api = Api::new(Some(env("CDAPP_LIVE_TOKEN"))).unwrap();
        let vendor = Vendor::parse(
            &api.golive_vendor(&env("CDAPP_LIVE_GUILD"), &channel, &user)
                .await
                .unwrap(),
        )
        .unwrap();
        if vendor == Vendor::Agora {
            cdapp_lib::share::agora_sdk::sync().await;
        }
        let screen = volc::sources()
            .unwrap()
            .into_iter()
            .find(|s| s.kind == SourceKind::Screen && s.primary)
            .unwrap();
        let ended = a.ended();
        let share = Session::start(
            api,
            Request {
                channel_id: channel.clone(),
                user_id: user,
                source_kind: screen.kind,
                source_id: screen.id,
                source_pid: screen.pid,
                tier: Tier {
                    resolution: 720,
                    frame_rate: 30,
                    mode: 0,
                },
                audio: false,
                vendor,
            },
            1,
            Arc::new(|e: ShareEvent| {
                if !matches!(e, ShareEvent::Stats { .. }) {
                    println!("分享 {e:?}");
                }
            }),
            Box::pin(ended.wait()),
            a.rebuilt(),
        );
        let t0 = Instant::now();
        for m in 1..=minutes {
            tokio::time::sleep(Duration::from_secs(60)).await;
            println!(
                "{m} 分钟 分享者重建 {} 次 静音者重建 {} 次 分享仍在={}",
                rebuilds(&alog),
                rebuilds(&blog),
                !share.is_finished()
            );
        }
        let (ra, rb, alive) = (rebuilds(&alog), rebuilds(&blog), !share.is_finished());
        println!(
            "结果 {} 秒 分享者重建 {ra} 次 静音者重建 {rb} 次 分享仍在={alive}",
            t0.elapsed().as_secs()
        );
        share.stop().await;
        a.leave().await;
        b.leave().await;
        volc::stop();
        assert!(ra == 0 && rb == 0 && alive);
    });
}

#[test]
#[ignore = "需要真实账号、语音频道与另一位分享者"]
fn watch_survives_voice_rebuild() {
    use cdapp_lib::share::watch::{
        Request as WatchRequest, Session as Watch, StateSink, WatchState,
    };
    let dir = std::env::temp_dir().join(format!("cdapp-rebuild-w-{}", std::process::id()));
    cdapp_lib::storage::init(&dir.join("fixed"), &dir.join("data"), &dir.join("fixed"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let s = volc_sdk::sync().await;
        assert!(matches!(s, Status::Ready { .. }), "{s:?}");
        let (token, channel, user, liver) = (
            env("CDAPP_LIVE_TOKEN"),
            env("CDAPP_LIVE_VOICE"),
            env("CDAPP_LIVE_USER"),
            env("CDAPP_LIVE_LIVER"),
        );
        let vlog: Arc<Mutex<Vec<VoiceEvent>>> = Arc::default();
        let l = vlog.clone();
        let vsink: Sink = Arc::new(move |e: VoiceEvent| {
            if let VoiceEvent::State { phase, message, .. } = &e {
                println!("语音 {phase:?} {message:?}");
            }
            l.lock().unwrap().push(e);
        });
        let voice = VoiceSession::start_with(
            Api::new(Some(token.clone())).unwrap(),
            channel.clone(),
            1,
            vsink,
            MicPrefs {
                source: Source::Tone(440.0),
                output: Out::Null,
                ..MicPrefs::default()
            },
        );
        assert!(wait(20, || connected(&vlog) >= 1).await, "语音未连接");
        let t0 = Instant::now();
        let states: Arc<Mutex<Vec<(Instant, String)>>> = Arc::default();
        let st = states.clone();
        let sink: StateSink = Arc::new(move |s: WatchState| {
            if !matches!(s, WatchState::Stats { .. }) {
                println!("{:>6.1}s 观看 {s:?}", t0.elapsed().as_secs_f64());
            }
            st.lock().unwrap().push((Instant::now(), format!("{s:?}")));
        });
        let frames: Arc<Mutex<Vec<Instant>>> = Arc::default();
        let fr = frames.clone();
        let ended = voice.ended();
        let watch = Watch::start(
            Api::new(Some(token.clone())).unwrap(),
            WatchRequest {
                channel_id: channel,
                user_id: user,
                liver,
                volume: 100,
            },
            1,
            sink,
            Box::new(move |_| {
                fr.lock().unwrap().push(Instant::now());
                Ok(())
            }),
            Box::pin(ended.wait()),
            voice.rebuilt(),
        );
        assert!(
            wait(30, || frames.lock().unwrap().len() > 30).await,
            "观看未收到画面"
        );
        let before = connected(&vlog);
        let at = Instant::now();
        println!("{:>6.1}s 注入 Sleep", t0.elapsed().as_secs_f64());
        voice.control().inject(Fault::Sleep);
        let back = wait(40, || connected(&vlog) > before).await;
        tokio::time::sleep(Duration::from_secs(25)).await;
        let f = frames.lock().unwrap().clone();
        let after = f.iter().filter(|t| **t > at).count();
        let gap = f
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_millis())
            .max();
        let ended_msg = states
            .lock()
            .unwrap()
            .iter()
            .find(|(_, s)| s.starts_with("Ended"))
            .map(|(t, s)| (t.duration_since(at).as_millis(), s.clone()));
        println!(
            "结果 语音重建={back} 观看结束={ended_msg:?} 重建后帧 {after} 最长帧间隔 {gap:?}ms 观看仍在={}",
            !watch.is_finished()
        );
        let alive = !watch.is_finished() && after > 100;
        watch.stop().await;
        voice.leave().await;
        volc::stop();
        assert!(back && alive, "语音重建后观看未恢复");
    });
}

#[test]
#[ignore = "需要真实账号与语音频道"]
#[allow(clippy::too_many_lines)]
fn share_survives_voice_rebuild() {
    let fault = match std::env::var("CDAPP_REBUILD_FAULT").as_deref() {
        Ok("close") => Fault::CloseSignal,
        _ => Fault::Sleep,
    };
    let dir = std::env::temp_dir().join(format!("cdapp-rebuild-{}", std::process::id()));
    cdapp_lib::storage::init(&dir.join("fixed"), &dir.join("data"), &dir.join("fixed"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let s = volc_sdk::sync().await;
        assert!(matches!(s, Status::Ready { .. }), "{s:?}");
        volc::log(&dir.join("sdklog")).unwrap();
        let screen = volc::sources()
            .unwrap()
            .into_iter()
            .find(|s| s.kind == SourceKind::Screen && s.primary)
            .expect("找不到主屏幕");
        let (token, channel, user) = (
            env("CDAPP_LIVE_TOKEN"),
            env("CDAPP_LIVE_VOICE"),
            env("CDAPP_LIVE_USER"),
        );
        let vlog: Arc<Mutex<Vec<VoiceEvent>>> = Arc::default();
        let l = vlog.clone();
        let vsink: Sink = Arc::new(move |e: VoiceEvent| {
            if let VoiceEvent::State { phase, message, .. } = &e {
                println!("语音 {phase:?} {message:?}");
            }
            l.lock().unwrap().push(e);
        });
        let voice = VoiceSession::start_with(
            Api::new(Some(token.clone())).unwrap(),
            channel.clone(),
            1,
            vsink,
            MicPrefs {
                source: Source::Tone(440.0),
                output: Out::Null,
                ..MicPrefs::default()
            },
        );
        assert!(wait(20, || connected(&vlog) >= 1).await, "语音未连接");

        let api = Api::new(Some(token.clone())).unwrap();
        let vendor = api
            .golive_vendor(&env("CDAPP_LIVE_GUILD"), &channel, &user)
            .await
            .unwrap();
        let vendor = Vendor::parse(&vendor).expect("未知分享厂商");
        println!("分享厂商 {vendor:?}");
        if vendor == Vendor::Agora {
            cdapp_lib::share::agora_sdk::sync().await;
            assert!(cdapp_lib::share::agora_sdk::ready());
        }
        let slog: Arc<Mutex<Vec<(Instant, ShareEvent)>>> = Arc::default();
        let l = slog.clone();
        let t0 = Instant::now();
        let ssink: cdapp_lib::share::push::Sink = Arc::new(move |e: ShareEvent| {
            if !matches!(e, ShareEvent::Stats { .. }) {
                println!("{:>6.1}s 分享 {e:?}", t0.elapsed().as_secs_f64());
            }
            l.lock().unwrap().push((Instant::now(), e));
        });
        let ended = voice.ended();
        let share = Session::start(
            api,
            Request {
                channel_id: channel.clone(),
                user_id: user.clone(),
                source_kind: screen.kind,
                source_id: screen.id,
                source_pid: screen.pid,
                tier: Tier {
                    resolution: 720,
                    frame_rate: 30,
                    mode: 0,
                },
                audio: false,
                vendor,
            },
            1,
            ssink,
            Box::pin(ended.wait()),
            voice.rebuilt(),
        );
        let live = || {
            slog.lock()
                .unwrap()
                .iter()
                .any(|(_, e)| matches!(e, ShareEvent::Live { .. }))
        };
        assert!(wait(30, live).await, "分享未开始");
        tokio::time::sleep(Duration::from_secs(10)).await;

        let before = connected(&vlog);
        let at = Instant::now();
        println!("{:>6.1}s 注入 {fault:?}", t0.elapsed().as_secs_f64());
        voice.control().inject(fault);
        let back = wait(40, || connected(&vlog) > before).await;
        println!(
            "{:>6.1}s 语音重建完成={back} 用时 {} ms",
            t0.elapsed().as_secs_f64(),
            at.elapsed().as_millis()
        );
        tokio::time::sleep(Duration::from_secs(20)).await;
        let log = slog.lock().unwrap().clone();
        let stats_after = log
            .iter()
            .filter(|(t, e)| *t > at && matches!(e, ShareEvent::Stats { .. }))
            .count();
        let ended_msg = log.iter().find_map(|(t, e)| match e {
            ShareEvent::Ended { message } => Some((t.duration_since(at).as_millis(), message.clone())),
            _ => None,
        });
        let relive = log.iter().find_map(|(t, e)| {
            (*t > at && matches!(e, ShareEvent::Live { .. }))
                .then(|| t.duration_since(at).as_millis())
        });
        let last_stat_gap = log
            .iter()
            .filter(|(_, e)| matches!(e, ShareEvent::Stats { .. }))
            .map(|(t, _)| *t)
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_millis())
            .max();
        println!(
            "结果 分享结束={ended_msg:?} 重新开播={relive:?}ms 重建后统计 {stats_after} 条 最长统计间隔 {last_stat_gap:?}ms 分享仍在={}",
            !share.is_finished()
        );
        let survived = !share.is_finished() && stats_after > 5;
        share.stop().await;
        voice.leave().await;
        volc::stop();
        assert!(back, "语音未重建");
        assert!(survived, "语音重建后分享结束：{ended_msg:?}");
    });
}
