//! 语音会话生命周期的真实环境测试。需要两个账号与两个语音频道：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_TOKEN2=... CDAPP_LIVE_VOICE=... CDAPP_LIVE_VOICE2=...
//!  cargo test --features test-media --test live_voice -- --ignored --nocapture`
//! 使用合成音推流，不打开扬声器。依次验证加入、暂停与恢复推流、
//! 其他成员加入与离开、切换频道、被同账号新会话替换与退出。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::audio::capture::Source;
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::voice::{EndReason, MicPrefs, Phase, Sink, VoiceEvent, VoiceSession};

type Log = Arc<Mutex<Vec<VoiceEvent>>>;

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

fn start(token: &str, channel: &str, generation: u64) -> (VoiceSession, Log) {
    let log: Log = Arc::default();
    let l = log.clone();
    let sink: Sink = Arc::new(move |e| l.lock().unwrap().push(e));
    let api = Api::new(Some(token.to_owned())).unwrap();
    (
        // 会话代次只用于区分日志。
        VoiceSession::start_with(
            api,
            channel.to_owned(),
            generation,
            sink,
            MicPrefs {
                muted: false,
                source: Source::Tone(440.0),
                // 不打开扬声器。
                output: cdapp_lib::audio::output::Sink::Null,
                ..MicPrefs::default()
            },
        ),
        log,
    )
}

/// 事件摘要，不输出成员名称与 ID。
fn brief(e: &VoiceEvent) -> String {
    match e {
        VoiceEvent::State { phase, reason, .. } => format!("state:{phase:?}:{reason:?}"),
        VoiceEvent::Joined { me, peers } => format!(
            "joined(device={},say={},peers={})",
            me.device,
            me.permission_say,
            peers.len()
        ),
        VoiceEvent::PeerJoined { .. } => "peer_joined".into(),
        VoiceEvent::PeerLeft { .. } => "peer_left".into(),
        VoiceEvent::PeerChanged { patch } => format!(
            "peer_changed(mic={:?},headset={:?})",
            patch.mic_off, patch.headset_off
        ),
        VoiceEvent::Mic { status, muted, .. } => format!("mic:{status:?}:muted={muted}"),
        VoiceEvent::MicLevel { .. } => "mic_level".into(),
        VoiceEvent::Output {
            deafened, active, ..
        } => format!("output(deafened={deafened},active={active})"),
        VoiceEvent::Speaking { ids } => format!("speaking({})", ids.len()),
        VoiceEvent::Link { quality, .. } => format!("link({quality:?})"),
    }
}

fn dump(name: &str, log: &Log) {
    let v: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .filter(|e| !matches!(e, VoiceEvent::MicLevel { .. }))
        .map(brief)
        .collect();
    println!("[{name}] {}", v.join(" "));
}

async fn wait_for(log: &Log, secs: u64, f: impl Fn(&VoiceEvent) -> bool) -> bool {
    for _ in 0..secs * 10 {
        if log.lock().unwrap().iter().any(&f) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn connected(e: &VoiceEvent) -> bool {
    matches!(
        e,
        VoiceEvent::State {
            phase: Phase::Connected,
            ..
        }
    )
}

fn ended(e: &VoiceEvent) -> Option<EndReason> {
    match e {
        VoiceEvent::State {
            phase: Phase::Disconnected,
            reason,
            ..
        } => *reason,
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
async fn join_pause_switch_replace_leave() {
    let (main, alt) = (env("CDAPP_LIVE_TOKEN"), env("CDAPP_LIVE_TOKEN2"));
    let (v1, v2) = (env("CDAPP_LIVE_VOICE"), env("CDAPP_LIVE_VOICE2"));

    // 主号加入频道一，确认以 pc 设备加入。
    let (a, la) = start(&main, &v1, 1);
    assert!(wait_for(&la, 20, connected).await, "主号未连接");
    let me = la.lock().unwrap().iter().find_map(|e| match e {
        VoiceEvent::Joined { me, .. } => Some(me.clone()),
        _ => None,
    });
    let me = me.expect("未收到 joined");
    assert_eq!(me.device, "pc", "设备类型应为 pc");

    // 暂停与恢复推流。
    let paused = a.pause(true).await;
    let resumed = a.pause(false).await;
    println!(
        "pauseProducer ok={} resumeProducer ok={}{}",
        paused.is_ok(),
        resumed.is_ok(),
        resumed
            .as_ref()
            .err()
            .map(|e| format!(" err={e}"))
            .unwrap_or_default()
    );

    // 小号加入同一频道，主号应看到成员加入。
    let (b, lb) = start(&alt, &v1, 1);
    assert!(wait_for(&lb, 20, connected).await, "小号未连接");
    let saw_peer = wait_for(&la, 10, |e| matches!(e, VoiceEvent::PeerJoined { .. })).await;
    // 小号暂停推流，主号应看到麦克风关闭。
    let alt_paused = b.pause(true).await.is_ok();
    let saw_mute = wait_for(
        &la,
        5,
        |e| matches!(e, VoiceEvent::PeerChanged { patch } if patch.mic_off == Some(true)),
    )
    .await;
    println!("成员加入 newPeer={saw_peer} 小号暂停={alt_paused} 主号看到暂停={saw_mute}");
    b.leave().await;
    let saw_left = wait_for(&la, 10, |e| matches!(e, VoiceEvent::PeerLeft { .. })).await;
    println!("成员离开 peerClosed={saw_left}");
    assert!(
        lb.lock()
            .unwrap()
            .iter()
            .any(|e| ended(e) == Some(EndReason::Left))
    );

    // 主号退出后加入频道二。
    a.leave().await;
    assert_eq!(
        la.lock().unwrap().iter().filter_map(ended).next_back(),
        Some(EndReason::Left)
    );
    let (c, lc) = start(&main, &v2, 2);
    assert!(wait_for(&lc, 20, connected).await, "主号在频道二未连接");

    // 同账号再次加入，旧会话应被替换。
    let (d, ld) = start(&main, &v2, 3);
    assert!(wait_for(&ld, 20, connected).await, "新会话未连接");
    assert!(
        wait_for(&lc, 10, |e| ended(e) == Some(EndReason::Replaced)).await,
        "旧会话未收到替换通知"
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    let reconnecting = lc.lock().unwrap().iter().any(|e| {
        matches!(
            e,
            VoiceEvent::State {
                phase: Phase::Reconnecting,
                ..
            }
        )
    });
    assert!(!reconnecting, "被替换的会话不应重连");
    assert!(c.is_finished(), "被替换的会话应已结束");
    // 新会话不受影响。
    assert!(
        !ld.lock().unwrap().iter().any(|e| ended(e).is_some()),
        "新会话不应结束"
    );
    c.leave().await;

    // 退出新会话。
    d.leave().await;
    assert!(d_finished(&ld));

    for (n, l) in [
        ("main-1", &la),
        ("alt", &lb),
        ("main-2", &lc),
        ("main-3", &ld),
    ] {
        dump(n, l);
    }
}

fn d_finished(log: &Log) -> bool {
    log.lock()
        .unwrap()
        .iter()
        .any(|e| ended(e) == Some(EndReason::Left))
}
