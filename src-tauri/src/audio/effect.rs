use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::dsp::Resampler;

const MAX_SECONDS: u32 = 10;
const MAX_PLAYING: usize = 4;

static PLAYING: AtomicUsize = AtomicUsize::new(0);

/// # Errors
///
/// 文件无法读取或格式不受支持时返回错误。
pub fn decode(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(
        Box::new(file),
        symphonia::core::io::MediaSourceStreamOptions::default(),
    );
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "没有音轨".to_owned())?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.unwrap_or(48_000);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let limit = (rate * MAX_SECONDS) as usize;
    let mut out = Vec::new();
    while out.len() < limit {
        let Ok(packet) = format.next_packet() else {
            break;
        };
        if packet.track_id() != track_id {
            continue;
        }
        let Ok(buf) = decoder.decode(&packet) else {
            continue;
        };
        let spec = *buf.spec();
        let channels = spec.channels.count().max(1);
        let mut samples = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
        samples.copy_interleaved_ref(buf);
        out.extend(super::dsp::mixdown(samples.samples(), channels));
    }
    out.truncate(limit);
    if out.is_empty() {
        return Err("没有可播放的内容".into());
    }
    Ok((out, rate))
}

pub fn play(path: &Path, device: Option<String>, volume: f32) {
    if volume <= 0.0 || PLAYING.load(Ordering::SeqCst) >= MAX_PLAYING {
        return;
    }
    let path = path.to_path_buf();
    PLAYING.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new()
        .name("effect-out".into())
        .spawn(move || {
            if let Err(e) = run(&path, device.as_deref(), volume) {
                log::info!("提示音播放失败 {}：{e}", path.display());
            }
            PLAYING.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        PLAYING.fetch_sub(1, Ordering::SeqCst);
    }
}

fn find_device(id: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    id.and_then(|id| id.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
        .or_else(|| host.default_output_device())
}

fn run(path: &Path, device: Option<&str>, volume: f32) -> Result<(), String> {
    let (samples, rate) = decode(path)?;
    let device = find_device(device).ok_or_else(|| "没有输出设备".to_owned())?;
    let cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let config: StreamConfig = cfg.config();
    let channels = usize::from(config.channels.max(1));
    let mut data = Resampler::new(rate, config.sample_rate).process(&samples);
    for s in &mut data {
        *s = (*s * volume).clamp(-1.0, 1.0);
    }
    let frames = data.len();
    let millis = u64::try_from(frames)
        .unwrap_or(u64::MAX)
        .saturating_mul(1000)
        / u64::from(config.sample_rate.max(1));
    let mut pos = 0usize;
    macro_rules! build {
        ($t:ty) => {{
            device.build_output_stream::<$t, _, _>(
                config,
                move |d: &mut [$t], _| {
                    for frame in d.chunks_mut(channels) {
                        let s = data.get(pos).copied().unwrap_or(0.0);
                        pos += 1;
                        frame.fill(<$t as cpal::FromSample<f32>>::from_sample_(s));
                    }
                },
                |e| log::info!("提示音输出错误：{e}"),
                Some(Duration::from_secs(5)),
            )
        }};
    }
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U8 => build!(u8),
        SampleFormat::I8 => build!(i8),
        f => return Err(format!("不支持的样本格式 {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(millis + 200));
    drop(stream);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_wav() {
        let dir = std::env::temp_dir().join(format!("cdapp-effect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let n: u32 = 100;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + n * 2).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&8000u32.to_le_bytes());
        b.extend_from_slice(&16000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(n * 2).to_le_bytes());
        for i in 0..n {
            b.extend_from_slice(&(i16::try_from(i).unwrap() * 100).to_le_bytes());
        }
        std::fs::write(&path, &b).unwrap();
        let (s, rate) = decode(&path).unwrap();
        assert_eq!(rate, 8000);
        assert_eq!(s.len(), 100);
        assert!(decode(&dir.join("none.wav")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
