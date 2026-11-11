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

#[repr(C)]
struct RawWatch {
    app_id: *const c_char,
    room: *const c_char,
    uid: *const c_char,
    token: *const c_char,
    liver: *const c_char,
}

type EventFn = unsafe extern "C" fn(*mut c_void, i32, i32, i32, i32, i32);
type FrameFn = unsafe extern "C" fn(*mut c_void, *const u8, i32, i32, i32, i32, i32, i64);

mod ffi {
    use super::{EventFn, FrameFn, RawLive, RawSource, RawWatch, c_char, c_void};
    unsafe extern "C" {
        pub fn volc_start() -> i32;
        pub fn volc_stop();
        pub fn volc_load(dir: *const u16, win32_error: *mut u32) -> i32;
        pub fn volc_version() -> *const c_char;
        pub fn volc_log(dir: *const c_char) -> i32;
        pub fn volc_sources(out: *mut RawSource, cap: i32) -> i32;
        pub fn volc_thumbnail(
            kind: i32,
            id: u64,
            max_w: i32,
            max_h: i32,
            out: *mut u8,
            w: *mut i32,
            h: *mut i32,
        ) -> i32;
        pub fn volc_preview_open() -> i32;
        pub fn volc_preview_close();
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
        pub fn volc_push_audio(pcm: *const i16, frames: i32, sdk_rc: *mut i32) -> i32;
        pub fn volc_watch_start(
            watch: *const RawWatch,
            cb: EventFn,
            frame: FrameFn,
            user: *mut c_void,
            sdk_rc: *mut i32,
        ) -> i32;
        pub fn volc_watch_subscribe(sdk_rc: *mut i32) -> i32;
        pub fn volc_watch_key_frame(sdk_rc: *mut i32) -> i32;
        pub fn volc_watch_token(token: *const c_char, sdk_rc: *mut i32) -> i32;
        pub fn volc_watch_stop();
        pub fn agora_load(dir: *const u16, win32_error: *mut u32) -> i32;
        pub fn agora_version() -> *const c_char;
        pub fn agora_log(file: *const c_char);
        pub fn agora_watch_start(
            watch: *const RawWatch,
            cb: EventFn,
            frame: FrameFn,
            user: *mut c_void,
            sdk_rc: *mut i32,
        ) -> i32;
        pub fn agora_watch_rejoin(sdk_rc: *mut i32) -> i32;
        pub fn agora_watch_token(token: *const c_char, sdk_rc: *mut i32) -> i32;
        pub fn agora_watch_stop();
    }
}

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
    /// 以十进制字符串传给前端：句柄可能超出 JavaScript 安全整数范围。
    #[serde(serialize_with = "id_str")]
    pub id: u64,
    pub pid: u32,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    pub name: String,
    pub application: String,
}

// serde 的 `serialize_with` 规定以引用传入。
#[allow(clippy::trivially_copy_pass_by_ref)]
fn id_str<S: serde::Serializer>(id: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(id)
}

/// 前端传回的源 ID。
///
/// # Errors
///
/// 不是十进制整数时返回 [`BridgeError::Source`]。
pub fn parse_id(s: &str) -> Result<u64, BridgeError> {
    s.parse().map_err(|_| BridgeError::Source)
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

/// 源缩略图，缩放到 `max` 以内并保持宽高比。返回宽、高与紧凑 BGRA 像素。
///
/// # Errors
///
/// 线程未启动、源已不存在或像素格式不支持。
pub fn thumbnail(
    kind: SourceKind,
    id: u64,
    max: (u32, u32),
) -> Result<(u32, u32, Vec<u8>), BridgeError> {
    let (mw, mh) = (int(max.0), int(max.1));
    let mut buf = vec![0u8; max.0 as usize * max.1 as usize * 4];
    let (mut w, mut h) = (0i32, 0i32);
    // SAFETY: buf 有 max_w*max_h*4 字节，桥接写入不超过此范围；w、h 指向本地变量。
    let rc = unsafe {
        ffi::volc_thumbnail(
            kind.raw(),
            id,
            mw,
            mh,
            buf.as_mut_ptr(),
            &raw mut w,
            &raw mut h,
        )
    };
    check(rc, 0)?;
    let (w, h) = (w.cast_unsigned(), h.cast_unsigned());
    buf.truncate(w as usize * h as usize * 4);
    Ok((w, h, buf))
}

/// 保留预览引擎，之后列源与缩略图复用它。开播时自动释放。
///
/// # Errors
///
/// 线程未启动或引擎创建失败。
pub fn preview_open() -> Result<(), BridgeError> {
    // SAFETY: 无参数。
    check(unsafe { ffi::volc_preview_open() }, 0)
}

/// 释放预览引擎。推流中或未打开时无操作。
pub fn preview_close() {
    // SAFETY: 无参数。
    unsafe { ffi::volc_preview_close() }
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
    /// 共享的窗口已销毁。
    SourceGone,
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

/// 推一帧屏幕声音：48 kHz 立体声 S16 交错，应为 10 ms。可在任意线程调用，开播时须带声音。
/// 未在推流或已停止时返回 [`BridgeError::State`]。
///
/// # Errors
///
/// 未在推流或 SDK 返回非 0。
pub fn push_audio(pcm: &[i16]) -> Result<(), BridgeError> {
    let frames = i32::try_from(pcm.len() / 2).unwrap_or(0);
    let mut sdk = 0i32;
    // SAFETY: pcm 在调用期间有效，桥接按 frames 读取且深拷贝。
    check(
        unsafe { ffi::volc_push_audio(pcm.as_ptr(), frames, &raw mut sdk) },
        sdk,
    )
}

impl Drop for Live {
    fn drop(&mut self) {
        // SAFETY: 无参数。返回后桥接不再调用回调。
        unsafe { ffi::volc_live_stop() };
        // SAFETY: sender 由 Box::into_raw 得到，此处唯一一次释放。
        drop(unsafe { Box::from_raw(self.sender) });
    }
}

/// 观看事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    /// 0 为已进房，非 0 为错误或警告码。
    RoomState(i32),
    /// 分享者发布屏幕流。
    Published,
    /// 分享者取消发布，附原因码。
    Unpublished(i32),
    TokenWillExpire,
    Stats {
        kbps: i32,
        width: i32,
        height: i32,
        stalls: i32,
    },
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
        _ => return None,
    })
}

/// 一帧编码后的画面。`data` 只在回调期间有效。
pub struct Frame<'a> {
    pub data: &'a [u8],
    /// 1 为 H.264，2 为 `ByteVC1`。
    pub codec: i32,
    pub key: bool,
    pub width: u32,
    pub height: u32,
    pub ts_us: i64,
}

/// 观看回调的接收方。在 SDK 内部线程调用，不得阻塞或回调 SDK。
pub struct WatchSink {
    pub events: UnboundedSender<WatchEvent>,
    pub frame: Box<dyn Fn(&Frame<'_>) + Send + Sync>,
}

unsafe extern "C" fn on_watch_event(user: *mut c_void, kind: i32, a: i32, b: i32, c: i32, d: i32) {
    // SAFETY: user 为 Watch 持有的 Box<WatchSink>，在 volc_watch_stop 返回前有效。
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
    // SAFETY: user 同上；data 在回调期间有效，长度为 size。
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

/// 观看参数。
pub struct WatchParams<'a> {
    pub app_id: &'a str,
    pub room: &'a str,
    pub uid: &'a str,
    pub token: &'a str,
    /// 分享者用户 ID。
    pub liver: &'a str,
}

/// 分享者分到的 RTC 厂商。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Volc,
    Agora,
}

/// 观看句柄。释放时离房。同一时间只能有一个，与推流互不影响。两家厂商的事件与帧格式相同。
pub struct Watch {
    sink: *mut WatchSink,
    vendor: Vendor,
}

// SAFETY: sink 只在桥接回调中只读访问，释放发生在桥接停止观看之后。
unsafe impl Send for Watch {}

impl Watch {
    /// 进房。火山不订阅，收到 [`WatchEvent::Published`] 后调用 [`Watch::subscribe`]；Agora 自动订阅。
    ///
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
        };
        let sink = Box::into_raw(Box::new(sink));
        let mut sdk = 0i32;
        // SAFETY: raw 中的字符串在调用期间有效；sink 在 Watch 释放前有效。
        let rc = unsafe {
            match vendor {
                Vendor::Volc => ffi::volc_watch_start(
                    &raw const raw,
                    on_watch_event,
                    on_watch_frame,
                    sink.cast(),
                    &raw mut sdk,
                ),
                Vendor::Agora => ffi::agora_watch_start(
                    &raw const raw,
                    on_watch_event,
                    on_watch_frame,
                    sink.cast(),
                    &raw mut sdk,
                ),
            }
        };
        if let Err(e) = check(rc, sdk) {
            // 失败时桥接已清除回调，不再访问 sink。
            // SAFETY: sink 由上面的 Box::into_raw 得到，只释放一次。
            drop(unsafe { Box::from_raw(sink) });
            return Err(e);
        }
        Ok(Self { sink, vendor })
    }

    #[must_use]
    pub fn vendor(&self) -> Vendor {
        self.vendor
    }

    /// 订阅分享者的屏幕画面与声音。Agora 进房即自动订阅，无操作。
    ///
    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn subscribe(&self) -> Result<(), BridgeError> {
        if self.vendor == Vendor::Agora {
            return Ok(());
        }
        let mut sdk = 0i32;
        // SAFETY: sdk 指向本地变量。
        check(unsafe { ffi::volc_watch_subscribe(&raw mut sdk) }, sdk)
    }

    /// 请求关键帧。Agora 没有对应接口，改为离房后重新进房，服务端在新订阅时下发关键帧，画面中断约 4 秒。
    ///
    /// # Errors
    ///
    /// SDK 返回非 0。
    pub fn key_frame(&self) -> Result<(), BridgeError> {
        let mut sdk = 0i32;
        // SAFETY: sdk 指向本地变量。
        let rc = unsafe {
            match self.vendor {
                Vendor::Volc => ffi::volc_watch_key_frame(&raw mut sdk),
                Vendor::Agora => ffi::agora_watch_rejoin(&raw mut sdk),
            }
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
        let rc = unsafe {
            match self.vendor {
                Vendor::Volc => ffi::volc_watch_token(t.as_ptr(), &raw mut sdk),
                Vendor::Agora => ffi::agora_watch_token(t.as_ptr(), &raw mut sdk),
            }
        };
        check(rc, sdk)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: 无参数。返回后桥接不再调用观看回调。
        unsafe {
            match self.vendor {
                Vendor::Volc => ffi::volc_watch_stop(),
                Vendor::Agora => ffi::agora_watch_stop(),
            }
        };
        // SAFETY: sink 由 Box::into_raw 得到，此处唯一一次释放。
        drop(unsafe { Box::from_raw(self.sink) });
    }
}

/// 从目录加载 Agora SDK，附属 DLL 须在同一目录。须在火山 SDK 加载之后调用。
///
/// # Errors
///
/// DLL 加载失败。
pub fn agora_load(dir: &Path) -> Result<(), BridgeError> {
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut win32 = 0u32;
    // SAFETY: wide 以 0 结尾且在调用期间有效，win32 指向本地变量。
    let rc = unsafe { ffi::agora_load(wide.as_ptr(), &raw mut win32) };
    check(rc, win32.cast_signed())
}

/// Agora SDK 版本，须在 [`agora_load`] 成功后调用。
#[must_use]
pub fn agora_version() -> String {
    // SAFETY: SDK 已加载，返回 SDK 持有的以 0 结尾的静态字符串。
    unsafe { CStr::from_ptr(ffi::agora_version()) }
        .to_string_lossy()
        .into_owned()
}

/// 设置 Agora SDK 日志文件，下次开始观看时生效。
///
/// # Errors
///
/// 路径包含空字符。
pub fn agora_log(file: &Path) -> Result<(), BridgeError> {
    let s = cstr(&file.to_string_lossy())?;
    // SAFETY: s 以 0 结尾且在调用期间有效，桥接复制内容。
    unsafe { ffi::agora_log(s.as_ptr()) };
    Ok(())
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
        assert_eq!(watch_event(9, 0, 0, 0, 0), None);
        assert_eq!(check(ERR_SUBSCRIBE, -2), Err(BridgeError::Subscribe(-2)));
    }

    #[test]
    fn source_id_round_trips_as_string() {
        // 句柄可能超出 JavaScript 安全整数，以字符串传递。
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
