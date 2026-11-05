//! 音频偏好与控制命令，未连接时只更新偏好。

use super::{
    AppResult, Arc, Deafen, HashMap, MicStatus, Mutex, Source, VoiceEvent, oneshot, output,
};

/// 混音输出的录制缓冲。
#[cfg(any(test, feature = "test-media"))]
pub type Tap = Arc<Mutex<Vec<f32>>>;

/// 会话的音频偏好。
/// 会话内的修改会写回，重建连接时沿用。
#[derive(Clone)]
pub struct MicPrefs {
    pub muted: bool,
    pub source: Source,
    pub deafen: Deafen,
    pub output: output::Sink,
    /// 成员音量百分比，按用户 ID 记录。
    pub volumes: HashMap<String, u16>,
    /// 总音量百分比。
    pub master: u16,
    /// 把混音输出追加到该缓冲。
    #[cfg(any(test, feature = "test-media"))]
    pub tap: Option<Tap>,
    /// 下一次加入直接失败。
    #[cfg(any(test, feature = "test-media"))]
    pub fail_join: bool,
}

impl Default for MicPrefs {
    fn default() -> Self {
        Self {
            muted: false,
            source: Source::Device(None),
            deafen: Deafen::default(),
            output: output::Sink::Device(None),
            volumes: HashMap::new(),
            master: 100,
            #[cfg(any(test, feature = "test-media"))]
            tap: None,
            #[cfg(any(test, feature = "test-media"))]
            fail_join: false,
        }
    }
}

pub(super) type Prefs = Arc<Mutex<MicPrefs>>;

pub(super) fn prefs_get(p: &Prefs) -> MicPrefs {
    p.lock().map(|g| g.clone()).unwrap_or_default()
}

pub type Sink = Arc<dyn Fn(VoiceEvent) + Send + Sync>;

pub(super) enum Ctl {
    Leave,
    /// 设置麦克风静音，结果经通道返回。
    Mute(bool, oneshot::Sender<AppResult<()>>),
    /// 切换采集来源。
    Source(Source),
    /// 设置耳机关闭，结果经通道返回。
    Deafen(bool, oneshot::Sender<AppResult<()>>),
    /// 切换播放设备。
    Output(output::Sink),
    /// 设置成员音量。
    Volume(String, u16),
    /// 设置总音量。
    Master(u16),
    /// 系统唤醒。等待重建时立即重试，媒体连接已断开时立即重建。
    Wake,
    /// 注入故障。
    #[cfg(any(test, feature = "test-media"))]
    Fault(Fault),
}

/// 可注入的故障。
#[cfg(any(test, feature = "test-media"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// 关闭信令连接。
    CloseSignal,
    /// 丢弃收到的媒体包。
    DropMedia,
    /// 模拟系统睡眠。
    Sleep,
    /// 关闭信令连接，并使之后的加入失败。
    FailJoin,
}

/// 构造扬声器输出事件。
pub(super) fn output_event(
    deafened: bool,
    active: bool,
    device: Option<String>,
    message: Option<String>,
) -> VoiceEvent {
    VoiceEvent::Output {
        deafened,
        active,
        device,
        message,
    }
}

/// 更新耳机偏好，返回需要同步的麦克风静音状态。
pub(super) fn deafen_prefs(prefs: &Prefs, on: bool) -> Option<bool> {
    let mut p = prefs.lock().ok()?;
    let muted = p.muted;
    let change = p.deafen.set(on, muted);
    if let Some(m) = change {
        p.muted = m;
    }
    change
}

/// 更新麦克风静音偏好。打开麦克风会同时打开耳机，此时返回 true。
pub(super) fn mute_prefs(prefs: &Prefs, muted: bool) -> bool {
    prefs.lock().is_ok_and(|mut p| {
        p.muted = muted;
        p.deafen.on_mic(muted)
    })
}

/// 未连接时处理控制命令，只更新偏好并上报状态。
pub(super) fn offline_ctl(c: Ctl, sink: &Sink, prefs: &Prefs) {
    match c {
        Ctl::Mute(muted, tx) => {
            if mute_prefs(prefs, muted) {
                sink(output_event(false, false, None, None));
            }
            sink(VoiceEvent::Mic {
                status: if muted {
                    MicStatus::Muted
                } else {
                    MicStatus::Off
                },
                muted,
                device: None,
                message: None,
            });
            let _ = tx.send(Ok(()));
        }
        Ctl::Source(src) => {
            if let Ok(mut p) = prefs.lock() {
                p.source = src;
            }
        }
        Ctl::Deafen(on, tx) => {
            let change = deafen_prefs(prefs, on);
            sink(output_event(on, false, None, None));
            if let Some(muted) = change {
                sink(VoiceEvent::Mic {
                    status: if muted {
                        MicStatus::Muted
                    } else {
                        MicStatus::Off
                    },
                    muted,
                    device: None,
                    message: None,
                });
            }
            let _ = tx.send(Ok(()));
        }
        Ctl::Output(o) => {
            if let Ok(mut p) = prefs.lock() {
                p.output = o;
            }
        }
        Ctl::Volume(peer, v) => {
            if let Ok(mut p) = prefs.lock() {
                p.volumes
                    .insert(peer, v.min(crate::audio::mixer::MAX_VOLUME));
            }
        }
        Ctl::Master(v) => {
            if let Ok(mut p) = prefs.lock() {
                p.master = v.min(crate::audio::mixer::MAX_VOLUME);
            }
        }
        // 退出与唤醒由调用方处理。
        Ctl::Leave | Ctl::Wake => {}
        #[cfg(any(test, feature = "test-media"))]
        Ctl::Fault(_) => {}
    }
}
