use std::ffi::CStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use super::{BridgeError, Source, SourceKind, check, cstr, int, text};

pub(super) mod ffi {
    use super::super::{EventFn, FrameFn, RawLive, RawSource, RawWatch};
    use std::ffi::{c_char, c_void};
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
        pub fn volc_pause(paused: i32, sdk_rc: *mut i32) -> i32;
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
        pub fn volc_watch_volume(volume: i32, sdk_rc: *mut i32) -> i32;
        pub fn volc_watch_stop();
        pub fn volc_own_start(cb: EventFn, frame: FrameFn, user: *mut c_void) -> i32;
        pub fn volc_own_key_frame(sdk_rc: *mut i32) -> i32;
        pub fn volc_own_stop();
    }
}

/// # Errors
///
/// 线程创建失败。
pub fn start() -> Result<(), BridgeError> {
    check(unsafe { ffi::volc_start() }, 0)
}

pub fn stop() {
    unsafe { ffi::volc_stop() }
}

/// # Errors
///
/// 线程未启动，或 DLL 加载失败。
pub fn load(dir: &Path) -> Result<(), BridgeError> {
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut win32 = 0u32;
    let rc = unsafe { ffi::volc_load(wide.as_ptr(), &raw mut win32) };
    check(rc, win32.cast_signed())
}

#[must_use]
pub fn version() -> String {
    unsafe { CStr::from_ptr(ffi::volc_version()) }
        .to_string_lossy()
        .into_owned()
}

/// # Errors
///
/// 线程未启动或路径包含空字符。
pub fn log(dir: &Path) -> Result<(), BridgeError> {
    let s = cstr(&dir.to_string_lossy())?;
    check(unsafe { ffi::volc_log(s.as_ptr()) }, 0)
}

/// # Errors
///
/// 线程未启动或引擎创建失败。
pub fn sources() -> Result<Vec<Source>, BridgeError> {
    let mut cap = 64usize;
    loop {
        let mut buf: Vec<super::RawSource> = Vec::with_capacity(cap);
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

/// # Errors
///
/// 线程未启动或引擎创建失败。
pub fn preview_open() -> Result<(), BridgeError> {
    check(unsafe { ffi::volc_preview_open() }, 0)
}

pub fn preview_close() {
    unsafe { ffi::volc_preview_close() }
}

#[cfg(test)]
mod tests {
    use super::*;

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
