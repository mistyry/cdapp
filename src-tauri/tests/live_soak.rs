//! 长时间通话、多轮加入退出与故障恢复的真实环境测试：
//! `CDAPP_LIVE_TOKEN=... CDAPP_LIVE_TOKEN2=... CDAPP_LIVE_VOICE=...
//!  cargo test --features test-media --test live_soak -- --ignored --nocapture`
//!
//! 两个账号推送合成音，每 2 秒的开头 120 ms 切换为标记频率。标记周期按系统时间对齐，
//! 接收端从混音录制中检测标记出现的时刻，据此测量端到端延迟。
//! - `long_call`：每分钟检查双向可听、延迟、CPU、内存、线程与文件描述符，
//!   时长由 `CDAPP_SOAK_MINUTES` 设置，默认 30 分钟。
//! - `join_leave_rounds`：多轮加入退出，检查资源是否持续增长。轮数由 `CDAPP_SOAK_ROUNDS` 设置。
//! - `fault_recovery`：注入信令断开与系统睡眠，检查重建与恢复收听。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cdapp_lib::audio::capture::{MARK_MS, MARK_PERIOD_MS, Source, live_captures};
use cdapp_lib::audio::output::{Sink as Out, live_outputs};
use cdapp_lib::kook::api::Api;
use cdapp_lib::kook::voice::health::{DownStats, Quality, STABLE_AFTER, UpStats};
use cdapp_lib::kook::voice::{EndReason, Fault, MicPrefs, Phase, Sink, VoiceEvent, VoiceSession};

type Log = Arc<Mutex<Vec<(f64, VoiceEvent)>>>;

const MAIN_TONE: f32 = 523.0;
const ALT_TONE: f32 = 660.0;
const MAIN_MARK: f32 = 1200.0;
const ALT_MARK: f32 = 1500.0;

fn env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("缺少环境变量 {key}"))
}

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// 一段混音录制，48 kHz 单声道。
#[derive(Default)]
struct Rec {
    samples: Vec<f32>,
}

struct Party {
    session: VoiceSession,
    log: Log,
    tap: Arc<Mutex<Vec<f32>>>,
}

fn start(token: &str, channel: &str, tone: f32, mark: f32) -> Party {
    let log: Log = Arc::default();
    let tap = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    let sink: Sink = Arc::new(move |e| l.lock().unwrap().push((now_ms(), e)));
    let api = Api::new(Some(token.to_owned())).unwrap();
    let session = VoiceSession::start_with(
        api,
        channel.to_owned(),
        1,
        sink,
        MicPrefs {
            source: Source::Marked(tone, mark),
            output: Out::Null,
            tap: Some(tap.clone()),
            ..MicPrefs::default()
        },
    );
    Party { session, log, tap }
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

fn connected_count(log: &Log) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(_, e)| {
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

async fn wait_connected(log: &Log, n: usize, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if connected_count(log) >= n {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// 清空录制缓冲，录制 `ms` 毫秒，返回录制与结束时的系统时间。
async fn record(tap: &Arc<Mutex<Vec<f32>>>, ms: u64) -> (Rec, f64) {
    tap.lock().unwrap().clear();
    tokio::time::sleep(Duration::from_millis(ms)).await;
    let samples = std::mem::take(&mut *tap.lock().unwrap());
    (Rec { samples }, now_ms())
}

#[allow(clippy::cast_precision_loss)]
fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
}

/// 单频点功率。
#[allow(clippy::cast_precision_loss)]
fn power(seg: &[f32], f: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f / 48_000.0;
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in seg.iter().enumerate() {
        re += f64::from(v) * (w * i as f64).cos();
        im += f64::from(v) * (w * i as f64).sin();
    }
    re * re + im * im
}

/// 中间 1 秒内 100 至 2000 Hz 的主频，步长 1 Hz。
fn peak(x: &[f32]) -> f64 {
    let n = x.len().min(48_000);
    if n == 0 {
        return 0.0;
    }
    let start = (x.len() - n) / 2;
    let seg = &x[start..start + n];
    (100..2000)
        .map(|f| (f64::from(f), power(seg, f64::from(f))))
        .fold((0.0, 0.0), |b, c| if c.1 > b.1 { c } else { b })
        .0
}

/// 检测标记音出现的时刻，换算为相对发送时刻的延迟，单位毫秒。
/// 发送时刻取所在标记周期的起点。
#[allow(clippy::cast_precision_loss)]
fn mark_delays(rec: &Rec, end_ms: f64, base: f32, mark: f32) -> Vec<f64> {
    const WIN: usize = 480;
    let x = &rec.samples;
    let t0 = end_ms - x.len() as f64 / 48.0;
    let mut out = Vec::new();
    let mut inside = false;
    for (k, seg) in x.as_chunks::<WIN>().0.iter().enumerate() {
        let m = power(seg, f64::from(mark));
        let b = power(seg, f64::from(base));
        let on = m > 4.0 * b && rms(seg) > 0.01;
        if on && !inside {
            let at = t0 + (k * WIN) as f64 / 48.0;
            let period = MARK_PERIOD_MS as f64;
            let sent = (at / period).floor() * period;
            let d = at - sent;
            // 落在标记时长之后的检测结果无法对应周期，舍弃。
            if (0.0..period - MARK_MS as f64).contains(&d) {
                out.push(d);
            }
        }
        inside = on;
    }
    out
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

/// 最近一次链路质量事件。
fn last_link(log: &Log) -> Option<(Option<Quality>, Option<UpStats>, DownStats)> {
    log.lock().unwrap().iter().rev().find_map(|(_, e)| match e {
        VoiceEvent::Link {
            quality, up, down, ..
        } => Some((*quality, *up, *down)),
        _ => None,
    })
}

fn count(log: &Log, f: impl Fn(&VoiceEvent) -> bool) -> usize {
    log.lock().unwrap().iter().filter(|(_, e)| f(e)).count()
}

fn reconnecting(e: &VoiceEvent) -> bool {
    matches!(
        e,
        VoiceEvent::State {
            phase: Phase::Reconnecting,
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

/// 进程资源占用。
#[derive(Debug, Clone, Copy)]
struct Res {
    rss_mb: f64,
    cpu_s: f64,
    threads: u64,
    fds: u64,
}

#[allow(clippy::cast_precision_loss)]
fn res() -> Res {
    // SAFETY: rusage 为纯数据结构，getrusage 只写入该结构。
    let cpu_s = unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &raw mut u);
        let t = |v: libc::timeval| v.tv_sec as f64 + f64::from(v.tv_usec) / 1e6;
        t(u.ru_utime) + t(u.ru_stime)
    };
    let (rss_mb, threads) = task_info();
    let fds = std::fs::read_dir("/dev/fd").map_or(0, |d| d.count() as u64);
    Res {
        rss_mb,
        cpu_s,
        threads,
        fds,
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::cast_precision_loss)]
fn task_info() -> (f64, u64) {
    // SAFETY: proc_taskinfo 为纯数据结构，proc_pidinfo 按传入大小写入。
    unsafe {
        let mut info: libc::proc_taskinfo = std::mem::zeroed();
        let size = i32::try_from(std::mem::size_of::<libc::proc_taskinfo>()).unwrap_or(0);
        let n = libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&raw mut info).cast(),
            size,
        );
        if n != size {
            return (0.0, 0);
        }
        (
            info.pti_resident_size as f64 / 1_048_576.0,
            u64::try_from(info.pti_threadnum).unwrap_or(0),
        )
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::cast_precision_loss)]
fn task_info() -> (f64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    (field("VmRSS:") as f64 / 1024.0, field("Threads:"))
}

/// 存活的异步任务数。
fn alive_tasks() -> usize {
    let tauri::async_runtime::RuntimeHandle::Tokio(h) = tauri::async_runtime::handle();
    h.metrics().num_alive_tasks()
}

/// 最小二乘斜率，单位为每个样本点。
#[allow(clippy::cast_precision_loss)]
fn slope(y: &[f64]) -> f64 {
    let n = y.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mx = (n - 1.0) / 2.0;
    let my = y.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, v) in y.iter().enumerate() {
        let dx = i as f64 - mx;
        num += dx * (v - my);
        den += dx * dx;
    }
    num / den
}

fn fmt_up(up: Option<UpStats>) -> String {
    up.map_or_else(
        || "无".into(),
        |u| {
            format!(
                "score={} loss={:.3} pkts={} lost={} kbps={}",
                u.score,
                u.loss,
                u.packets,
                u.lost,
                u.bitrate / 1000
            )
        },
    )
}

fn fmt_down(d: DownStats) -> String {
    format!(
        "pkts={} lost={} late={} overflow={} underruns={} depth={}ms",
        d.packets, d.lost, d.late, d.overflow, d.underruns, d.depth_ms
    )
}

/// 同时录制双方的混音，返回双方的主频、RMS 与延迟中位数。
async fn probe(a: &Party, b: &Party, ms: u64) -> [Option<f64>; 6] {
    let ((ra, ea), (rb, eb)) = tokio::join!(record(&a.tap, ms), record(&b.tap, ms));
    let mut da = mark_delays(&ra, ea, ALT_TONE, ALT_MARK);
    let mut db = mark_delays(&rb, eb, MAIN_TONE, MAIN_MARK);
    [
        Some(peak(&ra.samples)),
        Some(rms(&ra.samples)),
        median(&mut da),
        Some(peak(&rb.samples)),
        Some(rms(&rb.samples)),
        median(&mut db),
    ]
}

fn heard(f: Option<f64>, want: f32) -> bool {
    f.is_some_and(|f| (f - f64::from(want)).abs() <= 2.0)
}

fn opt(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "-".into(), |x| format!("{x:.digits$}"))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
#[allow(clippy::too_many_lines)]
async fn long_call() {
    let (main, alt, ch) = (
        env("CDAPP_LIVE_TOKEN"),
        env("CDAPP_LIVE_TOKEN2"),
        env("CDAPP_LIVE_VOICE"),
    );
    let minutes = env_num("CDAPP_SOAK_MINUTES", 30);
    let base = res();
    let a = start(&main, &ch, MAIN_TONE, MAIN_MARK);
    assert!(wait_connected(&a.log, 1, 20).await, "主号未连接");
    let b = start(&alt, &ch, ALT_TONE, ALT_MARK);
    assert!(wait_connected(&b.log, 1, 20).await, "小号未连接");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let t0 = Instant::now();
    let r0 = res();
    println!(
        "加入前 rss={:.1}MB threads={} fds={}，加入后 rss={:.1}MB threads={} fds={} 采集线程={} 播放线程={}",
        base.rss_mb,
        base.threads,
        base.fds,
        r0.rss_mb,
        r0.threads,
        r0.fds,
        live_captures(),
        live_outputs()
    );
    println!(
        "分钟 | 主号收到 Hz/rms/延迟ms | 小号收到 Hz/rms/延迟ms | rss MB | CPU% | 线程 | fd | 主号接收 | 主号推流 | 小号接收 | 小号推流 | 质量"
    );
    let (mut rss, mut ok, mut delays) = (Vec::new(), 0u64, Vec::new());
    let mut prev_cpu = r0.cpu_s;
    let mut prev_t = t0;
    for m in 1..=minutes {
        let due = t0 + Duration::from_secs(60 * m);
        tokio::time::sleep(due.saturating_duration_since(Instant::now() + Duration::from_secs(5)))
            .await;
        let p = probe(&a, &b, 5000).await;
        let r = res();
        let dt = prev_t.elapsed().as_secs_f64();
        let cpu = (r.cpu_s - prev_cpu) / dt * 100.0;
        prev_cpu = r.cpu_s;
        prev_t = Instant::now();
        let (la, lb) = (last_link(&a.log), last_link(&b.log));
        let good = heard(p[0], ALT_TONE) && heard(p[3], MAIN_TONE);
        ok += u64::from(good);
        delays.extend([p[2], p[5]].into_iter().flatten());
        rss.push(r.rss_mb);
        println!(
            "{m:>3} | {}/{}/{} | {}/{}/{} | {:.1} | {:.1} | {} | {} | {} | {} | {} | {} | {:?}/{:?}",
            opt(p[0], 0),
            opt(p[1], 3),
            opt(p[2], 0),
            opt(p[3], 0),
            opt(p[4], 3),
            opt(p[5], 0),
            r.rss_mb,
            cpu,
            r.threads,
            r.fds,
            la.map_or_else(|| "-".into(), |l| fmt_down(l.2)),
            fmt_up(la.and_then(|l| l.1)),
            lb.map_or_else(|| "-".into(), |l| fmt_down(l.2)),
            fmt_up(lb.and_then(|l| l.1)),
            la.and_then(|l| l.0),
            lb.and_then(|l| l.0),
        );
    }
    let total = t0.elapsed().as_secs_f64();
    let r1 = res();
    let rebuilt = count(&a.log, reconnecting) + count(&b.log, reconnecting);
    let mut d = delays.clone();
    let med = median(&mut d);
    let (lo, hi) = delays
        .iter()
        .fold((f64::MAX, 0.0f64), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    println!(
        "通话 {:.1} 分钟，双向可听 {ok}/{minutes} 次，重建 {rebuilt} 次，平均 CPU {:.1}%，\
         rss 从 {:.1} 到 {:.1} MB，斜率 {:.3} MB/分钟，线程 {}→{}，fd {}→{}，延迟中位数 {} ms，样本 {} 个，范围 {:.0}–{:.0} ms",
        total / 60.0,
        (r1.cpu_s - r0.cpu_s) / total * 100.0,
        rss.first().copied().unwrap_or(0.0),
        rss.last().copied().unwrap_or(0.0),
        slope(&rss),
        r0.threads,
        r1.threads,
        r0.fds,
        r1.fds,
        opt(med, 0),
        delays.len(),
        if delays.is_empty() { 0.0 } else { lo },
        hi,
    );
    a.session.leave().await;
    b.session.leave().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let r2 = res();
    println!(
        "退出后 采集线程={} 播放线程={} threads={} fds={} rss={:.1}MB",
        live_captures(),
        live_outputs(),
        r2.threads,
        r2.fds,
        r2.rss_mb
    );
    assert_eq!(live_captures() + live_outputs(), 0);
    assert!(ok * 10 >= minutes * 9, "双向可听的分钟数不足九成");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
#[allow(clippy::many_single_char_names)]
async fn join_leave_rounds() {
    let (main, alt, ch) = (
        env("CDAPP_LIVE_TOKEN"),
        env("CDAPP_LIVE_TOKEN2"),
        env("CDAPP_LIVE_VOICE"),
    );
    let rounds = env_num("CDAPP_SOAK_ROUNDS", 20);
    // 小号保持在线，主号反复加入退出。
    let b = start(&alt, &ch, ALT_TONE, ALT_MARK);
    assert!(wait_connected(&b.log, 1, 20).await, "小号未连接");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let r0 = res();
    let tasks0 = alive_tasks();
    println!(
        "初始 rss={:.1}MB threads={} fds={} tokio 任务={tasks0}",
        r0.rss_mb, r0.threads, r0.fds
    );
    println!(
        "轮次 | 加入ms | 主号收到Hz | 小号收到Hz | rss MB | 线程 | fd | tokio 任务 | 采集/播放线程"
    );
    let (mut rss, mut th, mut fds, mut tasks) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut heard_ok = 0;
    for round in 1..=rounds {
        let t = Instant::now();
        let a = start(&main, &ch, MAIN_TONE, MAIN_MARK);
        assert!(wait_connected(&a.log, 1, 20).await, "第 {round} 轮未连接");
        let join_ms = t.elapsed().as_millis();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let p = probe(&a, &b, 2000).await;
        heard_ok += u32::from(heard(p[0], ALT_TONE) && heard(p[3], MAIN_TONE));
        a.session.leave().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let r = res();
        let n = alive_tasks();
        #[allow(clippy::cast_precision_loss)]
        {
            rss.push(r.rss_mb);
            th.push(r.threads as f64);
            fds.push(r.fds as f64);
            tasks.push(n as f64);
        }
        println!(
            "{round:>2} | {join_ms} | {} | {} | {:.1} | {} | {} | {n} | {}/{}",
            opt(p[0], 0),
            opt(p[3], 0),
            r.rss_mb,
            r.threads,
            r.fds,
            live_captures(),
            live_outputs()
        );
    }
    b.session.leave().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let r1 = res();
    let half = rss.len() / 2;
    println!(
        "共 {rounds} 轮，双向可听 {heard_ok}/{rounds} 轮。\
         rss 从 {:.1} 到 {:.1} MB，斜率 {:.3} MB/轮，后半段 {:.3}。线程从 {} 到 {}，斜率 {:.3}/轮。\
         fd 从 {} 到 {}，斜率 {:.3}/轮。tokio 任务从 {} 到 {}，斜率 {:.3}/轮。\
         结束后 threads={} fds={} tokio 任务={} 采集/播放线程={}/{}",
        rss[0],
        rss[rss.len() - 1],
        slope(&rss),
        slope(&rss[half..]),
        th[0],
        th[th.len() - 1],
        slope(&th),
        fds[0],
        fds[fds.len() - 1],
        slope(&fds),
        tasks[0],
        tasks[tasks.len() - 1],
        slope(&tasks),
        r1.threads,
        r1.fds,
        alive_tasks(),
        live_captures(),
        live_outputs()
    );
    assert_eq!(live_captures() + live_outputs(), 0);
    assert!(slope(&fds) < 0.5, "fd 持续增长");
    assert!(slope(&th) < 0.5, "线程持续增长");
    assert!(slope(&tasks) < 0.5, "tokio 任务持续增长");
}

/// 注入故障，等待重新连接并检查双向可听，返回恢复用时。
async fn recover(a: &Party, b: &Party, fault: Fault, label: &str) -> u128 {
    // 先保持稳定连接，使重建次数重新计数。
    tokio::time::sleep(STABLE_AFTER + Duration::from_secs(2)).await;
    let before = connected_count(&a.log);
    let t = Instant::now();
    a.session.control().inject(fault);
    assert!(
        wait_connected(&a.log, before + 1, 40).await,
        "{label}后未重新连接"
    );
    let ms = t.elapsed().as_millis();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let p = probe(a, b, 3000).await;
    let (ra, rb) = (heard(p[0], ALT_TONE), heard(p[3], MAIN_TONE));
    // 小号看到的成员离开不应多于加入。
    let joined = count(&b.log, |e| matches!(e, VoiceEvent::PeerJoined { .. }));
    let left = count(&b.log, |e| matches!(e, VoiceEvent::PeerLeft { .. }));
    println!(
        "{label}：恢复用时 {ms} ms，主号收到 {} Hz，小号收到 {} Hz，成员加入 {joined} 次，离开 {left} 次",
        opt(p[0], 0),
        opt(p[3], 0)
    );
    assert!(ra && rb, "{label}后未恢复双向收听");
    assert!(joined >= left, "{label}后成员状态不一致");
    ms
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要真实账号与语音频道"]
#[allow(clippy::many_single_char_names)]
async fn fault_recovery() {
    let (main, alt, ch) = (
        env("CDAPP_LIVE_TOKEN"),
        env("CDAPP_LIVE_TOKEN2"),
        env("CDAPP_LIVE_VOICE"),
    );
    let a = start(&main, &ch, MAIN_TONE, MAIN_MARK);
    assert!(wait_connected(&a.log, 1, 20).await, "主号未连接");
    let b = start(&alt, &ch, ALT_TONE, ALT_MARK);
    assert!(wait_connected(&b.log, 1, 20).await, "小号未连接");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let p = probe(&a, &b, 3000).await;
    println!("主号收到 {} Hz，小号收到 {} Hz", opt(p[0], 0), opt(p[3], 0));
    assert!(heard(p[0], ALT_TONE) && heard(p[3], MAIN_TONE));

    recover(&a, &b, Fault::CloseSignal, "信令断开").await;
    recover(&a, &b, Fault::Sleep, "系统睡眠").await;
    let attempts: Vec<(Option<u32>, Option<u64>)> = a
        .log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, e)| match e {
            VoiceEvent::State {
                phase: Phase::Reconnecting,
                attempt,
                retry_in_ms,
                ..
            } => Some((*attempt, *retry_in_ms)),
            _ => None,
        })
        .collect();
    println!("重连次数与等待时长：{attempts:?}");

    // 加入持续失败时，重建次数用尽后结束会话。
    tokio::time::sleep(STABLE_AFTER + Duration::from_secs(2)).await;
    let t = Instant::now();
    a.session.control().inject(Fault::FailJoin);
    assert!(
        wait_for(&a.log, 60, |e| ended(e) == Some(EndReason::Lost)).await,
        "重建次数用尽后未结束"
    );
    let n = a
        .log
        .lock()
        .unwrap()
        .iter()
        .rev()
        .take_while(|(_, e)| {
            !matches!(
                e,
                VoiceEvent::State {
                    phase: Phase::Connected,
                    ..
                }
            )
        })
        .filter(|(_, e)| reconnecting(e))
        .count();
    println!(
        "结束用时 {} ms，重连 {n} 次，会话已结束={}",
        t.elapsed().as_millis(),
        a.session.is_finished()
    );
    let left = wait_for(&b.log, 15, |e| matches!(e, VoiceEvent::PeerLeft { .. })).await;
    println!("小号看到主号离开={left}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(a.session.is_finished());
    a.session.leave().await;
    b.session.leave().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!(
        "退出后 采集/播放线程={}/{}",
        live_captures(),
        live_outputs()
    );
    assert_eq!(live_captures() + live_outputs(), 0);
}
