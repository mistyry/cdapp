//! 扬声器输出。
//! 播放线程打开输出设备，在设备回调中从混音器取样本，按设备采样率重采样并复制到各声道。
//! 设备丢失时改用系统默认设备，停止时等待线程退出并释放设备。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, SampleFormat, StreamConfig};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use super::dsp::{RATE, Resampler};
use super::mixer::Mixer;

/// 打开设备的时限。
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// 每次从混音器取出的样本数，10 ms。
const CHUNK: usize = 480;

static LIVE_THREADS: AtomicUsize = AtomicUsize::new(0);

/// 存活的播放线程数，用于检查资源释放。
#[must_use]
pub fn live_outputs() -> usize {
    LIVE_THREADS.load(Ordering::SeqCst)
}

/// 输出设备。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// 列出输出设备，不打开设备。
///
/// # Errors
///
/// 系统无法枚举设备时返回错误。
pub fn output_devices() -> Result<Vec<OutputDevice>, String> {
    let host = cpal::default_host();
    let default = host
        .default_output_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());
    let list = host.output_devices().map_err(|e| e.to_string())?;
    Ok(list
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            Some(OutputDevice {
                default: default.as_deref() == Some(id.as_str()),
                name: device_name(&d),
                id,
            })
        })
        .collect())
}

/// 播放目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sink {
    /// 扬声器设备，None 为系统默认。
    Device(Option<String>),
    /// 不打开设备，只消耗混音输出。
    #[cfg(any(test, feature = "test-media"))]
    Null,
}

/// 播放线程发出的事件，`epoch` 用于区分播放轮次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputEvent {
    Started { epoch: u64, device: String },
    Failed { epoch: u64, message: String },
}

/// 一次播放。持有播放线程。
pub struct Playback {
    stop: Arc<AtomicBool>,
    wake: std::sync::mpsc::Sender<()>,
    released: oneshot::Receiver<()>,
    pub epoch: u64,
}

impl Playback {
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn start(
        sink: Sink,
        mixer: Arc<Mutex<Mixer>>,
        epoch: u64,
        events: mpsc::UnboundedSender<OutputEvent>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (wake, wait) = std::sync::mpsc::channel();
        let (released_tx, released) = oneshot::channel();
        let (st, tx) = (stop.clone(), events.clone());
        let spawned = std::thread::Builder::new()
            .name("speaker-out".into())
            .spawn(move || {
                LIVE_THREADS.fetch_add(1, Ordering::SeqCst);
                match sink {
                    #[cfg(any(test, feature = "test-media"))]
                    Sink::Null => run_null(&mixer, &st, &wait, epoch, &tx),
                    Sink::Device(id) => run_device(id.as_deref(), &mixer, &st, &wait, epoch, &tx),
                }
                LIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                let _ = released_tx.send(());
            });
        if let Err(e) = spawned {
            let _ = events.send(OutputEvent::Failed {
                epoch,
                message: format!("无法启动播放线程：{e}"),
            });
        }
        Self {
            stop,
            wake,
            released,
            epoch,
        }
    }

    /// 线程永不退出的句柄，用于测试停止超时。
    #[cfg(test)]
    #[must_use]
    pub fn stuck() -> Self {
        let (wake, wait) = std::sync::mpsc::channel();
        let (released_tx, released) = oneshot::channel::<()>();
        std::mem::forget((wait, released_tx));
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            wake,
            released,
            epoch: 0,
        }
    }

    /// 停止播放并等待设备释放，最多 2 秒。
    pub async fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.wake.send(());
        if tokio::time::timeout(Duration::from_secs(2), self.released)
            .await
            .is_err()
        {
            log::warn!("播放线程未在时限内退出");
        }
    }
}

/// 从混音器取样本，转换为设备的采样率与声道数。
pub struct Render {
    mixer: Arc<Mutex<Mixer>>,
    resampler: Resampler,
    channels: usize,
    pending: VecDeque<f32>,
    chunk: Vec<f32>,
}

impl Render {
    #[must_use]
    pub fn new(mixer: Arc<Mutex<Mixer>>, rate: u32, channels: usize) -> Self {
        Self {
            mixer,
            resampler: Resampler::new(RATE, rate),
            channels: channels.max(1),
            pending: VecDeque::new(),
            chunk: vec![0.0; CHUNK],
        }
    }

    /// 填满一段交错输出缓冲。
    pub fn fill(&mut self, out: &mut [f32]) {
        let frames = out.len() / self.channels;
        while self.pending.len() < frames {
            if let Ok(mut m) = self.mixer.lock() {
                m.render(&mut self.chunk);
            } else {
                self.chunk.fill(0.0);
            }
            let r = self.resampler.process(&self.chunk);
            self.pending.extend(r);
        }
        for (frame, s) in out
            .chunks_mut(self.channels)
            .zip(self.pending.drain(..frames))
        {
            frame.fill(s);
        }
    }
}

/// 不打开设备，按实时节奏消耗混音输出。
#[cfg(any(test, feature = "test-media"))]
fn run_null(
    mixer: &Arc<Mutex<Mixer>>,
    stop: &AtomicBool,
    wait: &std::sync::mpsc::Receiver<()>,
    epoch: u64,
    events: &mpsc::UnboundedSender<OutputEvent>,
) {
    let _ = events.send(OutputEvent::Started {
        epoch,
        device: "无设备输出".into(),
    });
    let mut render = Render::new(mixer.clone(), RATE, 1);
    let mut buf = vec![0.0; CHUNK];
    let start = std::time::Instant::now();
    let mut k: u32 = 0;
    while !stop.load(Ordering::SeqCst) {
        maintain(mixer);
        render.fill(&mut buf);
        k += 1;
        let due = start + Duration::from_millis(10) * k;
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

/// 为开启降噪的成员创建 KOOK AI 降噪实例。只在取列表时持有混音锁。
fn maintain(mixer: &Arc<Mutex<Mixer>>) {
    let list = mixer.lock().map(|m| m.denoisers()).unwrap_or_default();
    for d in list {
        d.maintain();
    }
}

fn device_name(d: &Device) -> String {
    d.description()
        .map_or_else(|_| d.to_string(), |x| x.name().to_owned())
}

/// 查找设备。所选设备不可用时改用系统默认设备，返回值第二项表示是否已改用。
fn find_device(id: Option<&str>) -> Option<(Device, bool)> {
    let host = cpal::default_host();
    if let Some(d) = id
        .and_then(|id| id.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
    {
        return Some((d, false));
    }
    host.default_output_device().map(|d| (d, id.is_some()))
}

/// 打开并启动输出流。设备支持 48 kHz 时优先使用。
fn open(
    device: &Device,
    mixer: &Arc<Mutex<Mixer>>,
    signal: &std::sync::mpsc::Sender<cpal::Error>,
) -> Result<cpal::Stream, String> {
    let fail = |e: cpal::Error| format!("无法打开扬声器：{e}");
    let cfg = device.default_output_config().map_err(fail)?;
    let cfg = device
        .supported_output_configs()
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
    let mut render = Render::new(
        mixer.clone(),
        config.sample_rate,
        usize::from(config.channels.max(1)),
    );
    let mut scratch: Vec<f32> = Vec::new();
    macro_rules! build {
        ($t:ty) => {{
            let err = signal.clone();
            device.build_output_stream::<$t, _, _>(
                config,
                move |d: &mut [$t], _| {
                    scratch.resize(d.len(), 0.0);
                    render.fill(&mut scratch);
                    for (o, &s) in d.iter_mut().zip(scratch.iter()) {
                        *o = <$t as cpal::FromSample<f32>>::from_sample_(s);
                    }
                },
                move |e| {
                    let _ = err.send(e);
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
        f => return Err(format!("不支持的样本格式 {f:?}")),
    }
    .map_err(fail)?;
    stream.play().map_err(fail)?;
    Ok(stream)
}

/// 播放线程：打开设备并监视错误，设备丢失时改用系统默认设备。
fn run_device(
    id: Option<&str>,
    mixer: &Arc<Mutex<Mixer>>,
    stop: &AtomicBool,
    wait: &std::sync::mpsc::Receiver<()>,
    epoch: u64,
    events: &mpsc::UnboundedSender<OutputEvent>,
) {
    let failed = |message| {
        let _ = events.send(OutputEvent::Failed { epoch, message });
    };
    // 所选设备不可用时改用系统默认设备，并定期检查，所选设备恢复后自动切回。
    let chosen = id.map(str::to_owned);
    let mut target = chosen.clone();
    loop {
        let Some((device, fell_back)) = find_device(target.as_deref()) else {
            return failed("没有可用的扬声器".into());
        };
        if fell_back {
            log::warn!(
                "所选扬声器不可用，改用系统默认设备 {}",
                device_name(&device)
            );
            target = None;
        }
        let (sig_tx, sig_rx) = std::sync::mpsc::channel();
        let stream = match open(&device, mixer, &sig_tx) {
            Ok(s) => s,
            Err(m) => return failed(m),
        };
        // 清空积压的音频。
        if let Ok(mut m) = mixer.lock() {
            m.clear_all();
        }
        let _ = events.send(OutputEvent::Started {
            epoch,
            device: device_name(&device),
        });
        let mut last_probe = std::time::Instant::now();
        let lost = loop {
            // 成员降噪实例在此创建，不占用设备回调的时间。
            maintain(mixer);
            if target.is_none()
                && let Some(c) = chosen.as_deref()
                && last_probe.elapsed() >= crate::audio::capture::RESTORE_PROBE
            {
                last_probe = std::time::Instant::now();
                if find_device(Some(c)).is_some_and(|(_, fell_back)| !fell_back) {
                    log::info!("所选扬声器已恢复，切回该设备");
                    target.clone_from(&chosen);
                    break None;
                }
            }
            if stop.load(Ordering::SeqCst)
                || !matches!(
                    wait.recv_timeout(Duration::from_millis(100)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                )
            {
                return;
            }
            if let Ok(e) = sig_rx.try_recv() {
                match e.kind() {
                    ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated => break Some(e),
                    ErrorKind::DeviceChanged | ErrorKind::Xrun => {}
                    _ => log::warn!("扬声器流错误：{e}"),
                }
            }
        };
        drop(stream);
        let Some(lost) = lost else {
            continue;
        };
        log::warn!("扬声器 {} 断开：{lost}", device_name(&device));
        // 正在使用系统默认设备时：所选设备已恢复则切回，否则重新查找当前的默认设备。
        if target.is_none() {
            if let Some(c) = chosen.as_deref()
                && find_device(Some(c)).is_some_and(|(_, fell_back)| !fell_back)
            {
                target.clone_from(&chosen);
                continue;
            }
            if find_device(None).is_some() {
                log::info!("改用新的系统默认扬声器");
                continue;
            }
            log::warn!("没有可用的扬声器，停止播放");
            return failed(format!("扬声器已断开：{lost}"));
        }
        target = None;
    }
}

/// 耳机关闭状态。
/// 关闭耳机时同时静音麦克风，重新打开时恢复之前的麦克风状态。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Deafen {
    on: bool,
    mic_before: Option<bool>,
}

impl Deafen {
    #[must_use]
    pub fn new(on: bool) -> Self {
        Self {
            on,
            mic_before: on.then_some(false),
        }
    }

    /// 指定关闭耳机前的麦克风静音状态。未知时视为未静音。
    #[must_use]
    pub fn with_mic_before(on: bool, mic_before: Option<bool>) -> Self {
        Self {
            on,
            mic_before: on.then_some(mic_before.unwrap_or(false)),
        }
    }

    #[must_use]
    pub fn on(&self) -> bool {
        self.on
    }

    /// 设置耳机关闭，返回需要同步的麦克风静音状态。
    pub fn set(&mut self, on: bool, mic_muted: bool) -> Option<bool> {
        if on == self.on {
            return None;
        }
        self.on = on;
        if on {
            self.mic_before = Some(mic_muted);
            (!mic_muted).then_some(true)
        } else {
            let before = self.mic_before.take().unwrap_or(mic_muted);
            (before != mic_muted).then_some(before)
        }
    }

    /// 麦克风取消静音时同时打开耳机，返回耳机是否因此打开。
    pub fn on_mic(&mut self, muted: bool) -> bool {
        if self.on && !muted {
            self.on = false;
            self.mic_before = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deafened_join_keeps_muted_mic_on_undeafen() {
        // 离线时先静音再关闭耳机，加入后打开耳机不应打开麦克风。
        let mut d = Deafen::with_mic_before(true, Some(true));
        assert_eq!(d.set(false, true), None);
        let mut d = Deafen::with_mic_before(true, Some(false));
        assert_eq!(d.set(false, true), Some(false));
        let mut d = Deafen::with_mic_before(true, None);
        assert_eq!(d.set(false, true), Some(false));
        assert!(!Deafen::with_mic_before(false, Some(true)).on());
    }

    #[test]
    fn deafen_mutes_mic_and_restores_previous_state() {
        let mut d = Deafen::default();
        // 关闭耳机时静音麦克风，打开后恢复。
        assert_eq!(d.set(true, false), Some(true));
        assert!(d.on());
        assert_eq!(d.set(true, true), None, "重复设置");
        assert_eq!(d.set(false, true), Some(false));
        // 关闭前已静音，打开后保持静音。
        assert_eq!(d.set(true, true), None);
        assert_eq!(d.set(false, true), None);
        // 关闭期间取消静音会同时打开耳机。
        assert_eq!(d.set(true, false), Some(true));
        assert!(d.on_mic(false));
        assert!(!d.on());
        assert_eq!(d.set(false, false), None);
        // 静音麦克风不影响耳机。
        assert_eq!(d.set(true, true), None);
        assert!(!d.on_mic(true));
        assert!(d.on());
    }

    #[test]
    fn render_upsamples_and_duplicates_channels() {
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let mut r = Render::new(mixer, 44_100, 2);
        let mut out = vec![1.0; 882];
        r.fill(&mut out);
        // 没有音轨时输出静音。
        assert!(out.iter().all(|&x| x == 0.0));
        let mut r = Render::new(Arc::new(Mutex::new(Mixer::default())), RATE, 6);
        let mut out = vec![1.0; 6 * 100];
        r.fill(&mut out);
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn null_playback_thread_exits_on_stop() {
        // 线程计数为全进程共享，并发测试会同时增减，这里以本线程的退出信号为准。
        let (tx, mut rx) = mpsc::unbounded_channel();
        let p = Playback::start(Sink::Null, Arc::default(), 7, tx);
        assert!(matches!(
            rx.recv().await,
            Some(OutputEvent::Started { epoch: 7, .. })
        ));
        let Playback {
            stop,
            wake,
            released,
            ..
        } = p;
        stop.store(true, Ordering::SeqCst);
        let _ = wake.send(());
        let done = tokio::time::timeout(Duration::from_secs(1), released).await;
        assert!(matches!(done, Ok(Ok(()))), "停止后线程退出");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropped_playback_handle_ends_thread() {
        // 会话任务被中止时句柄直接丢弃，线程也应退出，而不是空转。
        let (tx, mut rx) = mpsc::unbounded_channel();
        let p = Playback::start(Sink::Null, Arc::default(), 8, tx);
        assert!(matches!(
            rx.recv().await,
            Some(OutputEvent::Started { epoch: 8, .. })
        ));
        let Playback { wake, released, .. } = p;
        drop(wake);
        let done = tokio::time::timeout(Duration::from_secs(1), released).await;
        assert!(matches!(done, Ok(Ok(()))), "丢弃句柄后线程应退出");
    }
}
