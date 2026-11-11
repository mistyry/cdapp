//! Agora RTC SDK 运行文件，只用于观看分到 Agora 的分享者。启动时校验缓存，缺失或不符时从 Agora CDN 下载压缩包，
//! 只取观看所需的文件，就绪后加载。下载或加载失败时观看 Agora 分享不可用，其余功能照常。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

const VERSION: &str = "4.6.2.70";
/// Agora 原生 Windows SDK，`agora-electron-sdk` 4.6.2 的 `native_sdk_win` 指向此地址，不需登录。
const CDN_ZIP: &str = "https://download.agora.io/sdk/release/Agora_Native_SDK_for_Windows_rel.v4.6.2.70_31618_FULL_20260211_1724_1009714.zip";
const ZIP_PREFIX: &str = "Agora_Native_SDK_for_Windows_FULL/sdk/x86_64/";
const TIMEOUT: Duration = Duration::from_secs(900);

/// 下载地址。启用 `test-media` 特性时，`CDAPP_AGORA_CDN` 可替换，用于验收下载失败。
fn source() -> String {
    #[cfg(feature = "test-media")]
    if let Ok(v) = std::env::var("CDAPP_AGORA_CDN") {
        return v;
    }
    CDN_ZIP.to_owned()
}

struct Entry {
    path: &'static str,
    size: u64,
    sha256: &'static str,
}

/// 观看所需的运行文件：`agora_rtc_sdk.dll` 及其依赖的 Agora DLL，实测只放这些即可进房、取编码帧与播放声音。
/// 扩展类 DLL 用于降噪、美颜、编码等，观看不需要。
const FILES: &[Entry] = &[
    Entry {
        path: "agora_rtc_sdk.dll",
        size: 42_344_440,
        sha256: "95c476b049912ff18d7f679d1313400f963bc4af93ed7189cb995f2c4eae88c8",
    },
    Entry {
        path: "libaosl.dll",
        size: 1_302_008,
        sha256: "bf6763455b72c37c5755632ab281c54332515bf084b39b9f6c4fcb5d67a2c8a3",
    },
    Entry {
        path: "libagora-ffmpeg.dll",
        size: 8_536_056,
        sha256: "aecedd6fd3071d710b8ea5b42a30a467c4f5ac1b60b98bccc8a0ceeef8a0ceee",
    },
    Entry {
        path: "libagora-soundtouch.dll",
        size: 163_832,
        sha256: "76b84d0b789baa107f84db6bea4e04a14b500550a75f601e49ad09b73b2fc080",
    },
    Entry {
        path: "libagora-fdkaac.dll",
        size: 814_584,
        sha256: "e59fa9729f4a0ea7e982f66a31b30adc84b6966c512b890e44775d806f80ea6b",
    },
    Entry {
        path: "video_dec.dll",
        size: 1_305_080,
        sha256: "cc62296cbef0f8b3d29027a460f6f36fd54336d44996fd03edf992c66320f89f",
    },
];

/// 加载结果，未同步时为 None。只执行一次。
static STATUS: OnceLock<Result<String, String>> = OnceLock::new();

/// 已加载时返回版本。
#[must_use]
pub fn ready() -> bool {
    matches!(STATUS.get(), Some(Ok(_)))
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

fn extract(dir: &Path, zip: &[u8], want: &[&'static Entry]) -> AppResult<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip))
        .map_err(|e| AppError::Format(format!("压缩包损坏：{e}")))?;
    for e in want {
        let mut f = archive
            .by_name(&format!("{ZIP_PREFIX}{}", e.path))
            .map_err(|err| AppError::Format(format!("压缩包缺少 {}：{err}", e.path)))?;
        let mut bytes = Vec::with_capacity(usize::try_from(e.size).unwrap_or(0));
        f.read_to_end(&mut bytes)?;
        if !matches(e, &bytes) {
            return Err(AppError::Other(format!("{} 校验失败", e.path)));
        }
        crate::storage::write(&dir.join(e.path), &bytes)?;
    }
    Ok(())
}

fn sdk_dir() -> AppResult<PathBuf> {
    crate::storage::cache_file(&format!("agora/{VERSION}"))
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
    log::info!("Agora 观看组件缺少 {} 个文件，开始下载", want.len());
    let http = reqwest::Client::builder().timeout(TIMEOUT).build()?;
    let zip = http
        .get(source())
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let d = dir.clone();
    tauri::async_runtime::spawn_blocking(move || extract(&d, &zip, &want))
        .await
        .map_err(|e| AppError::Other(e.to_string()))??;
    Ok(dir)
}

fn load(dir: &Path) -> AppResult<String> {
    super::bridge::agora_load(dir).map_err(|e| AppError::Other(e.to_string()))?;
    Ok(super::bridge::agora_version())
}

/// 准备并加载 SDK，只执行一次。须在火山 SDK 加载之后调用，二者都设置进程的 DLL 搜索目录。
pub async fn sync() {
    if STATUS.get().is_some() {
        return;
    }
    let result = match prepare().await {
        Ok(dir) => tauri::async_runtime::spawn_blocking(move || load(&dir))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
            .and_then(|r| r),
        Err(e) => Err(e),
    };
    match &result {
        Ok(v) => log::info!("Agora 观看组件已加载：{v}"),
        Err(e) => log::warn!("Agora 观看组件不可用：{e}"),
    }
    let _ = STATUS.set(result.map_err(|e| e.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_well_formed() {
        assert_eq!(FILES.len(), 6);
        for e in FILES {
            assert_eq!(e.sha256.len(), 64, "{}", e.path);
            assert!(
                e.sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
        }
    }

    /// 需要本机 SDK 压缩包，用环境变量 `CDAPP_AGORA_ZIP` 指定。取出清单内文件、校验并加载。
    #[test]
    #[ignore = "需要 SDK 压缩包"]
    fn extracts_and_loads_zip() {
        let zip = std::fs::read(std::env::var_os("CDAPP_AGORA_ZIP").unwrap()).unwrap();
        let d = std::env::temp_dir().join(format!("cdapp-agora-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        crate::storage::init(&d.join("fixed"), &d.join("data"), &d.join("fixed"));
        let dir = sdk_dir().unwrap();
        extract(&dir, &zip, &missing(&dir)).unwrap();
        assert!(missing(&dir).is_empty());
        let v = load(&dir).unwrap();
        assert!(v.starts_with("4.6.2"), "{v}");
    }
}
