use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation,
    NSGraphicsContext, NSImage, NSWindingRule, NSWindow, NSWindowButton,
};
use objc2_foundation::{NSDictionary, NSError, NSPoint, NSRect, NSString};
use objc2_web_kit::WKWebView;
use tauri::{AppHandle, Manager};

pub fn init(app: &AppHandle) {
    let Some(dir) = std::env::var_os("CDAPP_CAPTURE_DIR").map(PathBuf::from) else {
        return;
    };
    log::info!("截图目录：{}", dir.display());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            for (name, script) in take_requests(&dir) {
                let Some(window) = app.get_webview_window("main") else {
                    continue;
                };
                if !script.trim().is_empty() {
                    if let Err(e) = window.eval(script) {
                        log::warn!("截图脚本执行失败：{e}");
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                let out = dir.join(format!("{name}.png"));
                if let Err(e) = window.with_webview(move |w| capture(w.inner(), w.ns_window(), out))
                {
                    log::warn!("截图失败：{e}");
                }
            }
        }
    });
}

fn take_requests(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_none_or(|x| x != "req") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        let script = std::fs::read_to_string(&path).unwrap_or_default();
        if std::fs::remove_file(&path).is_ok() {
            out.push((name, script));
        }
    }
    out.sort();
    out
}

fn capture(webview: *mut c_void, window: *mut c_void, out: PathBuf) {
    let webview = unsafe { Retained::retain(webview.cast::<WKWebView>()) };
    let window = unsafe { Retained::retain(window.cast::<NSWindow>()) };
    let (Some(webview), Some(window)) = (webview, window) else {
        return;
    };
    let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let result = match unsafe { image.as_ref() } {
            Some(image) => compose(image, &window)
                .and_then(|png| std::fs::write(&out, png).map_err(|e| e.to_string())),
            None => Err(unsafe { error.as_ref() }.map_or_else(
                || "没有图像".to_owned(),
                |e| e.localizedDescription().to_string(),
            )),
        };
        match result {
            Ok(()) => log::info!("截图已保存：{}", out.display()),
            Err(e) => log::warn!("截图失败：{e}"),
        }
    });
    unsafe { webview.takeSnapshotWithConfiguration_completionHandler(None, &handler) };
}

fn corner_radius(window: &NSWindow) -> f64 {
    if !window.respondsToSelector(sel!(_cornerRadius)) {
        return 0.0;
    }
    unsafe { msg_send![window, _cornerRadius] }
}

fn compose(shot: &NSImage, window: &NSWindow) -> Result<Vec<u8>, String> {
    let tiff = shot.TIFFRepresentation().ok_or("无法读取图像数据")?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("无法解码图像数据")?;
    rep.setSize(shot.size());
    let ctx =
        NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep).ok_or("无法创建绘图上下文")?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        let Some(button) = window.standardWindowButton(kind) else {
            continue;
        };
        let bounds = button.bounds();
        let Some(cache) = button.bitmapImageRepForCachingDisplayInRect(bounds) else {
            continue;
        };
        button.cacheDisplayInRect_toBitmapImageRep(bounds, &cache);
        let rect = button.convertRect_toView(bounds, None);
        ctx.saveGraphicsState();
        NSBezierPath::bezierPathWithOvalInRect(rect).addClip();
        cache.drawInRect(rect);
        ctx.restoreGraphicsState();
    }
    let radius = corner_radius(window);
    if radius > 0.0 {
        let size = shot.size();
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let mask = NSBezierPath::bezierPathWithRect(rect);
        mask.appendBezierPath(&NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect, radius, radius,
        ));
        mask.setWindingRule(NSWindingRule::EvenOdd);
        ctx.setCompositingOperation(NSCompositingOperation::Clear);
        mask.fill();
    }
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let props = NSDictionary::<NSString, AnyObject>::new();
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or("无法编码 PNG")?;
    Ok(png.to_vec())
}
