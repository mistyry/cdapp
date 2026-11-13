use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::audio::capture::{Source, live_captures};
use cdapp_lib::audio::mic::MicStatus;
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::voice::{EndReason, MicPrefs, Phase, Sink, VoiceEvent, VoiceSession};

type Log = Arc<Mutex<Vec<(std::time::Instant, VoiceEvent)>>>;

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

fn brief(e: &VoiceEvent) -> Option<String> {
    match e {
        VoiceEvent::State { phase, reason, .. } => Some(format!("state:{phase:?}:{reason:?}")),
        VoiceEvent::Joined { me, peers } => Some(format!(
            "joined(device={},say={},peers={})",
            me.device,
            me.permission_say,
            peers.len()
        )),
        VoiceEvent::Mic {
            status,
            muted,
            message,
            ..
        } => Some(format!(
            "mic:{status:?}:muted={muted}{}",
            if message.is_some() { "::有说明" } else { "" }
        )),
        _ => None,
    }
}

async fn wait_for(log: &Log, secs: u64, f: impl Fn(&VoiceEvent) -> bool) -> bool {
    for _ in 0..secs * 10 {
        if log.lock().unwrap().iter().any(|(_, e)| f(e)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn levels(log: &Log, from: std::time::Instant, to: std::time::Instant) -> Vec<u8> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| *t >= from && *t < to)
        .filter_map(|(_, e)| match e {
            VoiceEvent::MicLevel { level } => Some(*level),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
#[allow(clippy::too_many_lines)]
async fn push_mute_resume_leave() {
    let token = env("CDAPP_LIVE_TOKEN");
    let channel = env("CDAPP_LIVE_VOICE");
    let device = std::env::var("CDAPP_LIVE_MIC").is_ok_and(|v| v == "device");
    let source = if device {
        Source::Device(None)
    } else {
        Source::Tone(523.0)
    };
    let log: Log = Arc::default();
    let l = log.clone();
    let sink: Sink = Arc::new(move |e| l.lock().unwrap().push((std::time::Instant::now(), e)));
    let api = Api::new(Some(token)).unwrap();
    let t0 = std::time::Instant::now();
    let session = VoiceSession::start_with(
        api,
        channel,
        1,
        sink,
        MicPrefs {
            muted: false,
            source,
            output: cdapp_lib::audio::output::Sink::Null,
            ..MicPrefs::default()
        },
    );
    let active = |e: &VoiceEvent| {
        matches!(
            e,
            VoiceEvent::Mic {
                status: MicStatus::Active,
                ..
            }
        )
    };
    assert!(
        wait_for(&log, 20, |e| matches!(
            e,
            VoiceEvent::State {
                phase: Phase::Connected,
                ..
            }
        ))
        .await,
        "未连接"
    );
    let started = wait_for(&log, 5, active).await;
    println!(
        "连接用时 {:.1}s 开始采集={started} 采集线程={}",
        t0.elapsed().as_secs_f32(),
        live_captures()
    );

    tokio::time::sleep(Duration::from_secs(12).saturating_sub(t0.elapsed())).await;
    let t_mute = std::time::Instant::now();
    let muted = session.pause(true).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("静音 ok={} 采集线程={}", muted.is_ok(), live_captures());
    let threads_muted = live_captures();

    tokio::time::sleep(Duration::from_secs(22).saturating_sub(t0.elapsed())).await;
    let t_resume = std::time::Instant::now();
    let resumed = session.pause(false).await;
    println!(
        "取消静音 ok={} 采集线程={}",
        resumed.is_ok(),
        live_captures()
    );
    tokio::time::sleep(Duration::from_secs(30).saturating_sub(t0.elapsed())).await;
    let t_leave = std::time::Instant::now();
    session.leave().await;
    let threads_after = live_captures();

    let pushed = levels(&log, t0, t_mute);
    let silent = levels(&log, t_mute + Duration::from_millis(300), t_resume);
    let again = levels(&log, t_resume, t_leave);
    println!(
        "推流电平={} 中位电平={} 静音期间电平={} 恢复后电平={} 退出后采集线程={threads_after}",
        pushed.len(),
        pushed.get(pushed.len() / 2).copied().unwrap_or(0),
        silent.len(),
        again.len()
    );
    let events: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, e)| brief(e))
        .collect();
    println!("[main] {}", events.join(" "));
    assert!(started, "未开始采集");
    assert!(muted.is_ok() && resumed.is_ok());
    assert_eq!(threads_muted, 0, "静音后采集线程未退出");
    assert_eq!(threads_after, 0, "退出后采集线程未退出");
    assert!(silent.is_empty(), "静音期间仍有电平上报");
    assert!(!again.is_empty(), "取消静音后没有电平上报");
    assert!(log.lock().unwrap().iter().any(|(_, e)| matches!(
        e,
        VoiceEvent::State {
            reason: Some(EndReason::Left),
            ..
        }
    )));
}
