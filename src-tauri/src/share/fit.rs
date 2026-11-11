//! 编码尺寸换算。SDK 内部采集按像素总数缩放并保持源宽高比，非 16:9 的源会超出档位高度，
//! 因此按源宽高比算出能放进档位框的最大尺寸再交给编码配置。

/// 档位框：高为档位值，宽按 16:9。
#[must_use]
pub fn tier_box(resolution: u32) -> (u32, u32) {
    (even(resolution * 16 / 9), even(resolution))
}

/// 源尺寸缩进档位框后的编码尺寸，保持宽高比，不放大，宽高取偶数。源尺寸无效时返回档位框。
#[must_use]
pub fn fit(resolution: u32, source: (u32, u32)) -> (u32, u32) {
    let (bw, bh) = tier_box(resolution);
    let (sw, sh) = source;
    if sw == 0 || sh == 0 {
        return (bw, bh);
    }
    if sw <= bw && sh <= bh {
        return (even(sw), even(sh));
    }
    // 比较 sw/sh 与 bw/bh，用乘法避免浮点误差。
    let (w, h) = if u64::from(sw) * u64::from(bh) >= u64::from(bw) * u64::from(sh) {
        (bw, scale(sh, bw, sw))
    } else {
        (scale(sw, bh, sh), bh)
    };
    (even(w).max(2), even(h).max(2))
}

/// 服务端码率上限换算为 kbps，向下取整。
#[must_use]
pub fn kbps(max_bitrate: u64) -> u32 {
    u32::try_from(max_bitrate / 1000).unwrap_or(u32::MAX)
}

fn scale(v: u32, num: u32, den: u32) -> u32 {
    u32::try_from(u64::from(v) * u64::from(num) / u64::from(den)).unwrap_or(u32::MAX)
}

fn even(v: u32) -> u32 {
    v & !1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_boxes() {
        assert_eq!(tier_box(720), (1280, 720));
        assert_eq!(tier_box(1080), (1920, 1080));
    }

    #[test]
    fn sixteen_by_ten_fits_height() {
        // 实测：16:10 屏 1080p 档设 1728×1080，输出 1728×1080。
        assert_eq!(fit(1080, (1920, 1200)), (1728, 1080));
        assert_eq!(fit(720, (2560, 1600)), (1152, 720));
    }

    #[test]
    fn sixteen_by_nine_fills_box() {
        assert_eq!(fit(1080, (1920, 1080)), (1920, 1080));
        assert_eq!(fit(720, (3840, 2160)), (1280, 720));
    }

    #[test]
    fn ultrawide_fits_width() {
        assert_eq!(fit(1080, (3440, 1440)), (1920, 802));
    }

    #[test]
    fn small_source_is_not_upscaled() {
        assert_eq!(fit(1080, (1280, 721)), (1280, 720));
        assert_eq!(fit(1080, (0, 1080)), (1920, 1080));
    }

    #[test]
    fn bitrate_to_kbps() {
        assert_eq!(kbps(4_250_000), 4250);
        assert_eq!(kbps(2_850_999), 2850);
    }
}
