//! 火山 RTC SDK 桥接 `native/volc_bridge.cpp` 的 FFI 声明与安全封装。

use std::ffi::CStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

mod ffi {
    unsafe extern "C" {
        pub fn volc_start() -> i32;
        pub fn volc_stop();
        pub fn volc_load(dir: *const u16, win32_error: *mut u32) -> i32;
        pub fn volc_version() -> *const std::ffi::c_char;
    }
}

const OK: i32 = 0;
const ERR_NOT_STARTED: i32 = -1;
const ERR_THREAD: i32 = -2;
const ERR_LOAD: i32 = -3;

/// 桥接错误。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BridgeError {
    #[error("屏幕分享线程未启动")]
    NotStarted,
    #[error("屏幕分享线程启动失败")]
    Thread,
    #[error("屏幕分享组件加载失败：系统错误 {0}")]
    Load(u32),
    #[error("屏幕分享组件返回未知错误 {0}")]
    Unknown(i32),
}

fn check(rc: i32, win32: u32) -> Result<(), BridgeError> {
    match rc {
        OK => Ok(()),
        ERR_NOT_STARTED => Err(BridgeError::NotStarted),
        ERR_THREAD => Err(BridgeError::Thread),
        ERR_LOAD => Err(BridgeError::Load(win32)),
        other => Err(BridgeError::Unknown(other)),
    }
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

/// 停止 SDK 线程并等待退出。
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
    check(rc, win32)
}

/// SDK 版本，须在 [`load`] 成功后调用。
#[must_use]
pub fn version() -> String {
    // SAFETY: SDK 已加载，返回 SDK 持有的以 0 结尾的静态字符串。
    unsafe { CStr::from_ptr(ffi::volc_version()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_return_codes() {
        assert_eq!(check(OK, 0), Ok(()));
        assert_eq!(check(ERR_NOT_STARTED, 0), Err(BridgeError::NotStarted));
        assert_eq!(check(ERR_LOAD, 126), Err(BridgeError::Load(126)));
        assert_eq!(check(-99, 0), Err(BridgeError::Unknown(-99)));
    }

    /// 需要本机 SDK 运行文件，用环境变量 `CDAPP_VOLC_BIN` 指定目录。
    #[test]
    #[ignore = "需要 SDK 运行文件"]
    fn loads_real_sdk() {
        let dir = std::env::var_os("CDAPP_VOLC_BIN").expect("CDAPP_VOLC_BIN");
        start().unwrap();
        load(Path::new(&dir)).unwrap();
        assert!(version().starts_with("3.58"));
        stop();
    }
}
