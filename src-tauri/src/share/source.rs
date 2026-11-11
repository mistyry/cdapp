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

use super::bridge::{self, SourceKind};

/// 缩略图上限，与选源对话框的 16:9 卡片对应，2 倍像素。
pub const THUMB_MAX: (u32, u32) = (480, 270);

/// 源缩略图，PNG 数据地址。源已不存在或格式不支持时返回 None。
#[must_use]
pub fn thumbnail(kind: SourceKind, id: u64) -> Option<String> {
    use base64::Engine;
    let (w, h, bgra) = bridge::thumbnail(kind, id, THUMB_MAX).ok()?;
    let png = encode_png(w, h, &bgra)?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

/// BGRA 编码为不透明 RGB 的 PNG。
fn encode_png(w: u32, h: u32, bgra: &[u8]) -> Option<Vec<u8>> {
    let rgb: Vec<u8> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&rgb).ok()?;
    writer.finish().ok()?;
    Some(out)
}

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

    #[test]
    fn encodes_bgra_as_png() {
        // 2×1：蓝、红。
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let dec = png::Decoder::new(std::io::Cursor::new(png));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        r.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..6], [0, 0, 255, 255, 0, 0]);
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
            let (w, h, px) = bridge::thumbnail(s.kind, s.id, THUMB_MAX).unwrap();
            println!("{} 缩略图 {w}x{h}", s.name);
            assert!(w <= THUMB_MAX.0 && h <= THUMB_MAX.1 && w > 0 && h > 0);
            // 屏幕内容不应全黑。
            assert!(px.iter().any(|&b| b > 16), "{} 缩略图全黑", s.name);
            assert!(
                thumbnail(s.kind, s.id)
                    .unwrap()
                    .starts_with("data:image/png;base64,")
            );
            // CDAPP_THUMB_DIR 时保存 PNG，供目测颜色。
            if let Some(dir) = std::env::var_os("CDAPP_THUMB_DIR") {
                let name = s.name.trim_start_matches(['\\', '.']);
                let png = encode_png(w, h, &px).unwrap();
                std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png)
                    .unwrap();
            }
        }
        super::super::bridge::stop();
    }

    /// 主屏显示红绿蓝三条竖带时，缩略图三条带中心的颜色正确；打印预览引擎下列源与缩略图耗时。
    /// 需 `CDAPP_VOLC_BIN`，并先运行 poc 的 bands.py。
    #[test]
    #[ignore = "需要 SDK 运行文件与色带窗口"]
    fn thumbnail_colors_and_preview_timing() {
        let dir = std::env::var_os("CDAPP_VOLC_BIN").expect("CDAPP_VOLC_BIN");
        bridge::start().unwrap();
        bridge::load(std::path::Path::new(&dir)).unwrap();
        let t = std::time::Instant::now();
        let list = bridge::sources().unwrap();
        println!(
            "临时引擎列源 {} 项，{} ms",
            list.len(),
            t.elapsed().as_millis()
        );
        let t = std::time::Instant::now();
        bridge::preview_open().unwrap();
        println!("建预览引擎 {} ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let list = bridge::sources().unwrap();
        println!(
            "预览引擎列源 {} 项，{} ms",
            list.len(),
            t.elapsed().as_millis()
        );
        let screen = list
            .iter()
            .find(|s| s.kind == SourceKind::Screen && s.primary)
            .unwrap();
        let start = std::time::Instant::now();
        let (w, h, px) = bridge::thumbnail(screen.kind, screen.id, THUMB_MAX).unwrap();
        println!("主屏缩略图 {w}x{h}，{} ms", start.elapsed().as_millis());
        // 三条色带中心，转换后 BGRA 依次为红、绿、蓝。
        let expect: [(u32, [u8; 3]); 3] = [(1, [0, 0, 255]), (3, [0, 255, 0]), (5, [255, 0, 0])];
        for (sixth, want) in expect {
            let at = ((h / 2 * w + w * sixth / 6) * 4) as usize;
            assert_eq!(px[at..at + 3], want, "第 {sixth} 段色带");
        }
        let windows: Vec<_> = list
            .iter()
            .filter(|s| s.kind == SourceKind::Window)
            .collect();
        let t = std::time::Instant::now();
        let ok = windows
            .iter()
            .filter(|s| bridge::thumbnail(s.kind, s.id, THUMB_MAX).is_ok())
            .count();
        println!(
            "窗口缩略图 {} 个，成功 {ok}，共 {} ms",
            windows.len(),
            t.elapsed().as_millis()
        );
        bridge::preview_close();
        bridge::stop();
    }
}
