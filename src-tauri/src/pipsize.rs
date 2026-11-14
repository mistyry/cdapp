use std::sync::atomic::{AtomicU64, Ordering};

use tauri::WebviewWindow;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, WM_NCDESTROY, WM_SIZING, WMSZ_BOTTOM, WMSZ_BOTTOMLEFT, WMSZ_LEFT,
    WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT,
};

const SUBCLASS_ID: usize = 0x6364_7069;

pub const MIN_W: f64 = 240.0;
pub const MIN_H: f64 = 135.0;

static RATIO: AtomicU64 = AtomicU64::new(0);

fn pack(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

fn ratio() -> Option<(i64, i64)> {
    let v = RATIO.load(Ordering::Relaxed);
    let (w, h) = (
        i64::try_from(v >> 32).ok()?,
        i64::try_from(v & 0xffff_ffff).ok()?,
    );
    (w > 0 && h > 0).then_some((w, h))
}

pub fn set_ratio(width: u32, height: u32) {
    RATIO.store(pack(width, height), Ordering::Relaxed);
}

pub fn init(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0) };
    if !ok.as_bool() {
        log::warn!("无法安装悬浮窗子类");
    }
}

#[must_use]
pub fn height_for(width: i64, (rw, rh): (i64, i64)) -> i64 {
    (width * rh + rw / 2) / rw
}

#[must_use]
pub fn width_for(height: i64, (rw, rh): (i64, i64)) -> i64 {
    (height * rw + rh / 2) / rh
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub dw: i32,
    pub dh: i32,
    pub min_w: i64,
    pub min_h: i64,
}

#[must_use]
pub fn fit(
    edge: u32,
    r: (i32, i32, i32, i32),
    ratio: (i64, i64),
    frame: Frame,
) -> (i32, i32, i32, i32) {
    let (left, top, right, bottom) = r;
    let by_height = edge == WMSZ_TOP || edge == WMSZ_BOTTOM;
    if !by_height && !(1..=8).contains(&edge) {
        return r;
    }
    let cw = i64::from(right - left - frame.dw);
    let ch = i64::from(bottom - top - frame.dh);
    let (mut cw, mut ch) = if by_height {
        let ch = ch.max(frame.min_h);
        (width_for(ch, ratio), ch)
    } else {
        let cw = cw.max(frame.min_w);
        (cw, height_for(cw, ratio))
    };
    if cw < frame.min_w {
        cw = frame.min_w;
        ch = height_for(cw, ratio);
    }
    if ch < frame.min_h {
        ch = frame.min_h;
        cw = width_for(ch, ratio);
    }
    let clamp = |v: i64| i32::try_from(v).unwrap_or(i32::MAX / 2);
    let ow = clamp(cw) + frame.dw;
    let oh = clamp(ch) + frame.dh;
    let anchor_right = edge == WMSZ_LEFT || edge == WMSZ_TOPLEFT || edge == WMSZ_BOTTOMLEFT;
    let anchor_bottom = edge == WMSZ_TOP || edge == WMSZ_TOPLEFT || edge == WMSZ_TOPRIGHT;
    let (x0, x1) = if anchor_right {
        (right - ow, right)
    } else {
        (left, left + ow)
    };
    let (y0, y1) = if anchor_bottom {
        (bottom - oh, bottom)
    } else {
        (top, top + oh)
    };
    (x0, y0, x1, y1)
}

unsafe fn frame_of(hwnd: HWND) -> Frame {
    let mut win = RECT::default();
    let mut client = RECT::default();
    let (dw, dh) = unsafe {
        if GetWindowRect(hwnd, &raw mut win).is_ok() && GetClientRect(hwnd, &raw mut client).is_ok()
        {
            (
                ((win.right - win.left) - (client.right - client.left)).max(0),
                ((win.bottom - win.top) - (client.bottom - client.top)).max(0),
            )
        } else {
            (0, 0)
        }
    };
    let dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96.0,
        d => f64::from(d),
    };
    #[allow(clippy::cast_possible_truncation)]
    let scaled = |v: f64| (v * dpi / 96.0).round() as i64;
    Frame {
        dw,
        dh,
        min_w: scaled(MIN_W),
        min_h: scaled(MIN_H),
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
    unsafe {
        if msg == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
        } else if msg == WM_SIZING
            && lparam.0 != 0
            && let Some(ratio) = ratio()
        {
            let frame = frame_of(hwnd);
            let rect = &mut *(lparam.0 as *mut RECT);
            let edge = u32::try_from(wparam.0).unwrap_or(0);
            let (l, t, r, b) = fit(
                edge,
                (rect.left, rect.top, rect.right, rect.bottom),
                ratio,
                frame,
            );
            *rect = RECT {
                left: l,
                top: t,
                right: r,
                bottom: b,
            };
            return LRESULT(1);
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{WMSZ_BOTTOMRIGHT, WMSZ_RIGHT};

    const BARE: Frame = Frame {
        dw: 0,
        dh: 0,
        min_w: 0,
        min_h: 0,
    };

    #[test]
    fn side_edges_follow_width() {
        let r = fit(WMSZ_RIGHT, (0, 0, 640, 100), (16, 9), BARE);
        assert_eq!(r, (0, 0, 640, 360));
        let r = fit(WMSZ_BOTTOM, (10, 10, 50, 190), (16, 9), BARE);
        assert_eq!(r, (10, 10, 330, 190));
    }

    #[test]
    fn top_corners_keep_bottom() {
        let r = fit(WMSZ_TOPLEFT, (0, 0, 320, 500), (4, 3), BARE);
        assert_eq!(r, (0, 260, 320, 500));
    }

    fn client(r: (i32, i32, i32, i32), f: Frame) -> (i32, i32) {
        (r.2 - r.0 - f.dw, r.3 - r.1 - f.dh)
    }

    #[test]
    fn framed_ratio_applies_to_client() {
        let frame = Frame {
            dw: 2,
            dh: 2,
            min_w: 0,
            min_h: 0,
        };
        let got = fit(WMSZ_BOTTOMRIGHT, (0, 0, 642, 100), (16, 9), frame);
        assert_eq!(got, (0, 0, 642, 362));
        assert_eq!(client(got, frame), (640, 360));
        let got = fit(WMSZ_TOP, (0, 0, 10, 362), (16, 9), frame);
        assert_eq!(client(got, frame), (640, 360));
        assert_eq!(got.3, 362);
    }

    #[test]
    fn framed_client_respects_minimum() {
        let frame = Frame {
            dw: 2,
            dh: 2,
            min_w: 240,
            min_h: 135,
        };
        let got = fit(WMSZ_LEFT, (500, 0, 600, 50), (16, 9), frame);
        assert_eq!(client(got, frame), (240, 135));
        assert_eq!(got.2, 600);
        let got = fit(WMSZ_RIGHT, (0, 0, 300, 50), (32, 9), frame);
        assert_eq!(client(got, frame), (480, 135));
        let got = fit(WMSZ_TOPLEFT, (0, 0, 100, 100), (9, 16), frame);
        assert_eq!(client(got, frame), (240, 427));
        assert_eq!((got.2, got.3), (100, 100));
    }

    #[test]
    fn ratio_round_trips_and_rejects_zero() {
        set_ratio(1920, 1080);
        assert_eq!(ratio(), Some((1920, 1080)));
        set_ratio(0, 1080);
        assert_eq!(ratio(), None);
    }
}
