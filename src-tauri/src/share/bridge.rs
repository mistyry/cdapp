use std::ffi::{CStr, CString, c_char, c_void};

use tokio::sync::mpsc::UnboundedSender;

pub mod agora;
pub mod volc;

use agora::ffi as agora_ffi;
use volc::ffi as volc_ffi;

const NAME_LEN: usize = 256;

#[repr(C)]
struct RawSource {
    kind: i32,
    id: u64,
    pid: i32,
    primary: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    name: [c_char; NAME_LEN],
    application: [c_char; NAME_LEN],
}

#[repr(C)]
struct RawLive {
    app_id: *const c_char,
    room: *const c_char,
    uid: *const c_char,
    token: *const c_char,
    source_kind: i32,
    source_id: u64,
    width: i32,
    height: i32,
    frame_rate: i32,
    max_kbps: i32,
    audio: i32,
}

#[repr(C)]
struct RawWatch {
    app_id: *const c_char,
    room: *const c_char,
    uid: *const c_char,
    token: *const c_char,
    liver: *const c_char,
    volume: i32,
}

type EventFn = unsafe extern "C" fn(*mut c_void, i32, i32, i32, i32, i32);
type FrameFn = unsafe extern "C" fn(*mut c_void, *const u8, i32, i32, i32, i32, i32, i64);

const OK: i32 = 0;
const ERR_NOT_STARTED: i32 = -1;
const ERR_THREAD: i32 = -2;
const ERR_LOAD: i32 = -3;
const ERR_ENGINE: i32 = -4;
const ERR_SOURCE: i32 = -5;
const ERR_STATE: i32 = -6;
const ERR_JOIN: i32 = -10;
const ERR_ENCODER: i32 = -11;
const ERR_CAPTURE: i32 = -12;
const ERR_PUBLISH: i32 = -13;
const ERR_TOKEN: i32 = -14;
const ERR_AUDIO: i32 = -15;
const ERR_SUBSCRIBE: i32 = -16;
const ERR_VOLUME: i32 = -17;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BridgeError {
    #[error("屏幕分享线程未启动")]
    NotStarted,
    #[error("屏幕分享线程启动失败")]
    Thread,
    #[error("屏幕分享组件加载失败：系统错误 {0}")]
    Load(u32),
    #[error("屏幕分享引擎创建失败")]
    Engine,
    #[error("共享源不可用")]
    Source,
    #[error("屏幕分享状态不符")]
    State,
    #[error("进入分享房间失败：{0}")]
    Join(i32),
    #[error("编码设置失败：{0}")]
    Encoder(i32),
    #[error("屏幕采集失败：{0}")]
    Capture(i32),
    #[error("发布屏幕流失败：{0}")]
    Publish(i32),
    #[error("更换令牌失败：{0}")]
    Token(i32),
    #[error("推送屏幕声音失败：{0}")]
    Audio(i32),
    #[error("订阅屏幕流失败：{0}")]
    Subscribe(i32),
    #[error("调整屏幕共享音量失败：{0}")]
    Volume(i32),
    #[error("参数包含空字符")]
    Nul,
    #[error("屏幕分享组件返回未知错误 {0}")]
    Unknown(i32),
}

fn check(rc: i32, extra: i32) -> Result<(), BridgeError> {
    match rc {
        OK => Ok(()),
        ERR_NOT_STARTED => Err(BridgeError::NotStarted),
        ERR_THREAD => Err(BridgeError::Thread),
        ERR_LOAD => Err(BridgeError::Load(extra.cast_unsigned())),
        ERR_ENGINE => Err(BridgeError::Engine),
        ERR_SOURCE => Err(BridgeError::Source),
        ERR_STATE => Err(BridgeError::State),
        ERR_JOIN => Err(BridgeError::Join(extra)),
        ERR_ENCODER => Err(BridgeError::Encoder(extra)),
        ERR_CAPTURE => Err(BridgeError::Capture(extra)),
        ERR_PUBLISH => Err(BridgeError::Publish(extra)),
        ERR_TOKEN => Err(BridgeError::Token(extra)),
        ERR_AUDIO => Err(BridgeError::Audio(extra)),
        ERR_SUBSCRIBE => Err(BridgeError::Subscribe(extra)),
        ERR_VOLUME => Err(BridgeError::Volume(extra)),
        other => Err(BridgeError::Unknown(other)),
    }
}

fn cstr(s: &str) -> Result<CString, BridgeError> {
    CString::new(s).map_err(|_| BridgeError::Nul)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Window,
    Screen,
}

impl SourceKind {
    fn raw(self) -> i32 {
        match self {
            Self::Window => 1,
            Self::Screen => 2,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub kind: SourceKind,
    #[serde(serialize_with = "id_str")]
    pub id: u64,
    pub pid: u32,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    pub name: String,
    pub application: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn id_str<S: serde::Serializer>(id: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(id)
}

/// # Errors
///
/// 不是十进制整数时返回 [`BridgeError::Source`]。
pub fn parse_id(s: &str) -> Result<u64, BridgeError> {
    s.parse().map_err(|_| BridgeError::Source)
}

fn text(buf: &[c_char; NAME_LEN]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    RoomState(i32),
    FirstFrame {
        width: u32,
        height: u32,
    },
    SizeChanged {
        width: u32,
        height: u32,
    },
    TokenWillExpire,
    Stats {
        kbps: i32,
        fps: i32,
        width: i32,
        height: i32,
    },
    Error(i32),
    VideoState {
        state: i32,
        error: i32,
    },
    SourceGone,
    Paused(bool),
}

fn event(kind: i32, a: i32, b: i32, c: i32, d: i32) -> Option<Event> {
    let size = |w: i32, h: i32| (w.max(0).cast_unsigned(), h.max(0).cast_unsigned());
    Some(match kind {
        1 => Event::RoomState(a),
        2 => {
            let (width, height) = size(a, b);
            Event::FirstFrame { width, height }
        }
        3 => {
            let (width, height) = size(a, b);
            Event::SizeChanged { width, height }
        }
        4 => Event::TokenWillExpire,
        5 => Event::Stats {
            kbps: a,
            fps: b,
            width: c,
            height: d,
        },
        6 => Event::Error(a),
        7 => Event::VideoState { state: a, error: b },
        8 => Event::SourceGone,
        9 => Event::Paused(a != 0),
        _ => return None,
    })
}

unsafe extern "C" fn on_event(user: *mut c_void, kind: i32, a: i32, b: i32, c: i32, d: i32) {
    let tx = unsafe { &*user.cast::<UnboundedSender<Event>>() };
    if let Some(e) = event(kind, a, b, c, d) {
        let _ = tx.send(e);
    }
}

pub struct LiveParams<'a> {
    pub app_id: &'a str,
    pub room: &'a str,
    pub uid: &'a str,
    pub token: &'a str,
    pub source_kind: SourceKind,
    pub source_id: u64,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub max_kbps: u32,
    pub audio: bool,
}

fn int(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

pub struct Live {
    sender: *mut UnboundedSender<Event>,
    vendor: Vendor,
}

unsafe impl Send for Live {}

impl Live {
    /// # Errors
    ///
    /// 已在推流、参数含空字符或 SDK 接口失败。Agora 的用户 ID 须为数字。
    pub fn start(
        vendor: Vendor,
        p: &LiveParams<'_>,
        events: UnboundedSender<Event>,
    ) -> Result<Self, BridgeError> {
        let (app_id, room, uid, token) =
            (cstr(p.app_id)?, cstr(p.room)?, cstr(p.uid)?, cstr(p.token)?);
        let raw = RawLive {
            app_id: app_id.as_ptr(),
            room: room.as_ptr(),
            uid: uid.as_ptr(),
            token: token.as_ptr(),
            source_kind: p.source_kind.raw(),
            source_id: p.source_id,
            width: int(p.width),
            height: int(p.height),
            frame_rate: int(p.frame_rate),
            max_kbps: int(p.max_kbps),
            audio: i32::from(p.audio),
        };
        let sender = Box::into_raw(Box::new(events));
        let mut sdk = 0i32;
        let rc = unsafe {
            match vendor {
                Vendor::Volc => {
                    volc_ffi::volc_live_start(&raw const raw, on_event, sender.cast(), &raw mut sdk)
                }
                Vendor::Agora => agora_ffi::agora_live_start(
                    &raw const raw,
                    on_event,
                    sender.cast(),
                    &raw mut sdk,
                ),
            }
        };
        if let Err(e) = check(rc, sdk) {
            drop(unsafe { Box::from_raw(sender) });
            return Err(e);
        }
        Ok(Self { sender, vendor })
    }

    #[must_use]
    pub fn vendor(&self) -> Vendor {
        self.vendor
    }

    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn publish(&self) -> Result<(), BridgeError> {
        let mut sdk = 0i32;
        let rc = unsafe {
            match self.vendor {
                Vendor::Volc => volc_ffi::volc_publish(&raw mut sdk),
                Vendor::Agora => agora_ffi::agora_publish(&raw mut sdk),
            }
        };
        check(rc, sdk)
    }

    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn encoder(
        &self,
        width: u32,
        height: u32,
        frame_rate: u32,
        max_kbps: u32,
    ) -> Result<(), BridgeError> {
        let mut sdk = 0i32;
        let f = match self.vendor {
            Vendor::Volc => volc_ffi::volc_encoder,
            Vendor::Agora => agora_ffi::agora_encoder,
        };
        let rc = unsafe {
            f(
                int(width),
                int(height),
                int(frame_rate),
                int(max_kbps),
                &raw mut sdk,
            )
        };
        check(rc, sdk)
    }

    /// # Errors
    ///
    /// 令牌含空字符或 SDK 返回非 0。
    pub fn token(&self, token: &str) -> Result<(), BridgeError> {
        let t = cstr(token)?;
        let mut sdk = 0i32;
        let rc = unsafe {
            match self.vendor {
                Vendor::Volc => volc_ffi::volc_token(t.as_ptr(), &raw mut sdk),
                Vendor::Agora => agora_ffi::agora_token(t.as_ptr(), &raw mut sdk),
            }
        };
        check(rc, sdk)
    }

    /// # Errors
    ///
    /// 未在推流或 SDK 返回非 0。
    pub fn pause(&self, paused: bool) -> Result<(), BridgeError> {
        let mut sdk = 0i32;
        let p = i32::from(paused);
        let rc = unsafe {
            match self.vendor {
                Vendor::Volc => volc_ffi::volc_pause(p, &raw mut sdk),
                Vendor::Agora => agora_ffi::agora_pause(p, &raw mut sdk),
            }
        };
        check(rc, sdk)
    }
}

/// # Errors
///
/// 未在推流或 SDK 返回非 0。
pub fn push_audio(vendor: Vendor, pcm: &[i16]) -> Result<(), BridgeError> {
    let frames = i32::try_from(pcm.len() / 2).unwrap_or(0);
    let mut sdk = 0i32;
    let rc = unsafe {
        match vendor {
            Vendor::Volc => volc_ffi::volc_push_audio(pcm.as_ptr(), frames, &raw mut sdk),
            Vendor::Agora => agora_ffi::agora_push_audio(pcm.as_ptr(), frames, &raw mut sdk),
        }
    };
    check(rc, sdk)
}

impl Drop for Live {
    fn drop(&mut self) {
        unsafe {
            match self.vendor {
                Vendor::Volc => volc_ffi::volc_live_stop(),
                Vendor::Agora => agora_ffi::agora_live_stop(),
            }
        };
        drop(unsafe { Box::from_raw(self.sender) });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    RoomState(i32),
    Published,
    Unpublished(i32),
    TokenWillExpire,
    Stats {
        kbps: i32,
        width: i32,
        height: i32,
        stalls: i32,
    },
    Volume {
        raw: i32,
        volume: i32,
        why: i32,
    },
    Ended,
    Paused(bool),
}

fn watch_event(kind: i32, a: i32, b: i32, c: i32, d: i32) -> Option<WatchEvent> {
    Some(match kind {
        1 => WatchEvent::RoomState(a),
        2 if a != 0 => WatchEvent::Published,
        2 => WatchEvent::Unpublished(b),
        3 => WatchEvent::TokenWillExpire,
        4 => WatchEvent::Stats {
            kbps: a,
            width: b,
            height: c,
            stalls: d,
        },
        5 => WatchEvent::Volume {
            raw: a,
            volume: b,
            why: c,
        },
        6 => WatchEvent::Ended,
        7 => WatchEvent::Paused(a != 0),
        _ => return None,
    })
}

pub struct Frame<'a> {
    pub data: &'a [u8],
    pub codec: i32,
    pub key: bool,
    pub width: u32,
    pub height: u32,
    pub ts_us: i64,
}

pub struct WatchSink {
    pub events: UnboundedSender<WatchEvent>,
    pub frame: Box<dyn Fn(&Frame<'_>) + Send + Sync>,
}

unsafe extern "C" fn on_watch_event(user: *mut c_void, kind: i32, a: i32, b: i32, c: i32, d: i32) {
    let sink = unsafe { &*user.cast::<WatchSink>() };
    if let Some(e) = watch_event(kind, a, b, c, d) {
        let _ = sink.events.send(e);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn on_watch_frame(
    user: *mut c_void,
    data: *const u8,
    size: i32,
    codec: i32,
    key: i32,
    width: i32,
    height: i32,
    ts_us: i64,
) {
    if data.is_null() || size <= 0 {
        return;
    }
    let (sink, data) = unsafe {
        (
            &*user.cast::<WatchSink>(),
            std::slice::from_raw_parts(data, size.cast_unsigned() as usize),
        )
    };
    (sink.frame)(&Frame {
        data,
        codec,
        key: key != 0,
        width: width.max(0).cast_unsigned(),
        height: height.max(0).cast_unsigned(),
        ts_us,
    });
}

pub struct WatchParams<'a> {
    pub app_id: &'a str,
    pub room: &'a str,
    pub uid: &'a str,
    pub token: &'a str,
    pub liver: &'a str,
    pub volume: u16,
}

pub use crate::kook::golive::Vendor;

pub struct Watch {
    sink: *mut WatchSink,
    vendor: Vendor,
    own: bool,
}

unsafe impl Send for Watch {}

impl Watch {
    /// # Errors
    ///
    /// 已在观看、引擎正以其他 appId 使用、参数含空字符或 SDK 接口失败。Agora 的用户 ID 须为数字。
    pub fn start(
        vendor: Vendor,
        p: &WatchParams<'_>,
        sink: WatchSink,
    ) -> Result<Self, BridgeError> {
        let (app_id, room, uid, token, liver) = (
            cstr(p.app_id)?,
            cstr(p.room)?,
            cstr(p.uid)?,
            cstr(p.token)?,
            cstr(p.liver)?,
        );
        let raw = RawWatch {
            app_id: app_id.as_ptr(),
            room: room.as_ptr(),
            uid: uid.as_ptr(),
            token: token.as_ptr(),
            liver: liver.as_ptr(),
            volume: i32::from(p.volume),
        };
        let sink = Box::into_raw(Box::new(sink));
        let mut sdk = 0i32;
        let rc = unsafe {
            match vendor {
                Vendor::Volc => volc_ffi::volc_watch_start(
                    &raw const raw,
                    on_watch_event,
                    on_watch_frame,
                    sink.cast(),
                    &raw mut sdk,
                ),
                Vendor::Agora => agora_ffi::agora_watch_start(
                    &raw const raw,
                    on_watch_event,
                    on_watch_frame,
                    sink.cast(),
                    &raw mut sdk,
                ),
            }
        };
        if let Err(e) = check(rc, sdk) {
            drop(unsafe { Box::from_raw(sink) });
            return Err(e);
        }
        Ok(Self {
            sink,
            vendor,
            own: false,
        })
    }

    /// # Errors
    ///
    /// 当前厂商没有在推流或已在观看自己。
    pub fn own(vendor: Vendor, sink: WatchSink) -> Result<Self, BridgeError> {
        let sink = Box::into_raw(Box::new(sink));
        let mut sdk = 0i32;
        let rc = unsafe {
            match vendor {
                Vendor::Volc => {
                    volc_ffi::volc_own_start(on_watch_event, on_watch_frame, sink.cast())
                }
                Vendor::Agora => agora_ffi::agora_own_start(
                    on_watch_event,
                    on_watch_frame,
                    sink.cast(),
                    &raw mut sdk,
                ),
            }
        };
        if let Err(e) = check(rc, sdk) {
            drop(unsafe { Box::from_raw(sink) });
            return Err(e);
        }
        Ok(Self {
            sink,
            vendor,
            own: true,
        })
    }

    #[must_use]
    pub fn vendor(&self) -> Vendor {
        self.vendor
    }

    #[must_use]
    pub fn is_own(&self) -> bool {
        self.own
    }

    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn subscribe(&self) -> Result<(), BridgeError> {
        if self.vendor == Vendor::Agora || self.own {
            return Ok(());
        }
        let mut sdk = 0i32;
        check(unsafe { volc_ffi::volc_watch_subscribe(&raw mut sdk) }, sdk)
    }

    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn key_frame(&self) -> Result<(), BridgeError> {
        let mut sdk = 0i32;
        let rc = unsafe {
            match (self.own, self.vendor) {
                (true, Vendor::Volc) => volc_ffi::volc_own_key_frame(&raw mut sdk),
                (true, Vendor::Agora) => agora_ffi::agora_own_key_frame(),
                (false, Vendor::Volc) => volc_ffi::volc_watch_key_frame(&raw mut sdk),
                (false, Vendor::Agora) => agora_ffi::agora_watch_rejoin(&raw mut sdk),
            }
        };
        check(rc, sdk)
    }

    /// # Errors
    ///
    /// 令牌含空字符或 SDK 返回非 0。
    pub fn token(&self, token: &str) -> Result<(), BridgeError> {
        if self.own {
            return Ok(());
        }
        let t = cstr(token)?;
        let mut sdk = 0i32;
        let rc = unsafe {
            match self.vendor {
                Vendor::Volc => volc_ffi::volc_watch_token(t.as_ptr(), &raw mut sdk),
                Vendor::Agora => agora_ffi::agora_watch_token(t.as_ptr(), &raw mut sdk),
            }
        };
        check(rc, sdk)
    }

    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn volume(&self, percent: u16) -> Result<(), BridgeError> {
        if self.own {
            return Ok(());
        }
        let mut sdk = 0i32;
        let v = i32::from(percent);
        let rc = unsafe {
            match self.vendor {
                Vendor::Volc => volc_ffi::volc_watch_volume(v, &raw mut sdk),
                Vendor::Agora => agora_ffi::agora_watch_volume(v, &raw mut sdk),
            }
        };
        check(rc, sdk)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            match (self.own, self.vendor) {
                (true, Vendor::Volc) => volc_ffi::volc_own_stop(),
                (true, Vendor::Agora) => agora_ffi::agora_own_stop(),
                (false, Vendor::Volc) => volc_ffi::volc_watch_stop(),
                (false, Vendor::Agora) => agora_ffi::agora_watch_stop(),
            }
        };
        drop(unsafe { Box::from_raw(self.sink) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_return_codes() {
        assert_eq!(check(OK, 0), Ok(()));
        assert_eq!(check(ERR_NOT_STARTED, 0), Err(BridgeError::NotStarted));
        assert_eq!(check(ERR_LOAD, 126), Err(BridgeError::Load(126)));
        assert_eq!(check(ERR_JOIN, -1000), Err(BridgeError::Join(-1000)));
        assert_eq!(check(-99, 0), Err(BridgeError::Unknown(-99)));
    }

    #[test]
    fn maps_events() {
        assert_eq!(event(1, 0, 0, 0, 0), Some(Event::RoomState(0)));
        assert_eq!(
            event(2, 1920, 1080, 0, 0),
            Some(Event::FirstFrame {
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(event(4, 0, 0, 0, 0), Some(Event::TokenWillExpire));
        assert_eq!(event(8, 0, 0, 0, 0), Some(Event::SourceGone));
        assert_eq!(event(9, 1, 0, 0, 0), Some(Event::Paused(true)));
        assert_eq!(event(9, 0, 0, 0, 0), Some(Event::Paused(false)));
        assert_eq!(event(99, 0, 0, 0, 0), None);
    }

    #[test]
    fn maps_watch_events() {
        assert_eq!(watch_event(1, 0, 0, 0, 0), Some(WatchEvent::RoomState(0)));
        assert_eq!(watch_event(2, 1, 0, 0, 0), Some(WatchEvent::Published));
        assert_eq!(watch_event(2, 0, 3, 0, 0), Some(WatchEvent::Unpublished(3)));
        assert_eq!(
            watch_event(3, 0, 0, 0, 0),
            Some(WatchEvent::TokenWillExpire)
        );
        assert_eq!(
            watch_event(4, 2000, 1920, 1080, 1),
            Some(WatchEvent::Stats {
                kbps: 2000,
                width: 1920,
                height: 1080,
                stalls: 1
            })
        );
        assert_eq!(watch_event(6, 0, 0, 0, 0), Some(WatchEvent::Ended));
        assert_eq!(watch_event(7, 1, 0, 0, 0), Some(WatchEvent::Paused(true)));
        assert_eq!(watch_event(7, 0, 0, 0, 0), Some(WatchEvent::Paused(false)));
        assert_eq!(watch_event(9, 0, 0, 0, 0), None);
        assert_eq!(check(ERR_SUBSCRIBE, -2), Err(BridgeError::Subscribe(-2)));
    }

    #[test]
    fn source_id_round_trips_as_string() {
        let s = Source {
            kind: SourceKind::Window,
            id: u64::MAX,
            pid: 1,
            primary: false,
            width: 1,
            height: 1,
            name: String::new(),
            application: String::new(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["id"], "18446744073709551615");
        assert_eq!(v["kind"], "window");
        assert_eq!(parse_id(v["id"].as_str().unwrap()), Ok(u64::MAX));
        assert_eq!(parse_id("x"), Err(BridgeError::Source));
    }

    #[test]
    fn layout_matches_header() {
        assert_eq!(
            std::mem::size_of::<RawSource>(),
            8 + 8 + 6 * 4 + 2 * NAME_LEN
        );
    }
}
