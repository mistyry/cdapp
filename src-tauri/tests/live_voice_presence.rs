//! 语音频道在线成员的进出对照（真实账号）。默认忽略：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_VOICE=... CDAPP_LIVE_HOLD=15
//!  cargo test --features test-media --test live_voice_presence -- --ignored --nocapture`
//! 以合成静音加入语音频道，停留指定秒数后离开，供另一账号观察进出显示与事件。

use std::sync::Arc;
use std::time::Duration;

use cdapp_lib::audio::capture::Source;
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::voice::{MicPrefs, Sink, VoiceEvent, VoiceSession};

#[tokio::test]
#[ignore = "需要真实账号"]
async fn join_hold_leave() {
    let token = std::env::var("CDAPP_LIVE_TOKEN").expect("CDAPP_LIVE_TOKEN");
    let channel = std::env::var("CDAPP_LIVE_VOICE").expect("CDAPP_LIVE_VOICE");
    let hold: u64 = std::env::var("CDAPP_LIVE_HOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let sink: Sink = Arc::new(|e| match e {
        VoiceEvent::State { phase, reason, .. } => println!("state {phase:?} {reason:?}"),
        VoiceEvent::Joined { peers, .. } => println!("joined peers={}", peers.len()),
        _ => {}
    });
    let session = VoiceSession::start_with(
        Api::new(Some(token)).unwrap(),
        channel,
        1,
        sink,
        MicPrefs {
            muted: true,
            source: Source::Tone(440.0),
            output: cdapp_lib::audio::output::Sink::Null,
            ..MicPrefs::default()
        },
    );
    tokio::time::sleep(Duration::from_secs(hold)).await;
    session.leave().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("left");
}
