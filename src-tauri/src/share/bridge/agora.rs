use std::ffi::CStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use super::{BridgeError, check, cstr};

pub(super) mod ffi {
    use super::super::{EventFn, FrameFn, RawLive, RawWatch};
    use std::ffi::{c_char, c_void};
    unsafe extern "C" {
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
        pub fn agora_watch_volume(volume: i32, sdk_rc: *mut i32) -> i32;
        pub fn agora_watch_stop();
        pub fn agora_live_start(
            live: *const RawLive,
            cb: EventFn,
            user: *mut c_void,
            sdk_rc: *mut i32,
        ) -> i32;
        pub fn agora_publish(sdk_rc: *mut i32) -> i32;
        pub fn agora_encoder(
            width: i32,
            height: i32,
            frame_rate: i32,
            max_kbps: i32,
            sdk_rc: *mut i32,
        ) -> i32;
        pub fn agora_token(token: *const c_char, sdk_rc: *mut i32) -> i32;
        pub fn agora_pause(paused: i32, sdk_rc: *mut i32) -> i32;
        pub fn agora_live_stop();
        pub fn agora_push_audio(pcm: *const i16, frames: i32, sdk_rc: *mut i32) -> i32;
        pub fn agora_own_start(
            cb: EventFn,
            frame: FrameFn,
            user: *mut c_void,
            sdk_rc: *mut i32,
        ) -> i32;
        pub fn agora_own_key_frame() -> i32;
        pub fn agora_own_stop();
    }
}

/// # Errors
///
/// DLL 加载失败。
pub fn load(dir: &Path) -> Result<(), BridgeError> {
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut win32 = 0u32;
    let rc = unsafe { ffi::agora_load(wide.as_ptr(), &raw mut win32) };
    check(rc, win32.cast_signed())
}

#[must_use]
pub fn version() -> String {
    unsafe { CStr::from_ptr(ffi::agora_version()) }
        .to_string_lossy()
        .into_owned()
}

/// # Errors
///
/// 路径包含空字符。
pub fn log(file: &Path) -> Result<(), BridgeError> {
    let s = cstr(&file.to_string_lossy())?;
    unsafe { ffi::agora_log(s.as_ptr()) };
    Ok(())
}
