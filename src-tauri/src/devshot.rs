//! 开发构建的窗口截图，仅在 macOS 调试构建中编译。
//!
//! 设置环境变量 `CDAPP_CAPTURE_DIR` 后启用。在该目录写入 `名称.req`，应用截取主窗口并保存为 `名称.png`。
//! 请求文件内容非空时，先在页面中执行其中的脚本，0.5 秒后截图。
//! 截图以 `WKWebView` 快照为底，叠加系统绘制的红绿灯，并按窗口圆角裁去四角。

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

/// 取出并删除目录中的截图请求，按名称排序。
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
    // SAFETY: with_webview 在主线程回调，两个指针在回调期间有效，retain 后自行持有引用。
    let webview = unsafe { Retained::retain(webview.cast::<WKWebView>()) };
    let window = unsafe { Retained::retain(window.cast::<NSWindow>()) };
    let (Some(webview), Some(window)) = (webview, window) else {
        return;
    };
    let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        // SAFETY: 完成回调在主线程执行，参数在回调期间有效。
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
    // SAFETY: 在主线程调用，配置为空时截取整个可见区域。
    unsafe { webview.takeSnapshotWithConfiguration_completionHandler(None, &handler) };
}

/// 系统窗口圆角。读取私有方法 `_cornerRadius`，不存在时返回 0。
fn corner_radius(window: &NSWindow) -> f64 {
    if !window.respondsToSelector(sel!(_cornerRadius)) {
        return 0.0;
    }
    // SAFETY: 已确认对象响应该选择子，返回值为 CGFloat。
    unsafe { msg_send![window, _cornerRadius] }
}

/// 在网页快照上叠加红绿灯，编码为 PNG。网页铺满整个窗口，窗口坐标即图像坐标。
fn compose(shot: &NSImage, window: &NSWindow) -> Result<Vec<u8>, String> {
    let tiff = shot.TIFFRepresentation().ok_or("无法读取图像数据")?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("无法解码图像数据")?;
    // 按点绘制，位图保持原有像素密度。
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
        // 按钮缓存为方形，按圆形裁剪后再绘制。
        let rect = button.convertRect_toView(bounds, None);
        ctx.saveGraphicsState();
        NSBezierPath::bezierPathWithOvalInRect(rect).addClip();
        cache.drawInRect(rect);
        ctx.restoreGraphicsState();
    }
    // 窗口圆角由系统在合成时裁切，快照中没有，按系统值清除四角。
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
    // SAFETY: 属性字典为空。
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or("无法编码 PNG")?;
    Ok(png.to_vec())
}
