//! 麦克风采集与发送。
//! 采集线程把设备样本混为单声道、重采样到 48 kHz 并切分为 60 ms 帧，放入有界队列。
//! 发送任务从队列取帧，编码为 Opus 后经推流连接发出。停止时等待采集线程退出并释放设备。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, SampleFormat, StreamConfig};
use serde::Serialize;
use tokio::sync::{Notify, mpsc, oneshot};

use super::denoise::{Denoise, Stage};
#[cfg(any(test, feature = "test-media"))]
use super::dsp::FRAME_SAMPLES;
use super::dsp::{
    Bounded, FRAME_MS, Framer, Gain, RATE, Resampler, ZeroProbe, audio_level, meter, mixdown,
};
use super::mic::MicStatus;
use crate::kook::voice::peer::Uplink;

/// 采集队列的最大帧数，满时丢弃最旧的帧。
pub const QUEUE_FRAMES: usize = 5;
/// 判定输入全零的样本窗口，1.5 秒。
const ZERO_WINDOW: usize = RATE as usize * 3 / 2;
/// 每隔若干帧上报一次输入电平。
const METER_EVERY: u32 = 4;
/// 打开设备的时限。
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// 存活的采集线程数。
static LIVE_THREADS: AtomicUsize = AtomicUsize::new(0);

/// 存活的采集线程数，用于检查资源释放。
#[must_use]
pub fn live_captures() -> usize {
    LIVE_THREADS.load(Ordering::SeqCst)
}

/// 输入设备。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// 列出输入设备，不打开设备。
///
/// # Errors
///
/// 系统无法枚举设备时返回错误。
pub fn input_devices() -> Result<Vec<InputDevice>, String> {
    let host = cpal::default_host();
    let default = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());
    let list = host.input_devices().map_err(|e| e.to_string())?;
    Ok(list
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d
                .description()
                .map_or_else(|_| d.to_string(), |x| x.name().to_owned());
            Some(InputDevice {
                default: default.as_deref() == Some(id.as_str()),
                id,
                name,
            })
        })
        .collect())
}

/// 采集来源。
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// 麦克风设备，None 为系统默认。
    Device(Option<String>),
    /// 固定频率的合成正弦音。
    #[cfg(any(test, feature = "test-media"))]
    Tone(f32),
    /// 带标记音的合成音。按 `MARK_PERIOD_MS` 周期在每个周期开头切换为标记频率，持续 `MARK_MS`。
    /// 周期按系统时间对齐，接收端可据此测量端到端延迟。
    #[cfg(any(test, feature = "test-media"))]
    Marked(f32, f32),
    /// 白噪声叠加正弦音，幅度分别为 0.1 与 0.05，经与设备相同的降噪链路。
    #[cfg(any(test, feature = "test-media"))]
    Noisy(f32),
}

/// 标记音的周期与时长，单位毫秒。
#[cfg(any(test, feature = "test-media"))]
pub const MARK_PERIOD_MS: u64 = 2000;
#[cfg(any(test, feature = "test-media"))]
pub const MARK_MS: u64 = 120;

/// 采集线程发出的事件，`epoch` 用于区分采集轮次。
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    Started {
        epoch: u64,
        device: String,
    },
    Failed {
        epoch: u64,
        status: MicStatus,
        message: String,
    },
    Level {
        epoch: u64,
        level: u8,
    },
}

/// 采集线程与发送任务之间的帧队列。
#[derive(Clone)]
struct Frames {
    queue: Arc<Mutex<Bounded<Vec<i16>>>>,
    ready: Arc<Notify>,
}

impl Frames {
    fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(Bounded::new(QUEUE_FRAMES))),
            ready: Arc::new(Notify::new()),
        }
    }

    fn push(&self, frames: Vec<Vec<i16>>) {
        if frames.is_empty() {
            return;
        }
        if let Ok(mut q) = self.queue.lock() {
            for f in frames {
                q.push(f);
            }
        }
        self.ready.notify_one();
    }

    /// 取出一帧，并返回上次取帧以来丢弃的帧数。
    fn pop(&self, reported: &mut u64) -> Option<(Vec<i16>, u64)> {
        let mut q = self.queue.lock().ok()?;
        let f = q.pop()?;
        let dropped = q.dropped() - *reported;
        *reported = q.dropped();
        Some((f, dropped))
    }
}

/// 一次采集。持有采集线程与发送任务。
pub struct Capture {
    stop: Arc<AtomicBool>,
    wake: std::sync::mpsc::Sender<()>,
    released: oneshot::Receiver<()>,
    sender: tauri::async_runtime::JoinHandle<u64>,
    frames: Frames,
    pub epoch: u64,
}

impl Capture {
    /// 启动采集线程与发送任务。
    #[must_use]
    pub fn start(
        source: Source,
        gain: Gain,
        denoise: Denoise,
        uplink: Uplink,
        epoch: u64,
        events: mpsc::UnboundedSender<CaptureEvent>,
    ) -> Self {
        let frames = Frames::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (wake, wait) = std::sync::mpsc::channel();
        let (released_tx, released) = oneshot::channel();
        {
            let (frames, stop, tx) = (frames.clone(), stop.clone(), events.clone());
            let spawned = std::thread::Builder::new()
                .name("mic-capture".into())
                .spawn(move || {
                    LIVE_THREADS.fetch_add(1, Ordering::SeqCst);
                    match source {
                        #[cfg(any(test, feature = "test-media"))]
                        Source::Tone(freq) => {
                            run_tone(freq, None, &gain, &frames, &stop, &wait, epoch, &tx);
                        }
                        #[cfg(any(test, feature = "test-media"))]
                        Source::Marked(freq, mark) => {
                            run_tone(freq, Some(mark), &gain, &frames, &stop, &wait, epoch, &tx);
                        }
                        #[cfg(any(test, feature = "test-media"))]
                        Source::Noisy(freq) => {
                            run_noisy(freq, &gain, &denoise, &frames, &stop, &wait, epoch, &tx);
                        }
                        Source::Device(id) => {
                            run_device(
                                id.as_deref(),
                                &gain,
                                &denoise,
                                &frames,
                                &stop,
                                &wait,
                                epoch,
                                &tx,
                            );
                        }
                    }
                    LIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                    let _ = released_tx.send(());
                });
            if let Err(e) = spawned {
                let _ = events.send(CaptureEvent::Failed {
                    epoch,
                    status: MicStatus::Error,
                    message: format!("无法启动采集线程：{e}"),
                });
            }
        }
        let sender = tauri::async_runtime::spawn(pump(frames.clone(), uplink, epoch, events));
        Self {
            stop,
            wake,
            released,
            sender,
            frames,
            epoch,
        }
    }

    /// 停止采集并等待设备释放，最多 2 秒。返回累计丢弃的帧数。
    pub async fn stop(self) -> u64 {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.wake.send(());
        if tokio::time::timeout(Duration::from_secs(2), self.released)
            .await
            .is_err()
        {
            log::warn!("采集线程未在时限内退出");
        }
        self.sender.abort();
        let dropped = self.frames.queue.lock().map_or(0, |q| q.dropped());
        if dropped > 0 {
            log::info!("采集队列已满，丢弃了 {dropped} 帧");
        }
        dropped
    }
}

/// 发送任务：编码队列中的帧并发出。
async fn pump(
    frames: Frames,
    uplink: Uplink,
    epoch: u64,
    events: mpsc::UnboundedSender<CaptureEvent>,
) -> u64 {
    let fail = |message: String| {
        let _ = events.send(CaptureEvent::Failed {
            epoch,
            status: MicStatus::Error,
            message,
        });
    };
    let mut enc = match encoder() {
        Ok(e) => e,
        Err(e) => {
            fail(format!("Opus 编码器初始化失败：{e}"));
            return 0;
        }
    };
    let mut out = vec![0u8; 4000];
    let (mut reported, mut sent, mut n) = (0u64, 0u64, 0u32);
    loop {
        frames.ready.notified().await;
        while let Some((pcm, dropped)) = frames.pop(&mut reported) {
            let len = match enc.encode(&pcm, &mut out) {
                Ok(l) => l,
                Err(e) => {
                    fail(format!("Opus 编码失败：{e}"));
                    return sent;
                }
            };
            let level = audio_level(&pcm);
            let skipped = u16::try_from(dropped).unwrap_or(u16::MAX);
            // 丢弃的帧数随包传入，用于推进时间戳。
            if let Err(e) = uplink
                .write_opus(
                    Bytes::copy_from_slice(&out[..len]),
                    Duration::from_millis(u64::from(FRAME_MS)),
                    level,
                    skipped,
                )
                .await
            {
                log::debug!("音频发送失败：{e}");
            } else {
                sent += 1;
            }
            n = n.wrapping_add(1);
            if n % METER_EVERY == 0 {
                let _ = events.send(CaptureEvent::Level {
                    epoch,
                    level: meter(level),
                });
            }
        }
    }
}

/// 创建与推流参数一致的 Opus 编码器：64 kbps、带内 FEC、DTX。
///
/// # Errors
///
/// libopus 拒绝参数时返回错误。
pub fn encoder() -> Result<opus::Encoder, opus::Error> {
    let mut enc = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)?;
    enc.set_bitrate(opus::Bitrate::Bits(64_000))?;
    enc.set_inband_fec(true)?;
    enc.set_packet_loss_perc(5)?;
    enc.set_dtx(true)?;
    Ok(enc)
}

/// 按实时节奏生成合成正弦音。
#[cfg(any(test, feature = "test-media"))]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
#[allow(clippy::too_many_arguments)]
fn run_tone(
    freq: f32,
    mark: Option<f32>,
    gain: &Gain,
    frames: &Frames,
    stop: &AtomicBool,
    wait: &std::sync::mpsc::Receiver<()>,
    epoch: u64,
    events: &mpsc::UnboundedSender<CaptureEvent>,
) {
    let _ = events.send(CaptureEvent::Started {
        epoch,
        device: format!("合成音 {freq} Hz"),
    });
    let mut splitter = Framer::default();
    let mut i: u64 = 0;
    let start = std::time::Instant::now();
    // 标记音按系统时间定位。
    let wall0 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    let mut phase = 0.0f64;
    let mut k: u32 = 0;
    while !stop.load(Ordering::SeqCst) {
        let mut chunk: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|_| {
                let ms = wall0 + i as f64 * 1000.0 / f64::from(RATE);
                i += 1;
                let f = match mark {
                    Some(m) if (ms as u64) % MARK_PERIOD_MS < MARK_MS => m,
                    _ => freq,
                };
                // 相位连续累加，切换频率时不产生跳变。
                phase += 2.0 * std::f64::consts::PI * f64::from(f) / f64::from(RATE);
                (0.3 * phase.sin()) as f32
            })
            .collect();
        gain.apply(&mut chunk);
        frames.push(splitter.push(&chunk));
        k += 1;
        let due = start + Duration::from_millis(u64::from(FRAME_MS) * u64::from(k));
        let now = std::time::Instant::now();
        if due > now
            && !matches!(
                wait.recv_timeout(due - now),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            )
        {
            break;
        }
    }
}

/// 按实时节奏生成噪声加正弦音，经完整采集链路。
#[cfg(any(test, feature = "test-media"))]
#[allow(clippy::too_many_arguments, clippy::cast_possible_truncation)]
fn run_noisy(
    freq: f32,
    gain: &Gain,
    denoise: &Denoise,
    frames: &Frames,
    stop: &AtomicBool,
    wait: &std::sync::mpsc::Receiver<()>,
    epoch: u64,
    events: &mpsc::UnboundedSender<CaptureEvent>,
) {
    let _ = events.send(CaptureEvent::Started {
        epoch,
        device: format!("噪声加 {freq} Hz"),
    });
    let mut chain = Chain::new(RATE, 1, gain.clone(), denoise);
    let start = std::time::Instant::now();
    let (mut seed, mut phase, mut k) = (1u32, 0.0f64, 0u32);
    while !stop.load(Ordering::SeqCst) {
        denoise.maintain();
        // 10 ms 一段，与设备回调粒度相近。
        let chunk: Vec<f32> = (0..RATE as usize / 100)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let n = (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 0.2;
                phase += 2.0 * std::f64::consts::PI * f64::from(freq) / f64::from(RATE);
                (n + 0.05 * phase.sin()) as f32
            })
            .collect();
        chain.feed(&chunk, frames);
        k += 1;
        let due = start + Duration::from_millis(10 * u64::from(k));
        let now = std::time::Instant::now();
        if due > now
            && !matches!(
                wait.recv_timeout(due - now),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            )
        {
            break;
        }
    }
}

/// 采集处理链：混音、全零检测、增益、重采样、降噪与分帧。
/// 全零检测在增益之前，增益为 0% 时不误判为没有权限。
struct Chain {
    channels: usize,
    gain: Gain,
    resampler: Resampler,
    denoise: Stage,
    framer: Framer,
    probe: ZeroProbe,
    reported: bool,
}

impl Chain {
    fn new(rate: u32, channels: usize, gain: Gain, denoise: &Denoise) -> Self {
        Self {
            channels,
            gain,
            resampler: Resampler::new(rate, RATE),
            denoise: denoise.stage(),
            framer: Framer::default(),
            probe: ZeroProbe::new(ZERO_WINDOW),
            reported: false,
        }
    }

    /// 处理一段样本。首次判定为全零时返回 true，之后不再重复。
    fn feed(&mut self, data: &[f32], frames: &Frames) -> bool {
        let mut mono = mixdown(data, self.channels);
        let zero = self.probe.feed(&mono) == Some(true) && !self.reported;
        if zero {
            self.reported = true;
        }
        self.gain.apply(&mut mono);
        let mut out = self.resampler.process(&mono);
        self.denoise.process(&mut out);
        frames.push(self.framer.push(&out));
        zero
    }
}

/// 采集回调发往采集线程的信号。
enum Signal {
    Error(cpal::Error),
    Silent,
}

fn status_of(kind: ErrorKind) -> MicStatus {
    match kind {
        ErrorKind::PermissionDenied => MicStatus::Denied,
        ErrorKind::DeviceNotAvailable | ErrorKind::HostUnavailable => MicStatus::NoDevice,
        _ => MicStatus::Error,
    }
}

/// 选择设备。所选设备不可用时改用系统默认设备，返回值第二项表示是否已改用。
fn pick_device<D>(
    id: Option<&str>,
    by_id: impl FnOnce(&str) -> Option<D>,
    default: impl FnOnce() -> Option<D>,
) -> Result<(D, bool), (MicStatus, String)> {
    if let Some(d) = id.and_then(by_id) {
        return Ok((d, false));
    }
    default()
        .map(|d| (d, id.is_some()))
        .ok_or((MicStatus::NoDevice, "没有可用的麦克风".into()))
}

/// 采集流信号的处理结果。
enum Next {
    /// 设备断开，需要重新打开。
    Lost(cpal::Error),
    /// 结束采集。
    Fail(MicStatus, String),
}

fn classify(sig: Signal) -> Option<Next> {
    match sig {
        Signal::Silent => Some(Next::Fail(
            MicStatus::Denied,
            "麦克风没有输入。请在系统设置中允许 cdapp 使用麦克风".into(),
        )),
        Signal::Error(e) => match e.kind() {
            ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated => Some(Next::Lost(e)),
            // 设备切换与缓冲欠载由系统处理。
            ErrorKind::DeviceChanged | ErrorKind::Xrun => None,
            ErrorKind::PermissionDenied => Some(Next::Fail(
                MicStatus::Denied,
                format!("麦克风权限被拒绝：{e}"),
            )),
            _ => {
                log::warn!("麦克风流错误：{e}");
                None
            }
        },
    }
}

/// 所选设备已重新可用。未选择具体设备时为 false。
fn chosen_back(chosen: Option<&str>) -> bool {
    chosen.is_some_and(|c| find_device(Some(c)).is_ok_and(|(_, fell_back)| !fell_back))
}

/// 回退到系统默认设备期间，检查所选设备是否恢复的间隔。
pub(crate) const RESTORE_PROBE: Duration = Duration::from_secs(2);

fn find_device(id: Option<&str>) -> Result<(Device, bool), (MicStatus, String)> {
    let host = cpal::default_host();
    pick_device(
        id,
        |id| {
            id.parse::<cpal::DeviceId>()
                .ok()
                .and_then(|parsed| host.device_by_id(&parsed))
        },
        || host.default_input_device(),
    )
}

fn device_name(d: &Device) -> String {
    d.description()
        .map_or_else(|_| d.to_string(), |x| x.name().to_owned())
}

/// 打开并启动输入流。设备支持 48 kHz 时优先使用，避免重采样。
fn open(
    device: &Device,
    gain: &Gain,
    denoise: &Denoise,
    frames: &Frames,
    signal: &std::sync::mpsc::Sender<Signal>,
) -> Result<cpal::Stream, (MicStatus, String)> {
    let fail = |e: cpal::Error| (status_of(e.kind()), format!("无法打开麦克风：{e}"));
    let cfg = device.default_input_config().map_err(fail)?;
    // 保持声道与样本格式，只调整采样率。
    let cfg = device
        .supported_input_configs()
        .ok()
        .and_then(|mut it| {
            it.find(|r| {
                r.channels() == cfg.channels()
                    && r.sample_format() == cfg.sample_format()
                    && r.min_sample_rate() <= RATE
                    && r.max_sample_rate() >= RATE
            })
        })
        .map_or(cfg, |r| r.with_sample_rate(RATE));
    let config: StreamConfig = cfg.config();
    let mut chain = Chain::new(
        config.sample_rate,
        usize::from(config.channels.max(1)),
        gain.clone(),
        denoise,
    );
    let (frames, on_silent) = (frames.clone(), signal.clone());
    let mut scratch: Vec<f32> = Vec::new();
    macro_rules! build {
        ($t:ty) => {{
            let err = signal.clone();
            device.build_input_stream::<$t, _, _>(
                config,
                move |d: &[$t], _| {
                    scratch.clear();
                    scratch.extend(
                        d.iter()
                            .map(|&s| <f32 as cpal::FromSample<$t>>::from_sample_(s)),
                    );
                    if chain.feed(&scratch, &frames) {
                        let _ = on_silent.send(Signal::Silent);
                    }
                },
                move |e| {
                    let _ = err.send(Signal::Error(e));
                },
                Some(OPEN_TIMEOUT),
            )
        }};
    }
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U8 => build!(u8),
        SampleFormat::I8 => build!(i8),
        f => return Err((MicStatus::Error, format!("不支持的样本格式 {f:?}"))),
    }
    .map_err(fail)?;
    stream.play().map_err(fail)?;
    Ok(stream)
}

/// 采集线程：打开设备并监视错误，设备丢失时改用系统默认设备。
#[allow(clippy::too_many_arguments)]
fn run_device(
    id: Option<&str>,
    gain: &Gain,
    denoise: &Denoise,
    frames: &Frames,
    stop: &AtomicBool,
    wait: &std::sync::mpsc::Receiver<()>,
    epoch: u64,
    events: &mpsc::UnboundedSender<CaptureEvent>,
) {
    let failed = |status, message| {
        let _ = events.send(CaptureEvent::Failed {
            epoch,
            status,
            message,
        });
    };
    // 所选设备不可用时改用系统默认设备，并定期检查，所选设备恢复后自动切回。
    let chosen = id.map(str::to_owned);
    let mut target = chosen.clone();
    let mut restored = false;
    loop {
        let (sig_tx, sig_rx) = std::sync::mpsc::channel();
        let device = match find_device(target.as_deref()) {
            Ok((d, fell_back)) => {
                if fell_back {
                    log::warn!("所选麦克风不可用，改用系统默认设备 {}", device_name(&d));
                    target = None;
                }
                d
            }
            Err((status, message)) => return failed(status, message),
        };
        let stream = match open(&device, gain, denoise, frames, &sig_tx) {
            Ok(s) => s,
            Err((status, message)) => return failed(status, message),
        };
        let _ = events.send(CaptureEvent::Started {
            epoch,
            device: device_name(&device),
        });
        if restored {
            log::info!("所选麦克风已恢复，切回该设备");
            restored = false;
        }
        let mut last_probe = std::time::Instant::now();
        // 等待停止信号，同时处理流错误与全零检测。
        let lost = loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if target.is_none() && last_probe.elapsed() >= RESTORE_PROBE {
                last_probe = std::time::Instant::now();
                if chosen_back(chosen.as_deref()) {
                    target.clone_from(&chosen);
                    restored = true;
                    break None;
                }
            }
            // 降噪实例在此创建，不占用采集回调的时间。
            denoise.maintain();
            if !matches!(
                wait.recv_timeout(Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                return;
            }
            match sig_rx.try_recv().ok().and_then(classify) {
                Some(Next::Lost(e)) => break Some(e),
                Some(Next::Fail(status, message)) => {
                    drop(stream);
                    return failed(status, message);
                }
                None => {}
            }
        };
        drop(stream);
        let Some(lost) = lost else {
            continue;
        };
        log::warn!("麦克风 {} 断开：{lost}", device_name(&device));
        // 正在使用系统默认设备时：所选设备已恢复则切回，否则重新查找当前的默认设备。
        if target.is_none() {
            if chosen_back(chosen.as_deref()) {
                target.clone_from(&chosen);
                restored = true;
                continue;
            }
            if find_device(None).is_ok() {
                log::info!("改用新的系统默认麦克风");
                continue;
            }
            log::warn!("没有可用的麦克风，停止采集");
            return failed(MicStatus::DeviceLost, format!("麦克风已断开：{lost}"));
        }
        target = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_selected_device_falls_back_to_default() {
        let by_id = |id: &str| (id == "usb").then_some("usb");
        assert_eq!(
            pick_device(Some("usb"), by_id, || Some("默认设备")),
            Ok(("usb", false))
        );
        // 所选设备不存在时改用默认设备。
        assert_eq!(
            pick_device(Some("gone"), by_id, || Some("默认设备")),
            Ok(("默认设备", true))
        );
        assert_eq!(
            pick_device(None, by_id, || Some("默认设备")),
            Ok(("默认设备", false))
        );
        assert!(matches!(
            pick_device(Some("gone"), by_id, || None::<&str>),
            Err((MicStatus::NoDevice, _))
        ));
    }

    #[test]
    fn encoder_produces_60ms_frames() {
        let mut enc = encoder().unwrap();
        let mut out = vec![0u8; 4000];
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let pcm: Vec<i16> = (0..FRAME_SAMPLES)
            .map(|i| ((i as f32 * 0.0576).sin() * 8000.0) as i16)
            .collect();
        let len = enc.encode(&pcm, &mut out).unwrap();
        assert!(len > 3 && len < 1000, "{len}");
        let mut dec = opus::Decoder::new(RATE, opus::Channels::Mono).unwrap();
        let mut back = vec![0i16; 5760];
        assert_eq!(
            dec.decode(&out[..len], &mut back, false).unwrap(),
            FRAME_SAMPLES
        );
    }

    #[test]
    fn chain_reports_silence_once_and_frames_resampled_input() {
        let frames = Frames::new();
        let mut chain = Chain::new(44_100, 2, Gain::default(), &Denoise::default());
        let mut zero = 0;
        for _ in 0..200 {
            if chain.feed(&[0.0; 1024], &frames) {
                zero += 1;
            }
        }
        assert_eq!(zero, 1);
        // 全零输入仍按帧送入队列，超出容量的帧被丢弃。
        let q = frames.queue.lock().unwrap();
        assert_eq!(q.len(), QUEUE_FRAMES);
        assert!(q.dropped() >= 30, "{}", q.dropped());
    }

    #[test]
    fn zero_gain_does_not_trigger_silence_detection() {
        let frames = Frames::new();
        let gain = Gain::default();
        gain.set(0);
        let mut chain = Chain::new(48_000, 1, gain.clone(), &Denoise::default());
        let mut hits = 0;
        for _ in 0..200 {
            if chain.feed(&[0.3; 1024], &frames) {
                hits += 1;
            }
        }
        assert_eq!(hits, 0);
        // 输出为静音，修改增益后立即生效。
        let q = frames.queue.lock().unwrap().pop().unwrap();
        assert!(q.iter().all(|&s| s == 0));
        gain.set(100);
        let f2 = Frames::new();
        chain.feed(&vec![0.3; FRAME_SAMPLES * 2], &f2);
        let mut last = None;
        while let Some(x) = f2.queue.lock().unwrap().pop() {
            last = Some(x);
        }
        assert!(last.unwrap().iter().any(|&s| s > 9000));
    }

    #[test]
    fn frames_pop_reports_drops_once() {
        let f = Frames::new();
        f.push((0..7).map(|i| vec![i; 2]).collect());
        let mut reported = 0;
        assert_eq!(f.pop(&mut reported), Some((vec![2, 2], 2)));
        assert_eq!(f.pop(&mut reported), Some((vec![3, 3], 0)));
    }
}
