//! 本机音频偏好：输入输出设备、输入增益与输出总音量。
//! 保存在应用数据目录的 `audio.json`，与凭据分开。文件损坏或字段缺失时使用默认值。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::dsp::MAX_GAIN;
use super::mixer::MAX_VOLUME;

static PATH: OnceLock<PathBuf> = OnceLock::new();
const FILE: &str = "audio.json";

/// 音频偏好。设备为 None 时跟随系统默认。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioPrefs {
    pub input: Option<String>,
    pub output: Option<String>,
    /// 输入增益百分比，0 至 200。
    pub gain: u16,
    /// 输出总音量百分比，0 至 200。
    pub master: u16,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            gain: 100,
            master: 100,
        }
    }
}

impl AudioPrefs {
    /// 截断越界数值，空设备 ID 视为系统默认。
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.gain = self.gain.min(MAX_GAIN);
        self.master = self.master.min(MAX_VOLUME);
        self.input = self.input.filter(|s| !s.is_empty());
        self.output = self.output.filter(|s| !s.is_empty());
        self
    }

    #[must_use]
    pub fn parse(bytes: &[u8]) -> Self {
        serde_json::from_slice::<Self>(bytes)
            .unwrap_or_default()
            .sanitized()
    }
}

/// 设置偏好所在目录。只在启动时调用一次。
pub fn init(dir: &Path) {
    let _ = PATH.set(dir.join(FILE));
}

/// 读取偏好。未初始化、文件不存在或损坏时返回默认值。
#[must_use]
pub fn load() -> AudioPrefs {
    PATH.get()
        .and_then(|p| fs::read(p).ok())
        .map_or_else(AudioPrefs::default, |b| AudioPrefs::parse(&b))
}

/// 原子写入偏好。未初始化时不写入。
///
/// # Errors
///
/// 目录创建或写入失败时返回错误。
pub fn save(prefs: &AudioPrefs) -> io::Result<()> {
    let Some(path) = PATH.get() else {
        return Ok(());
    };
    write_atomic(path, &serde_json::to_vec_pretty(prefs)?)
}

/// 待保存的最新偏好与写入锁。连续修改时只写最新值，写入顺序不会倒置。
static PENDING: Mutex<Option<AudioPrefs>> = Mutex::new(None);
static WRITER: Mutex<()> = Mutex::new(());

/// 在后台线程保存偏好，不阻塞调用方。失败只记录日志。
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

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)
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
        };
        write_atomic(&path, &serde_json::to_vec(&p).unwrap()).unwrap();
        assert_eq!(AudioPrefs::parse(&fs::read(&path).unwrap()), p);
        assert!(!path.with_extension("tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
