use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

const VERSION: &str = "3.58.1.100";
const CDN_ZIP: &str = "https://hstob-cdn-tos.volccdn.com/volcengine/VolcEngineRTC-Electron-sdk-Win64/3.58.4/vertc-electron-addon.zip";
const ZIP_PREFIX: &str = "build/Release/";
const GITHUB_BASE: &str = "https://raw.githubusercontent.com/volcengine/VolcEngineRTC/4f0cb73622/Windows/3rd/Windows/VolcEngineRTC/bin/x64/";
const TIMEOUT: Duration = Duration::from_secs(600);

fn source(default: &'static str, env: &str) -> String {
    #[cfg(feature = "test-media")]
    if let Ok(v) = std::env::var(env) {
        return v;
    }
    #[cfg(not(feature = "test-media"))]
    let _ = env;
    default.to_owned()
}

struct Entry {
    path: &'static str,
    size: u64,
    sha256: &'static str,
}

const FILES: &[Entry] = &[
    Entry {
        path: "VolcEngineRTC.dll",
        size: 47_250_208,
        sha256: "53e7560b18169acca049c26dfeb6d8dcfeb314d5db9bb3486ee0958c46863fe7",
    },
    Entry {
        path: "RTCFFmpeg.dll",
        size: 7_112_480,
        sha256: "8ffac1b503230f9a68c2da7e8edb5c6d4653cb95b0392ca11934fc591f5c7b3b",
    },
    Entry {
        path: "openh264-4.dll",
        size: 1_319_200,
        sha256: "eed5fee51178833c21b231e09822178d0d5fe5a73d24b20bdcba06498c7174a1",
    },
    Entry {
        path: "bytertc_fdk-aac_extension.dll",
        size: 644_384,
        sha256: "5d71e476b3899edca1a954bb623769204bf356146b6cadcb985264781ab47fdc",
    },
    Entry {
        path: "bytertc_ffmpeg_audio_extension.dll",
        size: 7_849_760,
        sha256: "405b333a0039854ec9348453b23ba39657154cf924a4c33ee39a3022e333544c",
    },
    Entry {
        path: "bytertc_nico_extension.dll",
        size: 573_216,
        sha256: "d80bd58f2a58c389f2349a1f60d05613040bd8d91db7ac6ba76e27abed8a3100",
    },
    Entry {
        path: "bytertc_vp8codec_extension.dll",
        size: 3_886_368,
        sha256: "8d32be784ca7d25276894c36b0b8590212f586be22a7fa8daf8a8119bdd95c72",
    },
    Entry {
        path: "libEGL.dll",
        size: 21_280,
        sha256: "1f600b63100b609aaca7b169e75937e1bc31b1d27e95570f429a43ccf59c7c5e",
    },
    Entry {
        path: "libGLESv2.dll",
        size: 3_575_584,
        sha256: "6e50e17327a000b7088f69d4f1990d8a9e7690a235058af14e3fa1dba9f10707",
    },
    Entry {
        path: "AUDIO_MODEL/models/common_ainr.model",
        size: 1_181_671,
        sha256: "a9d2fc07038ccfcb5fc90a6d8da8f05bd5b97a8a59c66b776aa048d9e1967390",
    },
    Entry {
        path: "AUDIO_MODEL/models/crn.model",
        size: 265_152,
        sha256: "3081aaf1665a72443989e600f79a61afd0a542d18ca0caaaca207e8de548af95",
    },
    Entry {
        path: "AUDIO_MODEL/models/fullbandnr.model",
        size: 373_476,
        sha256: "c1fbb38314eb7ac969f2ec7af144f8b440f31527be0e08c2ed4b2897bfc6005e",
    },
    Entry {
        path: "AUDIO_MODEL/models/howlnr.model",
        size: 375_425,
        sha256: "1f0b01557bcc9e4d03b2177cf2276ecd2568b6a7ea7ae2308f21101ca650b13d",
    },
    Entry {
        path: "AUDIO_MODEL/models/quality_v1.model",
        size: 61_086,
        sha256: "98eaf4ec41e38545dacac9583be78de9f53e29defe083661f47ae423e9d1b24d",
    },
    Entry {
        path: "AUDIO_MODEL/models/rooms_v2.model",
        size: 180_966,
        sha256: "1118d5f66769e9efc50bc91f068dab7ed4036fd406856bc9298e3312030c77f7",
    },
    Entry {
        path: "AUDIO_MODEL/models/rooms_v3.model",
        size: 866_764,
        sha256: "08947e236e96e343ebe9793f4952ae265f6379bb2092469c0713320ba7e1b02f",
    },
];

pub use crate::commands::ShareSdk as Status;

static STATUS: OnceLock<Status> = OnceLock::new();

#[must_use]
pub fn status() -> Option<Status> {
    STATUS.get().cloned()
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

fn matches(e: &Entry, bytes: &[u8]) -> bool {
    bytes.len() as u64 == e.size && hex(bytes) == e.sha256
}

fn missing(dir: &Path) -> Vec<&'static Entry> {
    FILES
        .iter()
        .filter(|e| {
            let p = dir.join(e.path);
            !std::fs::metadata(&p).is_ok_and(|m| m.len() == e.size)
                || !std::fs::read(&p).is_ok_and(|b| matches(e, &b))
        })
        .collect()
}

fn store(dir: &Path, e: &Entry, bytes: &[u8]) -> AppResult<()> {
    if !matches(e, bytes) {
        return Err(AppError::Other(format!("{} 校验失败", e.path)));
    }
    crate::storage::write(&dir.join(e.path), bytes)?;
    Ok(())
}

fn extract(dir: &Path, zip: &[u8], want: &[&'static Entry]) -> AppResult<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip))
        .map_err(|e| AppError::Format(format!("压缩包损坏：{e}")))?;
    for e in want {
        let mut f = archive
            .by_name(&format!("{ZIP_PREFIX}{}", e.path))
            .map_err(|err| AppError::Format(format!("压缩包缺少 {}：{err}", e.path)))?;
        let mut bytes = Vec::with_capacity(usize::try_from(e.size).unwrap_or(0));
        f.read_to_end(&mut bytes)?;
        store(dir, e, &bytes)?;
    }
    Ok(())
}

async fn fetch(http: &reqwest::Client, url: &str) -> AppResult<bytes::Bytes> {
    Ok(http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?)
}

async fn from_cdn(http: &reqwest::Client, dir: &Path, want: &[&'static Entry]) -> AppResult<()> {
    let zip = fetch(http, &source(CDN_ZIP, "CDAPP_VOLC_CDN")).await?;
    let (dir, want) = (dir.to_path_buf(), want.to_vec());
    tauri::async_runtime::spawn_blocking(move || extract(&dir, &zip, &want))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

async fn from_github(http: &reqwest::Client, dir: &Path, want: &[&'static Entry]) -> AppResult<()> {
    let base = source(GITHUB_BASE, "CDAPP_VOLC_GITHUB");
    for e in want {
        let bytes = fetch(http, &format!("{base}{}", e.path)).await?;
        let dir = dir.to_path_buf();
        let e: &'static Entry = e;
        tauri::async_runtime::spawn_blocking(move || store(&dir, e, &bytes))
            .await
            .map_err(|err| AppError::Other(err.to_string()))??;
    }
    Ok(())
}

fn sdk_dir() -> AppResult<PathBuf> {
    crate::storage::cache_file(&format!("volc/{VERSION}"))
        .ok_or_else(|| AppError::Other("数据目录不可用".into()))
}

async fn prepare() -> AppResult<PathBuf> {
    let dir = sdk_dir()?;
    let d = dir.clone();
    let want = tauri::async_runtime::spawn_blocking(move || missing(&d))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    if want.is_empty() {
        return Ok(dir);
    }
    log::info!("屏幕分享组件缺少 {} 个文件，开始下载", want.len());
    let http = reqwest::Client::builder().timeout(TIMEOUT).build()?;
    if let Err(e) = from_cdn(&http, &dir, &want).await {
        log::warn!("屏幕分享组件从火山 CDN 下载失败：{e}");
        let d = dir.clone();
        let want = tauri::async_runtime::spawn_blocking(move || missing(&d))
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        from_github(&http, &dir, &want).await?;
    }
    Ok(dir)
}

fn load(dir: &Path) -> AppResult<String> {
    super::bridge::volc::start().map_err(|e| AppError::Other(e.to_string()))?;
    super::bridge::volc::load(dir).map_err(|e| AppError::Other(e.to_string()))?;
    Ok(super::bridge::volc::version())
}

pub async fn sync() -> Status {
    if let Some(s) = status() {
        return s;
    }
    let result = match prepare().await {
        Ok(dir) => tauri::async_runtime::spawn_blocking(move || load(&dir))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
            .and_then(|r| r),
        Err(e) => Err(e),
    };
    let s = match result {
        Ok(version) => {
            log::info!("屏幕分享组件已加载：{version}");
            Status::Ready { version }
        }
        Err(e) => {
            log::warn!("屏幕分享组件不可用：{e}");
            Status::Unavailable {
                message: e.to_string(),
            }
        }
    };
    STATUS.get_or_init(|| s).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cdapp-volc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn manifest_is_well_formed() {
        assert_eq!(FILES.len(), 16);
        for e in FILES {
            assert_eq!(e.sha256.len(), 64, "{}", e.path);
            assert!(
                e.sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn empty_dir_misses_everything_and_wrong_size_is_rejected() {
        let d = temp("missing");
        assert_eq!(missing(&d).len(), FILES.len());
        std::fs::write(d.join("libEGL.dll"), b"x").unwrap();
        assert_eq!(missing(&d).len(), FILES.len());
        assert!(!matches(&FILES[7], b"x"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    #[ignore = "需要 SDK 压缩包"]
    fn extracts_cdn_zip() {
        let zip = std::fs::read(std::env::var_os("CDAPP_VOLC_ZIP").unwrap()).unwrap();
        let d = temp("zip");
        extract(&d, &zip, &missing(&d)).unwrap();
        assert!(missing(&d).is_empty());
        std::fs::write(d.join("libEGL.dll"), vec![0u8; 21_280]).unwrap();
        let want = missing(&d);
        assert_eq!(want.len(), 1);
        extract(&d, &zip, &want).unwrap();
        assert!(missing(&d).is_empty());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    #[ignore = "需要联网"]
    fn downloads_and_loads() {
        let d = temp("net");
        crate::storage::init(&d.join("fixed"), &d.join("data"), &d.join("fixed"));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let s = rt.block_on(sync());
        assert!(
            matches!(&s, Status::Ready { version } if version.starts_with("3.58")),
            "{s:?}"
        );
        assert!(missing(&sdk_dir().unwrap()).is_empty());
    }

    #[test]
    #[ignore = "需要联网"]
    fn downloads_from_github() {
        let d = temp("github");
        let egl = FILES.iter().find(|e| e.path == "libEGL.dll").unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut b = reqwest::Client::builder().timeout(TIMEOUT);
        if let Ok(p) = std::env::var("CDAPP_TEST_PROXY") {
            b = b.proxy(reqwest::Proxy::all(p).unwrap());
        }
        rt.block_on(from_github(&b.build().unwrap(), &d, &[egl]))
            .unwrap();
        assert_eq!(missing(&d).len(), FILES.len() - 1);
        std::fs::remove_dir_all(d).unwrap();
    }
}
