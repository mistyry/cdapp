//! 接收、播放与混音的真实环境测试：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_TOKEN2=... CDAPP_LIVE_VOICE=...
//!  cargo test --features test-media --test live_playback -- --ignored --nocapture`
//! 两个账号以不同频率的合成音互推，从混音录制中测量主频，不打开扬声器。
//! 验证双向可听、关闭耳机后无声并静音麦克风、恢复后重新可听、
//! 成员离开后不残留声音，以及退出后播放线程全部退出。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::audio::capture::Source;
use cdapp_lib::audio::output::{Sink as Out, live_outputs};
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::voice::{MicPrefs, Phase, Sink, Tap, VoiceEvent, VoiceSession};

type Log = Arc<Mutex<Vec<VoiceEvent>>>;

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

fn start(token: &str, channel: &str, tone: f32) -> (VoiceSession, Log, Tap) {
    let log: Log = Arc::default();
    let tap: Tap = Arc::default();
    let l = log.clone();
    let sink: Sink = Arc::new(move |e| l.lock().unwrap().push(e));
    let api = Api::new(Some(token.to_owned())).unwrap();
    let s = VoiceSession::start_with(
        api,
        channel.to_owned(),
        1,
        sink,
        MicPrefs {
            source: Source::Tone(tone),
            output: Out::Null,
            tap: Some(tap.clone()),
            ..MicPrefs::default()
        },
    );
    (s, log, tap)
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

/// 清空录制缓冲，录制 `secs` 秒混音输出。
async fn record(tap: &Tap, secs: u64) -> Vec<f32> {
    tap.lock().unwrap().clear();
    tokio::time::sleep(Duration::from_secs(secs)).await;
    std::mem::take(&mut *tap.lock().unwrap())
}

#[allow(clippy::cast_precision_loss)]
fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
}

/// 中间 1 秒内 100 至 2000 Hz 的主频，步长 1 Hz。
#[allow(clippy::cast_precision_loss)]
fn peak(x: &[f32]) -> f64 {
    let n = x.len().min(48_000);
    let start = (x.len() - n) / 2;
    let seg = &x[start..start + n];
    let mut best = (0.0, 0.0);
    for f in 100..2000 {
        let w = 2.0 * std::f64::consts::PI * f64::from(f) / 48_000.0;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in seg.iter().enumerate() {
            re += f64::from(v) * (w * i as f64).cos();
            im += f64::from(v) * (w * i as f64).sin();
        }
        let m = re * re + im * im;
        if m > best.1 {
            best = (f64::from(f), m);
        }
    }
    best.0
}

fn speaking(log: &Log) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, VoiceEvent::Speaking { ids } if !ids.is_empty()))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
async fn two_way_playback_deafen_and_cleanup() {
    let (main, alt, ch) = (
        env("CDAPP_LIVE_TOKEN"),
        env("CDAPP_LIVE_TOKEN2"),
        env("CDAPP_LIVE_VOICE"),
    );
    let (a, la, ta) = start(&main, &ch, 523.0);
    assert!(wait_for(&la, 20, connected).await, "主号未连接");
    let (b, lb, tb) = start(&alt, &ch, 660.0);
    assert!(wait_for(&lb, 20, connected).await, "小号未连接");
    let threads = live_outputs();
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 双向收听。
    let (ra, rb) = tokio::join!(record(&ta, 4), record(&tb, 4));
    let (fa, fb) = (peak(&ra), peak(&rb));
    println!(
        "主号收到={fa} Hz rms={:.4} 小号收到={fb} Hz rms={:.4} 播放线程={threads} 说话事件={}",
        rms(&ra),
        rms(&rb),
        speaking(&la)
    );
    assert!((fa - 660.0).abs() <= 2.0, "主号应收到 660 Hz");
    assert!((fb - 523.0).abs() <= 2.0, "小号应收到 523 Hz");

    // 主号关闭耳机。
    a.control().deafen(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let deaf = record(&ta, 3).await;
    let headset = wait_for(
        &lb,
        5,
        |e| matches!(e, VoiceEvent::PeerChanged { patch } if patch.headset_off == Some(true)),
    )
    .await;
    let mic_muted = la
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|e| match e {
            VoiceEvent::Mic { muted, .. } => Some(*muted),
            _ => None,
        })
        .unwrap_or(false);
    println!(
        "关闭耳机后样本数={} rms={:.6} 麦克风静音={mic_muted} 对方看到耳机关闭={headset} 播放线程={}",
        deaf.len(),
        rms(&deaf),
        live_outputs()
    );
    assert!(rms(&deaf) < 1e-6, "关闭耳机后仍有声音");
    assert!(mic_muted, "关闭耳机未静音麦克风");

    // 主号打开耳机，恢复之前的麦克风状态。
    a.control().deafen(false).await.unwrap();
    let headset_back = wait_for(
        &lb,
        5,
        |e| matches!(e, VoiceEvent::PeerChanged { patch } if patch.headset_off == Some(false)),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (ra, rb) = tokio::join!(record(&ta, 3), record(&tb, 3));
    println!(
        "恢复后主号收到={} Hz 小号收到={} Hz 对方看到耳机打开={headset_back}",
        peak(&ra),
        peak(&rb)
    );
    assert!((peak(&ra) - 660.0).abs() <= 2.0);
    assert!(
        (peak(&rb) - 523.0).abs() <= 2.0,
        "恢复后小号未收到主号的声音"
    );

    // 小号离开后不应残留声音。
    b.leave().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let gone = record(&ta, 2).await;
    println!("小号离开后 rms={:.6}", rms(&gone));
    assert!(rms(&gone) < 1e-6, "成员离开后仍有声音");

    // 主号退出，播放线程应全部退出。
    a.leave().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    println!("退出后播放线程={}", live_outputs());
    assert_eq!(live_outputs(), 0);
}

/// 反复打开与关闭默认扬声器，检查线程释放：
/// `cargo test --features test-media --test live_playback device_open_close -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要扬声器设备"]
async fn device_open_close() {
    use cdapp_lib::audio::output::{OutputEvent, Playback, output_devices};
    let devices = output_devices().unwrap();
    println!("输出设备数={}", devices.len());
    let mixer = Arc::default();
    for round in 0..3 {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let p = Playback::start(Out::Device(None), Arc::clone(&mixer), round, tx);
        let started = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        let ok = matches!(started, Ok(Some(OutputEvent::Started { .. })));
        let during = live_outputs();
        p.stop().await;
        println!(
            "第 {round} 轮，打开={ok} 运行中线程={during} 停止后线程={}",
            live_outputs()
        );
        assert!(ok);
        assert_eq!(live_outputs(), 0);
    }
    // 所选设备不存在时直接改用默认设备，不另行通知。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let p = Playback::start(Out::Device(Some("missing:0".into())), mixer, 9, tx);
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    println!(
        "改用默认设备={}",
        matches!(first, Ok(Some(OutputEvent::Started { .. })))
    );
    p.stop().await;
    assert_eq!(live_outputs(), 0);
}
