//! 麦克风测试：把输入按当前增益与输出音量直接播放到扬声器，并上报输入电平。
//! 推流采集链路绑定发送任务，不能单独打开，因此另建一条只在本机回放的链路。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, SampleFormat, StreamConfig};

use super::dsp::{Gain, RATE, Resampler, audio_level, meter, mixdown, soft_limit};

/// 回放缓冲上限，200 ms。超过时丢弃最旧的样本，避免延迟累积。
const MAX_BUFFER: usize = RATE as usize / 5;
/// 输出侧每次从缓冲取出的样本数，10 ms。
const PULL: usize = RATE as usize / 100;
/// 电平上报间隔。
const METER_INTERVAL: Duration = Duration::from_millis(60);

/// 一次麦克风测试。丢弃时停止并释放设备。
pub struct Loopback {
    stop: Arc<AtomicBool>,
    master: Arc<AtomicU16>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Loopback {
    /// 启动测试线程。`on_level` 收到 0 至 100 的电平，`on_fail` 收到失败原因，失败后线程退出。
    #[must_use]
    pub fn start(
        input: Option<String>,
        output: Option<String>,
        gain: Gain,
        master: u16,
        on_level: impl Fn(u8) + Send + 'static,
        on_fail: impl Fn(String) + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let master = Arc::new(AtomicU16::new(master));
        let (st, ms) = (stop.clone(), master.clone());
        let thread = std::thread::Builder::new()
            .name("mic-test".into())
            .spawn(move || {
                if let Err(e) = run(
                    input.as_deref(),
                    output.as_deref(),
                    &gain,
                    &ms,
                    &st,
                    &on_level,
                ) {
                    on_fail(e);
                }
            })
            .ok();
        Self {
            stop,
            master,
            thread,
        }
    }

    /// 修改输出音量，立即生效。
    pub fn set_master(&self, percent: u16) {
        self.master.store(percent, Ordering::Relaxed);
    }

    /// 测试线程是否已退出。
    #[must_use]
    pub fn finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn pick(id: Option<&str>, input: bool) -> Option<Device> {
    let host = cpal::default_host();
    id.and_then(|id| id.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
        .or_else(|| {
            if input {
                host.default_input_device()
            } else {
                host.default_output_device()
            }
        })
}

/// 优先使用 48 kHz，保持设备默认的声道与样本格式。
fn config_of(
    default: cpal::SupportedStreamConfig,
    ranges: Option<impl Iterator<Item = cpal::SupportedStreamConfigRange>>,
) -> cpal::SupportedStreamConfig {
    ranges
        .and_then(|mut it| {
            it.find(|r| {
                r.channels() == default.channels()
                    && r.sample_format() == default.sample_format()
                    && r.min_sample_rate() <= RATE
                    && r.max_sample_rate() >= RATE
            })
        })
        .map_or(default, |r| r.with_sample_rate(RATE))
}

type Shared = Arc<Mutex<VecDeque<f32>>>;
type Errors = std::sync::mpsc::Sender<(&'static str, cpal::Error)>;

fn run(
    input: Option<&str>,
    output: Option<&str>,
    gain: &Gain,
    master: &Arc<AtomicU16>,
    stop: &AtomicBool,
    on_level: &dyn Fn(u8),
) -> Result<(), String> {
    let buffer: Shared = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFER)));
    let level = Arc::new(AtomicU8::new(127));
    let (err_tx, err_rx) = std::sync::mpsc::channel();

    let mic = pick(input, true).ok_or("没有可用的麦克风")?;
    let spk = pick(output, false).ok_or("没有可用的扬声器")?;
    let in_stream = open_input(&mic, gain, &buffer, &level, &err_tx)
        .map_err(|e| format!("无法打开麦克风：{e}"))?;
    let out_stream =
        open_output(&spk, master, &buffer, &err_tx).map_err(|e| format!("无法打开扬声器：{e}"))?;

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(METER_INTERVAL);
        on_level(meter(level.swap(127, Ordering::Relaxed)));
        if let Ok((what, e)) = err_rx.try_recv() {
            match e.kind() {
                ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated => {
                    return Err(format!("{what}已断开"));
                }
                ErrorKind::DeviceChanged | ErrorKind::Xrun => {}
                _ => log::warn!("麦克风测试{what}流错误：{e}"),
            }
        }
    }
    drop((in_stream, out_stream));
    on_level(0);
    Ok(())
}

/// 输入：混为单声道、增益、重采样到 48 kHz 后放入缓冲。
#[allow(clippy::cast_possible_truncation)]
fn open_input(
    mic: &Device,
    gain: &Gain,
    buffer: &Shared,
    level: &Arc<AtomicU8>,
    errors: &Errors,
) -> Result<cpal::Stream, String> {
    let cfg = config_of(
        mic.default_input_config().map_err(|e| e.to_string())?,
        mic.supported_input_configs().ok(),
    );
    let conf: StreamConfig = cfg.config();
    let ch = usize::from(conf.channels.max(1));
    let mut rs = Resampler::new(conf.sample_rate, RATE);
    let (buf, lv, gain) = (buffer.clone(), level.clone(), gain.clone());
    let mut scratch: Vec<f32> = Vec::new();
    let mut on_input = move |data: &[f32]| {
        let mut mono = mixdown(data, ch);
        gain.apply(&mut mono);
        let pcm: Vec<i16> = mono
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        // 取上报周期内的最大电平，即最小的 -dBov 值。
        lv.fetch_min(audio_level(&pcm), Ordering::Relaxed);
        let out = rs.process(&mono);
        if let Ok(mut b) = buf.lock() {
            b.extend(out);
            let over = b.len().saturating_sub(MAX_BUFFER);
            b.drain(..over);
        }
    };
    macro_rules! build {
        ($t:ty) => {{
            let err = errors.clone();
            mic.build_input_stream::<$t, _, _>(
                conf,
                move |d: &[$t], _| {
                    scratch.clear();
                    scratch.extend(
                        d.iter()
                            .map(|&s| <f32 as cpal::FromSample<$t>>::from_sample_(s)),
                    );
                    on_input(&scratch);
                },
                move |e| {
                    let _ = err.send(("麦克风", e));
                },
                None,
            )
        }};
    }
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U16 => build!(u16),
        f => return Err(format!("不支持的样本格式 {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

/// 输出：从缓冲取样本，乘以输出音量，重采样到设备采样率后写入各声道。
fn open_output(
    spk: &Device,
    master: &Arc<AtomicU16>,
    buffer: &Shared,
    errors: &Errors,
) -> Result<cpal::Stream, String> {
    let cfg = config_of(
        spk.default_output_config().map_err(|e| e.to_string())?,
        spk.supported_output_configs().ok(),
    );
    let conf: StreamConfig = cfg.config();
    let ch = usize::from(conf.channels.max(1));
    let mut rs = Resampler::new(RATE, conf.sample_rate);
    let (buf, ms) = (buffer.clone(), master.clone());
    let mut pending: VecDeque<f32> = VecDeque::new();
    let mut chunk = vec![0.0f32; PULL];
    let mut fill = move |out: &mut [f32]| {
        let frames = out.len() / ch;
        while pending.len() < frames {
            chunk.fill(0.0);
            if let Ok(mut b) = buf.lock() {
                let n = b.len().min(PULL);
                for (d, s) in chunk.iter_mut().zip(b.drain(..n)) {
                    *d = s;
                }
            }
            let g = f32::from(ms.load(Ordering::Relaxed)) / 100.0;
            for x in &mut chunk {
                *x = soft_limit(*x * g);
            }
            pending.extend(rs.process(&chunk));
        }
        for (frame, s) in out.chunks_mut(ch).zip(pending.drain(..frames)) {
            frame.fill(s);
        }
    };
    let mut mix: Vec<f32> = Vec::new();
    macro_rules! build {
        ($t:ty) => {{
            let err = errors.clone();
            spk.build_output_stream::<$t, _, _>(
                conf,
                move |d: &mut [$t], _| {
                    mix.resize(d.len(), 0.0);
                    fill(&mut mix);
                    for (o, &s) in d.iter_mut().zip(mix.iter()) {
                        *o = <$t as cpal::FromSample<f32>>::from_sample_(s);
                    }
                },
                move |e| {
                    let _ = err.send(("扬声器", e));
                },
                None,
            )
        }};
    }
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U16 => build!(u16),
        f => return Err(format!("不支持的样本格式 {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}
