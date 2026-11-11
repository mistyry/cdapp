//! 共享源的实际像素尺寸，直接向系统读取。
//!
//! SDK 的 `onLocalVideoSizeChanged` 报告的是缩放后的编码尺寸，改档后不再等于源尺寸，
//! 不能用来换算编码尺寸。显示器读物理分辨率，窗口读 DWM 可见边框，不含阴影。

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};

use super::bridge::SourceKind;

/// 源的物理像素尺寸。源已不存在时返回 None。
#[must_use]
pub fn size(kind: SourceKind, id: u64) -> Option<(u32, u32)> {
    // 按物理像素读取。cdapp 进程已由 tao 设为 Per-Monitor V2，测试进程没有，在本线程临时设置。
    // SAFETY: 设置并恢复当前线程的 DPI 感知，不影响其他线程。
    let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let rect = match kind {
        SourceKind::Screen => monitor(id),
        SourceKind::Window => window(id),
    };
    if !prev.is_invalid() {
        // SAFETY: 恢复调用前的值。
        unsafe { SetThreadDpiAwarenessContext(prev) };
    }
    let r = rect?;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    (w > 0 && h > 0).then(|| (w.cast_unsigned(), h.cast_unsigned()))
}

fn monitor(id: u64) -> Option<RECT> {
    let mut mi = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: id 为 SDK 列源返回的 HMONITOR；无效时函数返回 FALSE，不访问无效内存。
    let ok = unsafe { GetMonitorInfoW(HMONITOR(id as *mut _), &raw mut mi) };
    ok.as_bool().then_some(mi.rcMonitor)
}

fn window(id: u64) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: id 为 SDK 列源返回的 HWND；窗口已关闭时返回错误。
    unsafe {
        DwmGetWindowAttribute(
            HWND(id as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut r).cast(),
            u32::try_from(std::mem::size_of::<RECT>()).unwrap_or(0),
        )
    }
    .ok()?;
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_source_is_none() {
        assert_eq!(size(SourceKind::Screen, 0), None);
        assert_eq!(size(SourceKind::Window, 0), None);
    }

    /// 本机显示器的物理尺寸，需要 SDK 运行文件列源：`CDAPP_VOLC_BIN`。
    #[test]
    #[ignore = "需要 SDK 运行文件"]
    fn reads_monitor_sizes() {
        let dir = std::env::var_os("CDAPP_VOLC_BIN").expect("CDAPP_VOLC_BIN");
        super::super::bridge::start().unwrap();
        super::super::bridge::load(std::path::Path::new(&dir)).unwrap();
        for s in super::super::bridge::sources()
            .unwrap()
            .into_iter()
            .filter(|s| s.kind == SourceKind::Screen && s.name.contains("DISPLAY"))
        {
            let got = size(s.kind, s.id);
            println!("{} SDK {}x{} 系统 {got:?}", s.name, s.width, s.height);
            assert_eq!(got, Some((s.width, s.height)), "{}", s.name);
        }
        super::super::bridge::stop();
    }
}
