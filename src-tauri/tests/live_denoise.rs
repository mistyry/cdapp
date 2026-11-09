//! 降噪的真实环境测试：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_TOKEN2=... CDAPP_LIVE_VOICE=...
//!  cargo test --features test-media --test live_denoise -- --ignored --nocapture`
//! 主号推送白噪声加 440 Hz 正弦音，依次关闭降噪、使用 RNNoise、使用 KOOK AI 降噪 2.0，
//! 小号录制混音输出，比较各阶段的能量。不打开麦克风与扬声器。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdapp_lib::audio::capture::Source;
use cdapp_lib::audio::denoise::{Denoise, Mode};
use cdapp_lib::audio::output::Sink as Out;
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::voice::{MicPrefs, Phase, Sink, Tap, VoiceEvent, VoiceSession};

type Log = Arc<Mutex<Vec<VoiceEvent>>>;

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

fn start(token: &str, channel: &str, prefs: MicPrefs) -> (VoiceSession, Log) {
    let log: Log = Arc::default();
    let l = log.clone();
    let sink: Sink = Arc::new(move |e| l.lock().unwrap().push(e));
    let api = Api::new(Some(token.to_owned())).unwrap();
    (
        VoiceSession::start_with(api, channel.to_owned(), 1, sink, prefs),
        log,
    )
}

async fn connected(log: &Log) -> bool {
    for _ in 0..200 {
        if log.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                VoiceEvent::State {
                    phase: Phase::Connected,
                    ..
                }
            )
        }) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

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
    (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

/// 指定频率处的幅度，取中间 1 秒。
#[allow(clippy::many_single_char_names)]
#[allow(clippy::cast_precision_loss)]
fn tone(x: &[f32], f: f64) -> f64 {
    let n = x.len().min(48_000);
    let s = &x[(x.len() - n) / 2..][..n];
    let w = 2.0 * std::f64::consts::PI * f / 48_000.0;
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in s.iter().enumerate() {
        re += f64::from(v) * (w * i as f64).cos();
        im += f64::from(v) * (w * i as f64).sin();
    }
    2.0 * (re * re + im * im).sqrt() / n as f64
}

fn levels(log: &Log) -> Vec<u8> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            VoiceEvent::MicLevel { level } => Some(*level),
            _ => None,
        })
        .collect()
}

/// 录制 `secs` 秒内上报的推流电平，返回平均值。电平取自编码前的最终帧，0 至 100。
#[allow(clippy::cast_precision_loss)]
async fn level_over(log: &Log, secs: u64) -> f64 {
    let n0 = levels(log).len();
    tokio::time::sleep(Duration::from_secs(secs)).await;
    let l = levels(log);
    let v = &l[n0..];
    if v.is_empty() {
        return -1.0;
    }
    v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64
}

/// 单账号：只用 `CDAPP_LIVE_TOKEN` 加入，比较推流电平。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
async fn denoise_on_uplink() {
    let (token, ch) = (env("CDAPP_LIVE_TOKEN"), env("CDAPP_LIVE_VOICE"));
    let api = Api::new(Some(token.clone())).unwrap();
    cdapp_lib::kook::denoise_asset::ensure_license(&api)
        .await
        .unwrap();
    let wasm = std::fs::read(env("DENOISE_WASM")).unwrap();
    cdapp_lib::audio::denoise::prepare(&wasm).unwrap();

    let denoise = Denoise::default();
    let (a, la) = start(
        &token,
        &ch,
        MicPrefs {
            source: Source::Noisy(440.0),
            denoise: denoise.clone(),
            output: Out::Null,
            ..MicPrefs::default()
        },
    );
    assert!(connected(&la).await, "未连接");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let off = level_over(&la, 4).await;
    denoise.set_mode(Mode::Rnnoise);
    denoise.set(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let rnn = level_over(&la, 4).await;
    denoise.set_mode(Mode::Kook);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let kook = level_over(&la, 4).await;
    // 持续 12 秒以上，确认许可实时有效。
    tokio::time::sleep(Duration::from_secs(6)).await;
    let late = level_over(&la, 4).await;
    denoise.set(false);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let back = level_over(&la, 3).await;
    println!(
        "推流电平 关闭 {off:.1} RNNoise {rnn:.1} KOOK {kook:.1} KOOK12秒后 {late:.1} 再关闭 {back:.1}"
    );
    let ended = la.lock().unwrap().iter().any(|e| {
        matches!(
            e,
            VoiceEvent::State {
                phase: Phase::Disconnected,
                ..
            }
        )
    });
    a.leave().await;
    assert!(!ended, "会话失败");
    assert!(off > 30.0, "关闭降噪时应有电平");
    assert!(rnn < off - 5.0, "RNNoise 应降低电平");
    assert!(kook < off - 10.0, "KOOK 降噪应降低电平");
    assert!(late < off - 10.0, "KOOK 降噪应持续有效");
    assert!(back > off - 5.0, "关闭后应恢复");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
async fn denoise_reaches_remote() {
    let (main, alt, ch) = (
        env("CDAPP_LIVE_TOKEN"),
        env("CDAPP_LIVE_TOKEN2"),
        env("CDAPP_LIVE_VOICE"),
    );
    let api = Api::new(Some(main.clone())).unwrap();
    cdapp_lib::kook::denoise_asset::ensure_license(&api)
        .await
        .unwrap();
    let wasm = std::fs::read(env("DENOISE_WASM")).unwrap();
    cdapp_lib::audio::denoise::prepare(&wasm).unwrap();

    let denoise = Denoise::default();
    let tap: Tap = Arc::default();
    let (a, la) = start(
        &main,
        &ch,
        MicPrefs {
            source: Source::Noisy(440.0),
            denoise: denoise.clone(),
            output: Out::Null,
            ..MicPrefs::default()
        },
    );
    assert!(connected(&la).await, "主号未连接");
    let (b, lb) = start(
        &alt,
        &ch,
        MicPrefs {
            source: Source::Tone(0.0),
            muted: true,
            output: Out::Null,
            tap: Some(tap.clone()),
            ..MicPrefs::default()
        },
    );
    assert!(connected(&lb).await, "小号未连接");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let off = record(&tap, 4).await;
    denoise.set_mode(Mode::Rnnoise);
    denoise.set(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let rnn = record(&tap, 4).await;
    denoise.set_mode(Mode::Kook);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let kook = record(&tap, 4).await;
    // 持续 12 秒以上，确认许可在实时运行中有效，未在约 9 秒后静音。
    tokio::time::sleep(Duration::from_secs(6)).await;
    let kook_late = record(&tap, 4).await;

    let line = |name: &str, x: &[f32]| {
        println!(
            "{name}: 样本 {} rms {:.4} 440Hz {:.4}",
            x.len(),
            rms(x),
            tone(x, 440.0)
        );
    };
    line("关闭", &off);
    line("RNNoise", &rnn);
    line("KOOK", &kook);
    line("KOOK 12 秒后", &kook_late);

    a.leave().await;
    b.leave().await;

    assert!(rms(&off) > 0.02, "关闭降噪时应收到噪声");
    assert!(rms(&rnn) < rms(&off) * 0.7, "RNNoise 应降低能量");
    assert!(rms(&kook) < rms(&off) * 0.5, "KOOK 降噪应降低能量");
    assert!(rms(&kook_late) < rms(&off) * 0.5, "KOOK 降噪应持续有效");
}
