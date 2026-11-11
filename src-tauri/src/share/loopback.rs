//! 屏幕声音：WASAPI 进程环回采集，每 10 ms 推 480 个采样点给 SDK。
//! 显示器共享排除本进程树，窗口共享只采目标进程树。48 kHz 立体声 S16。
//!
//! 没有成熟库同时提供排除模式与包含模式：`wasapi` crate 只封装包含模式，
//! 因此直接用 `windows` crate 调用公开 API，做法与探测程序 `golive358.cpp` 一致。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceCompletionHandler, IAudioCaptureClient,
    IAudioClient, PROCESS_LOOPBACK_MODE, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVE_FORMAT_PCM, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::Interface;

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// 每次推送的每声道采样数，即 10 ms。
pub const FRAMES: usize = 480;
/// 缓冲上限 200 ms，超出丢弃最旧样本，避免延迟累积。
const MAX_BUFFER: usize = RATE as usize / 5 * CHANNELS;
/// 激活等待上限。
const ACTIVATE_TIMEOUT_MS: u32 = 10_000;

/// 采集范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 系统声音，排除该进程树。显示器共享时传本进程。
    Exclude(u32),
    /// 只采该进程树。窗口共享时传窗口所属进程。
    Include(u32),
}

impl Scope {
    fn params(self) -> (u32, PROCESS_LOOPBACK_MODE) {
        match self {
            Self::Exclude(pid) => (pid, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE),
            Self::Include(pid) => (pid, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE),
        }
    }
}

/// 交错立体声样本缓冲。
#[derive(Default)]
struct Ring(VecDeque<i16>);

impl Ring {
    fn push(&mut self, samples: impl IntoIterator<Item = i16>) {
        self.0.extend(samples);
        let over = self.0.len().saturating_sub(MAX_BUFFER);
        self.0.drain(..over);
    }

    /// 取满一帧，不足补零。
    fn take(&mut self, out: &mut [i16]) {
        for s in out {
            *s = self.0.pop_front().unwrap_or(0);
        }
    }
}

/// 采集与推送两个线程。释放时停止并等待退出。
pub struct Loopback {
    running: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

/// 推送前的声音观察者。
#[cfg(any(test, feature = "test-media"))]
pub type Tap = Box<dyn Fn(&[i16]) + Send>;

/// 推送前的声音观察者，用于测试测量推出的内容。
#[cfg(any(test, feature = "test-media"))]
pub static TAP: Mutex<Option<Tap>> = Mutex::new(None);

impl Loopback {
    /// 开始采集并推送。`push` 在推送线程中每 10 ms 调用一次，参数为 480 个交错立体声采样点。
    /// 激活失败时推送线程照常推静音，不影响画面。
    pub fn start(scope: Scope, push: impl Fn(&[i16]) + Send + 'static) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let ring = Arc::new(Mutex::new(Ring::default()));
        let (r, q) = (running.clone(), ring.clone());
        let capture = std::thread::Builder::new()
            .name("share-loopback".into())
            .spawn(move || {
                if let Err(e) = capture(scope, &r, &q) {
                    log::warn!("屏幕声音采集失败：{e}");
                }
            });
        let (r, q) = (running.clone(), ring);
        let pusher = std::thread::Builder::new()
            .name("share-audio-push".into())
            .spawn(move || pump(&r, &q, &push));
        let threads = [capture, pusher]
            .into_iter()
            .filter_map(|t| {
                t.inspect_err(|e| log::warn!("屏幕声音线程启动失败：{e}"))
                    .ok()
            })
            .collect();
        Self { running, threads }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// 按绝对时间每 10 ms 推一帧，避免累积漂移。
fn pump(running: &AtomicBool, ring: &Mutex<Ring>, push: &impl Fn(&[i16])) {
    let mut frame = vec![0i16; FRAMES * CHANNELS];
    let start = Instant::now();
    let mut n: u32 = 0;
    while running.load(Ordering::SeqCst) {
        if let Ok(mut r) = ring.lock() {
            r.take(&mut frame);
        }
        #[cfg(any(test, feature = "test-media"))]
        if let Ok(t) = TAP.lock()
            && let Some(f) = t.as_ref()
        {
            f(&frame);
        }
        push(&frame);
        n = n.wrapping_add(1);
        let due = start + Duration::from_millis(10) * n;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
}

// `implement` 宏在结构体之外生成代码，lint 只能在模块级关闭。
#[allow(clippy::ref_as_ptr, clippy::inline_always)]
mod handler {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Media::Audio::{
        IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Impl,
    };
    use windows::Win32::System::Threading::SetEvent;
    use windows::core::implement;

    /// 激活完成时置位事件。`implement` 宏自动提供 `IAgileObject`。
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    pub(super) struct Activated {
        pub(super) done: HANDLE,
    }

    impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
        fn ActivateCompleted(
            &self,
            _op: windows::core::Ref<IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            // SAFETY: done 为激活线程创建的有效事件，激活完成前不会关闭。
            unsafe { SetEvent(self.done) }
        }
    }
}
use handler::Activated;

fn format() -> WAVEFORMATEX {
    let block = u16::try_from(CHANNELS * 2).unwrap_or(4);
    WAVEFORMATEX {
        wFormatTag: u16::try_from(WAVE_FORMAT_PCM).unwrap_or(1),
        nChannels: u16::try_from(CHANNELS).unwrap_or(2),
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

/// 激活进程环回设备。完成回调实现 `IAgileObject`，由 `implement` 宏自动提供。
fn activate(scope: Scope) -> windows::core::Result<IAudioClient> {
    let (pid, mode) = scope.params();
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: mode,
            },
        },
    };
    // windows crate 的 PROPVARIANT 析构时调用 PropVariantClear，会用 CoTaskMemFree 释放 VT_BLOB 的数据。
    // 这里的数据在栈上，因此不能析构。
    let pv = std::mem::ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: u32::try_from(std::mem::size_of_val(&params)).unwrap_or(0),
                        pBlobData: (&raw mut params).cast(),
                    },
                },
                ..Default::default()
            }),
        },
    });
    // SAFETY: 无名手动重置事件。
    let done = unsafe { CreateEventW(None, true, false, None)? };
    let handler: IActivateAudioInterfaceCompletionHandler = Activated { done }.into();
    // SAFETY: pv 与 params 在调用期间有效。pv 不析构，见上。
    let op = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const *pv),
            &handler,
        )?
    };
    // SAFETY: done 有效。
    let waited = unsafe { WaitForSingleObject(done, ACTIVATE_TIMEOUT_MS) };
    // SAFETY: done 不再使用。
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(done) };
    if waited != WAIT_OBJECT_0 {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_FAIL,
        ));
    }
    let mut hr = windows::core::HRESULT(0);
    let mut unk = None;
    // SAFETY: 激活已完成，输出指向本地变量。
    unsafe { op.GetActivateResult(&raw mut hr, &raw mut unk)? };
    hr.ok()?;
    unk.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?
        .cast()
}

/// 采集线程。COM 须在 MTA 中初始化，激活回调在 MTA 线程触发。
fn capture(scope: Scope, running: &AtomicBool, ring: &Mutex<Ring>) -> windows::core::Result<()> {
    // SAFETY: 本线程首次初始化 COM，退出前对应调用 CoUninitialize。
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let r = capture_loop(scope, running, ring);
    // SAFETY: 与上面的 CoInitializeEx 配对。COM 对象已在 capture_loop 中释放。
    unsafe { CoUninitialize() };
    r
}

fn capture_loop(
    scope: Scope,
    running: &AtomicBool,
    ring: &Mutex<Ring>,
) -> windows::core::Result<()> {
    let client = activate(scope)?;
    let wf = format();
    // 进程环回设备不支持 GetMixFormat，自行指定格式。缓冲 200 ms，单位 100 ns。
    // SAFETY: wf 在调用期间有效。
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            2_000_000,
            0,
            &raw const wf,
            None,
        )?;
    }
    // SAFETY: 已初始化。
    let cap: IAudioCaptureClient = unsafe { client.GetService()? };
    // SAFETY: 已初始化。
    unsafe { client.Start()? };
    log::info!("屏幕声音采集已开始：{scope:?}");
    let silent = u32::try_from(AUDCLNT_BUFFERFLAGS_SILENT.0).unwrap_or(2);
    while running.load(Ordering::SeqCst) {
        // SAFETY: 采集客户端有效，GetBuffer 与 ReleaseBuffer 成对调用。
        unsafe {
            while cap.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                cap.GetBuffer(&raw mut data, &raw mut frames, &raw mut flags, None, None)?;
                let n = frames as usize * CHANNELS;
                if let Ok(mut r) = ring.lock() {
                    if flags & silent != 0 || data.is_null() {
                        r.push(std::iter::repeat_n(0, n));
                    } else {
                        // 按字节读取，不假设缓冲按 i16 对齐。
                        let bytes = std::slice::from_raw_parts(data, n * 2);
                        r.push(
                            bytes
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|b| i16::from_le_bytes(*b)),
                        );
                    }
                }
                cap.ReleaseBuffer(frames)?;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // SAFETY: 已开始。
    let _ = unsafe { client.Stop() };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest_and_pads() {
        let mut r = Ring::default();
        r.push(0..i16::try_from(MAX_BUFFER + 10).unwrap());
        assert_eq!(r.0.len(), MAX_BUFFER);
        assert_eq!(r.0.front(), Some(&10));
        let mut out = vec![1i16; 4];
        let mut empty = Ring::default();
        empty.take(&mut out);
        assert_eq!(out, [0, 0, 0, 0]);
    }

    #[test]
    fn format_is_48k_stereo_s16() {
        let f = format();
        assert_eq!(
            (
                f.nSamplesPerSec,
                f.nChannels,
                f.wBitsPerSample,
                f.nBlockAlign
            ),
            (48_000, 2, 16, 4)
        );
    }

    /// 本机需有声音设备。排除本进程，采集 1 秒，确认有数据流入。
    #[test]
    #[ignore = "需要声音设备"]
    fn captures_system_audio() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        let lb = Loopback::start(Scope::Exclude(std::process::id()), move |f| {
            assert_eq!(f.len(), FRAMES * CHANNELS);
            c.fetch_add(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_secs(1));
        drop(lb);
        let n = count.load(Ordering::SeqCst);
        assert!((90..=110).contains(&n), "1 秒推送 {n} 帧");
    }

    fn db(x: &[f32], f: f32) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let (len, w) = (x.len() as f32, std::f32::consts::TAU * f / 48_000.0);
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &v) in x.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let a = w * i as f32;
            re += v * a.cos();
            im -= v * a.sin();
        }
        20.0 * ((re * re + im * im).sqrt() / len + 1e-9).log10()
    }

    /// 按环境变量指定的范围采集 2 秒，打印各频率能量。用于检验其他进程的排除与包含：
    /// `CDAPP_LOOPBACK=exclude:<pid>` 或 `include:<pid>`，`CDAPP_LOOPBACK_FREQS=440,1000`。
    #[test]
    #[ignore = "需要声音设备与外部声源"]
    fn measures_scope() {
        let spec = std::env::var("CDAPP_LOOPBACK").expect("CDAPP_LOOPBACK");
        let (mode, pid) = spec.split_once(':').expect("格式为 exclude:<pid>");
        let pid: u32 = pid.parse().unwrap();
        let scope = if mode == "include" {
            Scope::Include(pid)
        } else {
            Scope::Exclude(pid)
        };
        let freqs: Vec<f32> = std::env::var("CDAPP_LOOPBACK_FREQS")
            .unwrap_or_else(|_| "440,1000".into())
            .split(',')
            .filter_map(|f| f.parse().ok())
            .collect();
        let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
        let b = buf.clone();
        let lb = Loopback::start(scope, move |f| {
            b.lock()
                .unwrap()
                .extend(f.iter().step_by(2).map(|&s| f32::from(s) / 32768.0));
        });
        std::thread::sleep(Duration::from_millis(2500));
        drop(lb);
        let x = buf.lock().unwrap();
        // 取最后 1 秒，跳过激活前的静音。
        let tail = &x[x.len().saturating_sub(48_000)..];
        for f in freqs {
            println!("{scope:?} {f} Hz {:.1} dB", db(tail, f));
        }
    }
}
