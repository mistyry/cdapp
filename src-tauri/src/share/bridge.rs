//! 火山 RTC SDK 桥接 `native/volc_bridge.cpp` 的 FFI 声明与安全封装。
//! 每个调用都会阻塞到 SDK 线程执行完毕，异步代码中须放在 `spawn_blocking` 里调用。

use std::ffi::{CStr, CString, c_char, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use tokio::sync::mpsc::UnboundedSender;

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

type EventFn = unsafe extern "C" fn(*mut c_void, i32, i32, i32, i32, i32);

mod ffi {
    use super::{EventFn, RawLive, RawSource, c_char, c_void};
    unsafe extern "C" {
        pub fn volc_start() -> i32;
        pub fn volc_stop();
        pub fn volc_load(dir: *const u16, win32_error: *mut u32) -> i32;
        pub fn volc_version() -> *const c_char;
        pub fn volc_log(dir: *const c_char) -> i32;
        pub fn volc_sources(out: *mut RawSource, cap: i32) -> i32;
        pub fn volc_live_start(
            live: *const RawLive,
            cb: EventFn,
            user: *mut c_void,
            sdk_rc: *mut i32,
        ) -> i32;
        pub fn volc_publish(sdk_rc: *mut i32) -> i32;
        pub fn volc_encoder(
            width: i32,
            height: i32,
            frame_rate: i32,
            max_kbps: i32,
            sdk_rc: *mut i32,
        ) -> i32;
        pub fn volc_token(token: *const c_char, sdk_rc: *mut i32) -> i32;
        pub fn volc_live_stop();
    }
}

const OK: i32 = 0;
const ERR_NOT_STARTED: i32 = -1;
const ERR_THREAD: i32 = -2;
const ERR_LOAD: i32 = -3;
const ERR_ENGINE: i32 = -4;
const ERR_STATE: i32 = -6;
const ERR_JOIN: i32 = -10;
const ERR_ENCODER: i32 = -11;
const ERR_CAPTURE: i32 = -12;
const ERR_PUBLISH: i32 = -13;
const ERR_TOKEN: i32 = -14;

/// 桥接错误。SDK 接口失败时附带其原始返回值。
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
        ERR_STATE => Err(BridgeError::State),
        ERR_JOIN => Err(BridgeError::Join(extra)),
        ERR_ENCODER => Err(BridgeError::Encoder(extra)),
        ERR_CAPTURE => Err(BridgeError::Capture(extra)),
        ERR_PUBLISH => Err(BridgeError::Publish(extra)),
        ERR_TOKEN => Err(BridgeError::Token(extra)),
        other => Err(BridgeError::Unknown(other)),
    }
}

fn cstr(s: &str) -> Result<CString, BridgeError> {
    CString::new(s).map_err(|_| BridgeError::Nul)
}

/// 启动 SDK 线程，重复调用无副作用。
///
/// # Errors
///
/// 线程创建失败。
pub fn start() -> Result<(), BridgeError> {
    // SAFETY: 无参数，桥接内部加锁。
    check(unsafe { ffi::volc_start() }, 0)
}

/// 停止推流与 SDK 线程并等待退出。
pub fn stop() {
    // SAFETY: 无参数，桥接内部加锁。
    unsafe { ffi::volc_stop() }
}

/// 从目录加载 SDK，附属 DLL 须在同一目录。
///
/// # Errors
///
/// 线程未启动，或 DLL 加载失败。
pub fn load(dir: &Path) -> Result<(), BridgeError> {
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut win32 = 0u32;
    // SAFETY: wide 以 0 结尾且在调用期间有效，win32 指向本地变量。
    let rc = unsafe { ffi::volc_load(wide.as_ptr(), &raw mut win32) };
    check(rc, win32.cast_signed())
}

/// SDK 版本，须在 [`load`] 成功后调用。
#[must_use]
pub fn version() -> String {
    // SAFETY: SDK 已加载，返回 SDK 持有的以 0 结尾的静态字符串。
    unsafe { CStr::from_ptr(ffi::volc_version()) }
        .to_string_lossy()
        .into_owned()
}

/// 设置 SDK 日志目录，须在首次创建引擎前调用。
///
/// # Errors
///
/// 线程未启动或路径包含空字符。
pub fn set_log_dir(dir: &Path) -> Result<(), BridgeError> {
    let s = cstr(&dir.to_string_lossy())?;
    // SAFETY: s 以 0 结尾且在调用期间有效。
    check(unsafe { ffi::volc_log(s.as_ptr()) }, 0)
}

/// 共享源类型，与 SDK 的 `ScreenCaptureSourceType` 一致。
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

/// 可共享的显示器或窗口。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub kind: SourceKind,
    /// 窗口为 HWND，显示器为 HMONITOR。只在本次运行中有效。
    pub id: u64,
    pub pid: u32,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    pub name: String,
    pub application: String,
}

fn text(buf: &[c_char; NAME_LEN]) -> String {
    // SAFETY: 桥接写入时以 0 结尾。
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// 列出可共享的显示器与窗口。
///
/// # Errors
///
/// 线程未启动或引擎创建失败。
pub fn sources() -> Result<Vec<Source>, BridgeError> {
    let mut cap = 64usize;
    loop {
        let mut buf: Vec<RawSource> = Vec::with_capacity(cap);
        // SAFETY: buf 有 cap 个元素的空间，桥接最多写入 cap 项。
        let total =
            unsafe { ffi::volc_sources(buf.as_mut_ptr(), i32::try_from(cap).unwrap_or(i32::MAX)) };
        if total < 0 {
            return Err(check(total, 0).unwrap_err());
        }
        let total = total.cast_unsigned() as usize;
        if total > cap {
            cap = total;
            continue;
        }
        // SAFETY: 桥接已写入前 total 项。
        unsafe { buf.set_len(total) };
        return Ok(buf
            .iter()
            .filter_map(|r| {
                let kind = match r.kind {
                    1 => SourceKind::Window,
                    2 => SourceKind::Screen,
                    _ => return None,
                };
                Some(Source {
                    kind,
                    id: r.id,
                    pid: r.pid.cast_unsigned(),
                    primary: r.primary != 0,
                    width: r.width.max(0).cast_unsigned(),
                    height: r.height.max(0).cast_unsigned(),
                    name: text(&r.name),
                    application: text(&r.application),
                })
            })
            .collect());
    }
}

/// SDK 事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// 0 为已进房，非 0 为错误或警告码。
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
    /// 本地视频流状态与错误码。状态 3 为采集失败，例如窗口关闭。
    VideoState {
        state: i32,
        error: i32,
    },
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
        _ => return None,
    })
}

unsafe extern "C" fn on_event(user: *mut c_void, kind: i32, a: i32, b: i32, c: i32, d: i32) {
    // SAFETY: user 为 Live 持有的 Box<UnboundedSender>，在 volc_live_stop 返回前有效。
    let tx = unsafe { &*user.cast::<UnboundedSender<Event>>() };
    if let Some(e) = event(kind, a, b, c, d) {
        let _ = tx.send(e);
    }
}

/// 开播参数。
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

/// 推流句柄。释放时停止推流、离房并销毁引擎。同一时间只能有一个。
pub struct Live {
    sender: *mut UnboundedSender<Event>,
}

// SAFETY: sender 只在桥接回调中只读访问，释放发生在 volc_live_stop 之后。
unsafe impl Send for Live {}

impl Live {
    /// 创建引擎、进房并开始采集。事件发往 `events`。成功后等进房与首帧事件再调用 [`Live::publish`]。
    ///
    /// # Errors
    ///
    /// 已在推流、参数含空字符或 SDK 接口失败。
    pub fn start(p: &LiveParams<'_>, events: UnboundedSender<Event>) -> Result<Self, BridgeError> {
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
        // SAFETY: raw 中的字符串在调用期间有效；sender 在 Live 释放前有效。
        let rc =
            unsafe { ffi::volc_live_start(&raw const raw, on_event, sender.cast(), &raw mut sdk) };
        if let Err(e) = check(rc, sdk) {
            // 失败时桥接已清除回调，不再访问 sender。
            // SAFETY: sender 由上面的 Box::into_raw 得到，只释放一次。
            drop(unsafe { Box::from_raw(sender) });
            return Err(e);
        }
        Ok(Self { sender })
    }

    /// 发布屏幕流。
    ///
    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn publish(&self) -> Result<(), BridgeError> {
        let mut sdk = 0i32;
        // SAFETY: sdk 指向本地变量。
        check(unsafe { ffi::volc_publish(&raw mut sdk) }, sdk)
    }

    /// 重新提交编码配置。
    ///
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
        // SAFETY: sdk 指向本地变量。
        let rc = unsafe {
            ffi::volc_encoder(
                int(width),
                int(height),
                int(frame_rate),
                int(max_kbps),
                &raw mut sdk,
            )
        };
        check(rc, sdk)
    }

    /// 更换令牌。
    ///
    /// # Errors
    ///
    /// 令牌含空字符或 SDK 返回非 0。
    pub fn token(&self, token: &str) -> Result<(), BridgeError> {
        let t = cstr(token)?;
        let mut sdk = 0i32;
        // SAFETY: t 在调用期间有效，sdk 指向本地变量。
        check(unsafe { ffi::volc_token(t.as_ptr(), &raw mut sdk) }, sdk)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // SAFETY: 无参数。返回后桥接不再调用回调。
        unsafe { ffi::volc_live_stop() };
        // SAFETY: sender 由 Box::into_raw 得到，此处唯一一次释放。
        drop(unsafe { Box::from_raw(self.sender) });
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
        assert_eq!(event(99, 0, 0, 0, 0), None);
    }

    #[test]
    fn layout_matches_header() {
        // volc_source：kind 后按 8 字节对齐 id，再 6 个 int32，两段名称。
        assert_eq!(
            std::mem::size_of::<RawSource>(),
            8 + 8 + 6 * 4 + 2 * NAME_LEN
        );
    }

    /// 需要本机 SDK 运行文件，用环境变量 `CDAPP_VOLC_BIN` 指定目录。
    #[test]
    #[ignore = "需要 SDK 运行文件"]
    fn loads_real_sdk_and_lists_sources() {
        let dir = std::env::var_os("CDAPP_VOLC_BIN").expect("CDAPP_VOLC_BIN");
        start().unwrap();
        load(Path::new(&dir)).unwrap();
        assert!(version().starts_with("3.58"));
        let list = sources().unwrap();
        assert!(
            list.iter()
                .any(|s| s.kind == SourceKind::Screen && s.primary)
        );
        for s in list.iter().filter(|s| s.kind == SourceKind::Screen) {
            println!("{} {}x{} primary={}", s.name, s.width, s.height, s.primary);
        }
        stop();
    }
}
