use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};

use super::bridge::{self, SourceKind};

pub const THUMB_MAX: (u32, u32) = (480, 270);

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

#[must_use]
pub fn size(kind: SourceKind, id: u64) -> Option<(u32, u32)> {
    let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let rect = match kind {
        SourceKind::Screen => monitor(id),
        SourceKind::Window => window(id),
    };
    if !prev.is_invalid() {
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
    let ok = unsafe { GetMonitorInfoW(HMONITOR(id as *mut _), &raw mut mi) };
    ok.as_bool().then_some(mi.rcMonitor)
}

fn window(id: u64) -> Option<RECT> {
    let mut r = RECT::default();
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
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let dec = png::Decoder::new(std::io::Cursor::new(png));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        r.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..6], [0, 0, 255, 255, 0, 0]);
    }

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
            assert!(px.iter().any(|&b| b > 16), "{} 缩略图全黑", s.name);
            assert!(
                thumbnail(s.kind, s.id)
                    .unwrap()
                    .starts_with("data:image/png;base64,")
            );
            if let Some(dir) = std::env::var_os("CDAPP_THUMB_DIR") {
                let name = s.name.trim_start_matches(['\\', '.']);
                let png = encode_png(w, h, &px).unwrap();
                std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png)
                    .unwrap();
            }
        }
        super::super::bridge::stop();
    }

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
