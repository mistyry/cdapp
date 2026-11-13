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
