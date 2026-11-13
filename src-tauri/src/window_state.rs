use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{LogicalSize, PhysicalPosition, WebviewWindow, WindowEvent};

const FILE: &str = "window.json";
const SAVE_DELAY: Duration = Duration::from_millis(500);
const GRIP_W: i32 = 120;
const GRIP_H: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub v: u8,
    pub width: f64,
    pub height: f64,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            v: 1,
            width: 0.0,
            height: 0.0,
            x: None,
            y: None,
            maximized: false,
        }
    }
}

impl WindowState {
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let s: Self = serde_json::from_slice(bytes).ok()?;
        let ok = |v: f64| v.is_finite() && (1.0..=100_000.0).contains(&v);
        (ok(s.width) && ok(s.height)).then_some(s)
    }
}

#[must_use]
pub fn reachable(x: i32, y: i32, width: i32, monitors: &[(i32, i32, i32, i32)]) -> bool {
    monitors.iter().any(|&(mx, my, mw, mh)| {
        let ox = (x + width).min(mx + mw) - x.max(mx);
        ox >= GRIP_W.min(width) && y >= my && y + GRIP_H <= my + mh
    })
}

fn load() -> Option<WindowState> {
    let path = crate::storage::config_file(FILE)?;
    WindowState::parse(&fs::read(path).ok()?)
}

fn save(state: &WindowState) {
    let Some(path) = crate::storage::config_file(FILE) else {
        return;
    };
    let result = serde_json::to_vec_pretty(state)
        .map_err(std::io::Error::other)
        .and_then(|b| crate::storage::write(&path, &b));
    if let Err(e) = result {
        log::warn!("窗口状态保存失败：{e}");
    }
}

fn capture(window: &WebviewWindow, last: Option<WindowState>) -> Option<WindowState> {
    if window.is_minimized().unwrap_or(false) {
        return None;
    }
    let maximized = window.is_maximized().unwrap_or(false);
    if maximized {
        let base = last.unwrap_or_default();
        return Some(WindowState {
            maximized: true,
            ..base
        });
    }
    let scale = window.scale_factor().ok()?;
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    let pos = window.outer_position().ok()?;
    Some(WindowState {
        v: 1,
        width: size.width,
        height: size.height,
        x: Some(pos.x),
        y: Some(pos.y),
        maximized: false,
    })
}

pub fn init(window: &WebviewWindow) {
    if let Some(s) = load() {
        let _ = window.set_size(LogicalSize::new(s.width, s.height));
        let monitors: Vec<_> = window
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .map(|m| {
                (
                    m.position().x,
                    m.position().y,
                    i32::try_from(m.size().width).unwrap_or(i32::MAX),
                    i32::try_from(m.size().height).unwrap_or(i32::MAX),
                )
            })
            .collect();
        let width = window
            .outer_size()
            .map_or(0, |o| i32::try_from(o.width).unwrap_or(0));
        match (s.x, s.y) {
            (Some(x), Some(y)) if reachable(x, y, width, &monitors) => {
                let _ = window.set_position(PhysicalPosition::new(x, y));
            }
            _ => {
                let _ = window.center();
            }
        }
        if s.maximized {
            let _ = window.maximize();
        }
    }
    let _ = window.show();
    track(window);
}

static TICK: AtomicU64 = AtomicU64::new(0);

fn track(window: &WebviewWindow) {
    let w = window.clone();
    window.on_window_event(move |ev| match ev {
        WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
            let my = TICK.fetch_add(1, Ordering::SeqCst) + 1;
            let w = w.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(SAVE_DELAY).await;
                if TICK.load(Ordering::SeqCst) == my
                    && let Some(s) = capture(&w, load())
                {
                    tauri::async_runtime::spawn_blocking(move || save(&s));
                }
            });
        }
        WindowEvent::CloseRequested { .. } => {
            TICK.fetch_add(1, Ordering::SeqCst);
            if let Some(s) = capture(&w, load()) {
                save(&s);
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_invalid() {
        assert!(WindowState::parse(b"oops").is_none());
        assert!(WindowState::parse(b"{}").is_none());
        assert!(WindowState::parse(br#"{"width":-1,"height":600}"#).is_none());
        let s =
            WindowState::parse(br#"{"width":1200,"height":800,"x":10,"y":20,"maximized":true}"#)
                .unwrap();
        assert_eq!(
            (s.width, s.height, s.x, s.y, s.maximized),
            (1200.0, 800.0, Some(10), Some(20), true)
        );
    }

    #[test]
    fn reachable_requires_title_on_screen() {
        let screens = [(0, 0, 1920, 1080), (1920, 0, 1280, 1024)];
        assert!(reachable(100, 100, 1100, &screens));
        assert!(reachable(2000, 50, 1100, &screens));
        assert!(!reachable(5000, 100, 1100, &screens));
        assert!(!reachable(100, -50, 1100, &screens));
        assert!(!reachable(100, 1070, 1100, &screens));
        assert!(reachable(-900, 100, 1100, &screens));
        assert!(!reachable(-1050, 100, 1100, &screens));
    }
}
