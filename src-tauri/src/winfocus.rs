//! Windows 拖动窗口时的焦点闪烁修正。
//!
//! wry 0.57.1 在父窗口收到 `WM_ENTERSIZEMOVE` 时同步调用 `WebView2` 的 `MoveFocus`，
//! 每次点击拖拽区或标题栏都会先失去再取回焦点，并推迟移动循环，表现为闪烁和卡顿（tauri#10767）。
//! 上游已在 wry#1910 撤销该调用，发布前在此拦截：进入移动循环时不转交该消息，结束时补发一次。

use tauri::WebviewWindow;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE};

const SUBCLASS_ID: usize = 0x6364_6170;

pub fn init(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    // SAFETY: hwnd 为主窗口句柄，子类过程在窗口销毁时由系统一并移除。
    let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0) };
    if !ok.as_bool() {
        log::warn!("无法安装窗口子类");
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: 由系统在窗口线程调用，参数原样转交下一层子类。
    unsafe {
        match msg {
            WM_ENTERSIZEMOVE => LRESULT(0),
            WM_EXITSIZEMOVE => {
                DefSubclassProc(hwnd, WM_ENTERSIZEMOVE, WPARAM(0), LPARAM(0));
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}
