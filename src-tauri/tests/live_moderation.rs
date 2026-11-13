use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::audio::capture::Source;
use cdapp_lib::audio::output::Sink as Out;
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::voice::{EndReason, MicPrefs, Phase, Sink, VoiceEvent, VoiceSession};

type Log = Arc<Mutex<Vec<VoiceEvent>>>;

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

fn join(token: &str, channel: &str) -> (VoiceSession, Log) {
    let log: Log = Arc::default();
    let l = log.clone();
    let sink: Sink = Arc::new(move |e| l.lock().unwrap().push(e));
    let api = Api::new(Some(token.to_owned())).unwrap();
    let s = VoiceSession::start_with(
        api,
        channel.to_owned(),
        1,
        sink,
        MicPrefs {
            source: Source::Tone(440.0),
            output: Out::Null,
            ..MicPrefs::default()
        },
    );
    (s, log)
}

async fn wait(log: &Log, secs: u64, f: impl Fn(&VoiceEvent) -> bool) -> bool {
    for _ in 0..secs * 10 {
        if log.lock().unwrap().iter().any(&f) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn me_patch(
    e: &VoiceEvent,
    me: &str,
    f: impl Fn(&cdapp_lib::kook::voice::PeerPatch) -> bool,
) -> bool {
    matches!(e, VoiceEvent::PeerChanged { patch } if patch.id == me && f(patch))
}

#[tokio::test]
#[ignore = "需要两个真实账号"]
#[allow(clippy::too_many_lines)]
async fn server_mute_deafen_move_kick() {
    let (main, alt) = (env("CDAPP_LIVE_TOKEN"), env("CDAPP_LIVE_TOKEN2"));
    let (guild, voice, voice2) = (
        env("CDAPP_LIVE_GUILD"),
        env("CDAPP_LIVE_VOICE"),
        env("CDAPP_LIVE_VOICE2"),
    );
    let m = Api::new(Some(main)).unwrap();
    let me = Api::new(Some(alt.clone())).unwrap().me().await.unwrap();
    let alt_id = me["id"]
        .as_str()
        .map_or_else(|| me["id"].to_string(), str::to_owned);

    let (s, log) = join(&alt, &voice);
    assert!(
        wait(&log, 20, |e| matches!(
            e,
            VoiceEvent::State {
                phase: Phase::Connected,
                ..
            }
        ))
        .await,
        "副号未连接"
    );

    m.guild_mute(&guild, &alt_id, 1, true).await.unwrap();
    assert!(
        wait(&log, 8, |e| me_patch(e, &alt_id, |p| p.server_mute
            == Some(true)))
        .await,
        "未收到闭麦"
    );
    let (mutes, _) = m.guild_mutes(&guild).await.unwrap();
    assert!(mutes.contains(&alt_id));
    m.guild_mute(&guild, &alt_id, 1, false).await.unwrap();
    assert!(
        wait(&log, 8, |e| me_patch(e, &alt_id, |p| p.server_mute
            == Some(false)))
        .await,
        "未收到取消闭麦"
    );
    println!("闭麦与取消通过");

    m.guild_mute(&guild, &alt_id, 2, true).await.unwrap();
    assert!(
        wait(&log, 8, |e| me_patch(e, &alt_id, |p| p.server_deafen
            == Some(true)))
        .await,
        "未收到静音"
    );
    let (_, deaf) = m.guild_mutes(&guild).await.unwrap();
    assert!(deaf.contains(&alt_id));
    m.guild_mute(&guild, &alt_id, 2, false).await.unwrap();
    assert!(
        wait(&log, 8, |e| me_patch(e, &alt_id, |p| p.server_deafen
            == Some(false)))
        .await,
        "未收到取消静音"
    );
    let (a, b) = m.guild_mutes(&guild).await.unwrap();
    assert!(!a.contains(&alt_id) && !b.contains(&alt_id));
    println!("静音与取消通过");

    m.voice_move(&voice2, &alt_id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!s.is_finished(), "移动后会话不应结束");
    println!("移动请求通过，会话保持");

    m.voice_kick(&voice2, &alt_id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!s.is_finished(), "踢出其他频道不应影响");
    m.voice_kick(&voice, &alt_id).await.unwrap();
    assert!(
        wait(&log, 10, |e| matches!(
            e,
            VoiceEvent::State {
                phase: Phase::Disconnected,
                reason: Some(EndReason::Kicked),
                ..
            }
        ))
        .await,
        "未以 kicked 结束: {:?}",
        log.lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, VoiceEvent::State { .. }))
            .collect::<Vec<_>>()
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    let reconnecting = log
        .lock()
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
        .count();
    assert_eq!(reconnecting, 0, "被踢出后不应重连");
    println!("踢出通过");
    s.leave().await;
}
