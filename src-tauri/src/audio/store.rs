use std::fs;
use std::io;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::dsp::MAX_GAIN;
use super::mixer::MAX_VOLUME;

const FILE: &str = "audio.json";
pub const DEFAULT_EFFECT_VOLUME: u16 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioPrefs {
    pub input: Option<String>,
    pub output: Option<String>,
    pub gain: u16,
    pub master: u16,
    pub effect_volume: u16,
    pub effects_off: Vec<u8>,
    pub muted: bool,
    pub deafened: bool,
    pub denoise: bool,
    pub denoise_mode: super::denoise::Mode,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            gain: 100,
            master: 100,
            effect_volume: DEFAULT_EFFECT_VOLUME,
            effects_off: Vec::new(),
            muted: false,
            deafened: false,
            denoise: false,
            denoise_mode: super::denoise::Mode::Kook,
        }
    }
}

impl AudioPrefs {
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.gain = self.gain.min(MAX_GAIN);
        self.master = self.master.min(MAX_VOLUME);
        self.input = self.input.filter(|s| !s.is_empty());
        self.output = self.output.filter(|s| !s.is_empty());
        self.effect_volume = self.effect_volume.min(100);
        self.effects_off.sort_unstable();
        self.effects_off.dedup();
        self
    }

    #[must_use]
    pub fn parse(bytes: &[u8]) -> Self {
        serde_json::from_slice::<Self>(bytes)
            .unwrap_or_default()
            .sanitized()
    }
}

#[must_use]
pub fn load() -> AudioPrefs {
    crate::storage::config_file(FILE)
        .and_then(|p| fs::read(p).ok())
        .map_or_else(AudioPrefs::default, |b| AudioPrefs::parse(&b))
}

/// # Errors
///
/// 目录创建或写入失败时返回错误。
pub fn save(prefs: &AudioPrefs) -> io::Result<()> {
    let Some(path) = crate::storage::config_file(FILE) else {
        return Ok(());
    };
    crate::storage::write(&path, &serde_json::to_vec_pretty(prefs)?)
}

static PENDING: Mutex<Option<AudioPrefs>> = Mutex::new(None);
static WRITER: Mutex<()> = Mutex::new(());

pub fn save_later(prefs: AudioPrefs) {
    if let Ok(mut p) = PENDING.lock() {
        *p = Some(prefs);
    }
    tauri::async_runtime::spawn_blocking(|| {
        let _w = WRITER.lock();
        let next = PENDING.lock().ok().and_then(|mut p| p.take());
        if let Some(p) = next
            && let Err(e) = save(&p)
        {
            log::warn!("音频偏好保存失败：{e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tolerates_missing_and_invalid_fields() {
        assert_eq!(AudioPrefs::parse(b"not json"), AudioPrefs::default());
        assert_eq!(AudioPrefs::parse(b"{}"), AudioPrefs::default());
        let p = AudioPrefs::parse(br#"{"input":"","output":"o","gain":900,"master":50}"#);
        assert_eq!(p.input, None);
        assert_eq!(p.output.as_deref(), Some("o"));
        assert_eq!(p.gain, MAX_GAIN);
        assert_eq!(p.master, 50);
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = std::env::temp_dir().join(format!("cdapp-audio-{}", std::process::id()));
        let path = dir.join(FILE);
        let p = AudioPrefs {
            input: Some("in".into()),
            output: None,
            gain: 150,
            master: 80,
            effect_volume: 20,
            effects_off: vec![2, 10],
            muted: true,
            deafened: false,
            denoise: true,
            denoise_mode: crate::audio::denoise::Mode::Rnnoise,
        };
        crate::storage::write_raw(&path, &serde_json::to_vec(&p).unwrap(), false).unwrap();
        assert_eq!(AudioPrefs::parse(&fs::read(&path).unwrap()), p);
        assert!(!path.with_extension("tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
