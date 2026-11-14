use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::{AppError, AppResult};

const LOCATION: &str = "location.json";
const MARKER: &str = "root.json";
const LOCK: &str = ".lock";
const CREDENTIALS: &str = "credentials.json";
const VERSION: u64 = 1;
const CACHE_LIMIT: u64 = 512 * 1024 * 1024;
const ACCOUNT_LIMIT: usize = 256 * 1024;
const ACCOUNT_KEYS: &[&str] = &[
    "emojiRecent",
    "openFolders",
    "peerVolumes",
    "peerDenoise",
    "shareVolumes",
    "shareBeforeMute",
    "shareAudio",
    "stageCollapsed",
];

#[derive(Default, Serialize, Deserialize)]
struct Location {
    #[serde(default)]
    v: u64,
    #[serde(default)]
    root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remove: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    Ready,
    Missing,
    Unwritable { message: String },
    Locked,
}

pub struct Storage {
    fixed: PathBuf,
    default_root: PathBuf,
    root: PathBuf,
    status: Status,
    _lock: Option<File>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    #[serde(flatten)]
    pub status: Status,
    pub root: String,
    pub default_root: String,
    pub custom: bool,
}

static STORAGE: OnceLock<Storage> = OnceLock::new();
static GATE: RwLock<()> = RwLock::new(());
static ACCOUNT: Mutex<()> = Mutex::new(());
static FROZEN: AtomicBool = AtomicBool::new(false);

pub fn init(fixed: &Path, default_root: &Path, legacy: &Path) {
    let s = open(fixed, default_root, legacy);
    if s.status != Status::Ready {
        log::warn!("数据目录不可用：{:?}", s.status);
    }
    let _ = STORAGE.set(s);
}

fn open(fixed: &Path, default_root: &Path, legacy: &Path) -> Storage {
    let loc = read_location(fixed);
    let custom = loc.root.clone().filter(|p| p.is_absolute());
    let root = custom.clone().unwrap_or_else(|| default_root.to_path_buf());
    let mut status = prepare(&root, custom.is_none());
    let mut lock = None;
    if status == Status::Ready {
        match acquire(&root) {
            Ok(f) => lock = Some(f),
            Err(fs::TryLockError::WouldBlock) => status = Status::Locked,
            Err(fs::TryLockError::Error(e)) => {
                status = Status::Unwritable {
                    message: e.to_string(),
                };
            }
        }
    }
    move_file(&legacy.join(CREDENTIALS), &fixed.join(CREDENTIALS));
    if status == Status::Ready {
        if let Some(old) = loc.remove.as_deref() {
            if old != root
                && old.join(MARKER).is_file()
                && let Err(e) = remove_root(old)
            {
                log::warn!("旧数据目录删除失败：{e}");
            }
            let _ = write_location(fixed, custom.as_deref());
        }
        move_file(
            &legacy.join("audio.json"),
            &root.join("config").join("audio.json"),
        );
        move_file(
            &legacy.join("saying.json"),
            &root.join("cache").join("saying.json"),
        );
        prune(&root.join("cache"), CACHE_LIMIT);
    }
    Storage {
        fixed: fixed.to_path_buf(),
        default_root: default_root.to_path_buf(),
        root,
        status,
        _lock: lock,
    }
}

fn read_location(fixed: &Path) -> Location {
    fs::read(fixed.join(LOCATION))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_location(fixed: &Path, root: Option<&Path>) -> io::Result<()> {
    write_location_full(fixed, root, None)
}

fn write_location_full(fixed: &Path, root: Option<&Path>, remove: Option<&Path>) -> io::Result<()> {
    let loc = Location {
        v: VERSION,
        root: root.map(Path::to_path_buf),
        remove: remove.map(Path::to_path_buf),
    };
    write_raw(
        &fixed.join(LOCATION),
        &serde_json::to_vec_pretty(&loc)?,
        false,
    )
}

fn prepare(root: &Path, is_default: bool) -> Status {
    if root.join(MARKER).is_file() {
        return Status::Ready;
    }
    if !is_default {
        return Status::Missing;
    }
    match write_marker(root) {
        Ok(()) => Status::Ready,
        Err(e) => Status::Unwritable {
            message: e.to_string(),
        },
    }
}

fn write_marker(root: &Path) -> io::Result<()> {
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let bytes = serde_json::to_vec_pretty(&json!({ "v": VERSION, "created": created }))?;
    write_raw(&root.join(MARKER), &bytes, false)
}

fn acquire(root: &Path) -> Result<File, fs::TryLockError> {
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK))
        .map_err(fs::TryLockError::Error)?;
    f.try_lock()?;
    Ok(f)
}

fn move_file(from: &Path, to: &Path) {
    if from == to || !from.is_file() {
        return;
    }
    let r = (|| -> io::Result<()> {
        if !to.exists() {
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir)?;
            }
            if fs::rename(from, to).is_ok() {
                return Ok(());
            }
            fs::copy(from, to)?;
        }
        fs::remove_file(from)
    })();
    if let Err(e) = r {
        log::warn!("旧文件迁移失败：{e}");
    }
}

fn ready() -> AppResult<&'static Storage> {
    STORAGE
        .get()
        .filter(|s| s.status == Status::Ready)
        .ok_or_else(|| AppError::Other("数据目录不可用".into()))
}

fn data_file(dir: &str, name: &str) -> Option<PathBuf> {
    ready().ok().map(|s| s.root.join(dir).join(name))
}

#[must_use]
pub fn config_file(name: &str) -> Option<PathBuf> {
    data_file("config", name)
}

#[must_use]
pub fn cache_file(name: &str) -> Option<PathBuf> {
    data_file("cache", name)
}

#[must_use]
pub fn credentials() -> Option<PathBuf> {
    STORAGE.get().map(|s| s.fixed.join(CREDENTIALS))
}

/// # Errors
///
/// 目录创建或写入失败时返回错误。
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let _g = GATE.read().map_err(|_| io::Error::other("存储锁失效"))?;
    if FROZEN.load(Ordering::SeqCst) {
        return Err(io::Error::other("数据目录正在迁移"));
    }
    write_raw(path, bytes, false)
}

/// # Errors
///
/// 目录创建或写入失败时返回错误。
pub fn write_raw(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)
}

#[must_use]
pub fn info() -> Info {
    STORAGE.get().map_or_else(
        || Info {
            status: Status::Unwritable {
                message: "存储未初始化".into(),
            },
            root: String::new(),
            default_root: String::new(),
            custom: false,
        },
        |s| Info {
            status: s.status.clone(),
            root: s.root.display().to_string(),
            default_root: s.default_root.display().to_string(),
            custom: s.root != s.default_root,
        },
    )
}

#[must_use]
pub fn cache_size() -> u64 {
    ready().map_or(0, |s| {
        files(&s.root.join("cache")).iter().map(|f| f.1).sum()
    })
}

/// # Errors
///
/// 数据根不可用或删除失败时返回错误。
pub fn clear_cache() -> AppResult<()> {
    let s = ready()?;
    let _g = GATE
        .write()
        .map_err(|_| AppError::Other("存储锁失效".into()))?;
    let dir = s.root.join("cache");
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    fs::create_dir_all(dir)?;
    Ok(())
}

/// # Errors
///
/// 目标无效、不为空或复制校验失败时返回错误，当前位置保持可用。
pub fn relocate(target: &Path) -> AppResult<()> {
    let s = ready()?;
    let target = checked_target(&s.root, target)?;
    let _g = GATE
        .write()
        .map_err(|_| AppError::Other("存储锁失效".into()))?;
    if FROZEN.load(Ordering::SeqCst) {
        return Err(AppError::Other("数据目录正在迁移".into()));
    }
    if target.join(MARKER).is_file() {
        write_location(&s.fixed, as_custom(s, &target))?;
    } else {
        if fs::read_dir(&target).is_ok_and(|mut d| d.next().is_some()) {
            return Err(AppError::Other("所选文件夹不为空".into()));
        }
        let existed = target.exists();
        if let Err(e) = copy_root(&s.root, &target) {
            if existed {
                clear_dir(&target);
            } else {
                let _ = fs::remove_dir_all(&target);
            }
            return Err(e);
        }
        write_location_full(&s.fixed, as_custom(s, &target), Some(&s.root))?;
    }
    FROZEN.store(true, Ordering::SeqCst);
    Ok(())
}

/// # Errors
///
/// 目标不是本应用数据根或写入失败时返回错误。
pub fn switch(target: Option<&Path>) -> AppResult<()> {
    let s = STORAGE
        .get()
        .ok_or_else(|| AppError::Other("存储未初始化".into()))?;
    let root = match target {
        Some(t) => {
            let t = fs::canonicalize(t)?;
            if !t.join(MARKER).is_file() {
                return Err(AppError::Other("所选文件夹不是 cdapp 数据目录".into()));
            }
            as_custom(s, &t).map(Path::to_path_buf)
        }
        None => None,
    };
    write_location(&s.fixed, root.as_deref())?;
    Ok(())
}

fn as_custom<'a>(s: &Storage, target: &'a Path) -> Option<&'a Path> {
    let default = fs::canonicalize(&s.default_root).unwrap_or_else(|_| s.default_root.clone());
    (target != default).then_some(target)
}

fn checked_target(root: &Path, target: &Path) -> AppResult<PathBuf> {
    if !target.is_absolute() {
        return Err(AppError::Other("请选择完整的文件夹路径".into()));
    }
    let target = if target.exists() {
        fs::canonicalize(target)?
    } else {
        let parent = target
            .parent()
            .ok_or_else(|| AppError::Other("无法使用该位置".into()))?;
        fs::canonicalize(parent)?.join(target.file_name().unwrap_or_default())
    };
    let root = fs::canonicalize(root)?;
    if target == root {
        return Err(AppError::Other("已是当前数据位置".into()));
    }
    if target.starts_with(&root) || root.starts_with(&target) {
        return Err(AppError::Other("新位置不能与当前位置互相包含".into()));
    }
    Ok(target)
}

fn copy_root(from: &Path, to: &Path) -> AppResult<()> {
    let list = files(from);
    for (rel, size) in &list {
        if [LOCK, MARKER, LOCATION, CREDENTIALS]
            .iter()
            .any(|n| rel == Path::new(n))
        {
            continue;
        }
        let dst = to.join(rel);
        if let Some(dir) = dst.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::copy(from.join(rel), &dst)?;
        if fs::metadata(&dst)?.len() != *size {
            return Err(AppError::Other("复制校验失败".into()));
        }
    }
    fs::copy(from.join(MARKER), to.join(MARKER))?;
    Ok(())
}

fn remove_root(root: &Path) -> io::Result<()> {
    for dir in ["config", "accounts", "cache"] {
        match fs::remove_dir_all(root.join(dir)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    for file in [LOCK, MARKER] {
        let _ = fs::remove_file(root.join(file));
    }
    let _ = fs::remove_dir(root);
    Ok(())
}

fn clear_dir(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            let _ = if p.is_dir() {
                fs::remove_dir_all(p)
            } else {
                fs::remove_file(p)
            };
        }
    }
}

fn files(dir: &Path) -> Vec<(PathBuf, u64)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let Ok(meta) = fs::symlink_metadata(e.path()) else {
                continue;
            };
            if meta.is_dir() {
                walk(base, &e.path(), out);
            } else if meta.is_file()
                && let Ok(rel) = e.path().strip_prefix(base)
            {
                out.push((rel.to_path_buf(), meta.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

fn prune(dir: &Path, limit: u64) {
    let mut list: Vec<_> = files(dir)
        .into_iter()
        .map(|(rel, size)| {
            let path = dir.join(rel);
            let time = fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (time, path, size)
        })
        .collect();
    let mut total: u64 = list.iter().map(|x| x.2).sum();
    if total <= limit {
        return;
    }
    list.sort_by_key(|x| x.0);
    for (_, path, size) in list {
        if total <= limit {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

fn account_file(user: &str) -> AppResult<PathBuf> {
    if user.is_empty() || user.len() > 32 || !user.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(AppError::Other("用户 ID 无效".into()));
    }
    data_file("accounts", user)
        .map(|p| p.join("state.json"))
        .ok_or_else(|| AppError::Other("数据目录不可用".into()))
}

fn read_account(path: &Path) -> Map<String, Value> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// # Errors
///
/// 用户 ID 无效或数据根不可用时返回错误。
pub fn account_get(user: &str) -> AppResult<Value> {
    let path = account_file(user)?;
    let mut m = read_account(&path);
    m.remove("v");
    Ok(Value::Object(m))
}

/// # Errors
///
/// 字段不允许、内容过大、数据根不可用或写入失败时返回错误。
pub fn account_set(user: &str, key: &str, value: Value) -> AppResult<()> {
    if !ACCOUNT_KEYS.contains(&key) {
        return Err(AppError::Other("不支持的状态字段".into()));
    }
    let path = account_file(user)?;
    let _l = ACCOUNT
        .lock()
        .map_err(|_| AppError::Other("存储锁失效".into()))?;
    let mut m = read_account(&path);
    m.insert("v".into(), VERSION.into());
    m.insert(key.into(), value);
    let bytes = serde_json::to_vec(&m)?;
    if bytes.len() > ACCOUNT_LIMIT {
        return Err(AppError::Other("账号状态过大".into()));
    }
    write(&path, &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cdapp-storage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::canonicalize(d).unwrap()
    }

    #[test]
    fn default_root_is_created_and_legacy_files_move() {
        let base = temp("default");
        let fixed = base.join("fixed");
        let legacy = base.join("legacy");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("audio.json"), b"{}").unwrap();
        fs::write(legacy.join("saying.json"), b"[]").unwrap();
        fs::write(legacy.join(CREDENTIALS), b"c").unwrap();
        let s = open(&fixed, &base.join("data"), &legacy);
        assert_eq!(s.status, Status::Ready);
        assert!(s.root.join(MARKER).is_file());
        assert!(s.root.join("config/audio.json").is_file());
        assert!(s.root.join("cache/saying.json").is_file());
        assert!(fixed.join(CREDENTIALS).is_file());
        assert!(!legacy.join("audio.json").exists());
        assert!(!legacy.join(CREDENTIALS).exists());
        drop(s);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn missing_custom_root_is_not_created() {
        let base = temp("missing");
        let fixed = base.join("fixed");
        let gone = base.join("gone");
        write_location(&fixed, Some(&gone)).unwrap();
        let s = open(&fixed, &base.join("data"), &base);
        assert_eq!(s.status, Status::Missing);
        assert!(!gone.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn second_instance_is_locked() {
        let base = temp("lock");
        let a = open(&base.join("fixed"), &base.join("data"), &base);
        let b = open(&base.join("fixed"), &base.join("data"), &base);
        assert_eq!(a.status, Status::Ready);
        assert_eq!(b.status, Status::Locked);
        drop((a, b));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn copy_root_skips_lock_and_verifies() {
        let base = temp("copy");
        let from = base.join("from");
        fs::create_dir_all(from.join("cache/a")).unwrap();
        fs::write(from.join("cache/a/x"), b"123").unwrap();
        fs::write(from.join(LOCK), b"").unwrap();
        fs::write(from.join(CREDENTIALS), b"c").unwrap();
        fs::write(from.join(LOCATION), b"{}").unwrap();
        write_marker(&from).unwrap();
        let to = base.join("to");
        copy_root(&from, &to).unwrap();
        assert_eq!(fs::read(to.join("cache/a/x")).unwrap(), b"123");
        assert!(to.join(MARKER).is_file());
        assert!(!to.join(LOCK).exists());
        assert!(!to.join(CREDENTIALS).exists());
        assert!(!to.join(LOCATION).exists());
        remove_root(&from).unwrap();
        assert!(from.join(CREDENTIALS).is_file());
        assert!(!from.join("cache").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn target_must_not_nest() {
        let base = temp("nest");
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        assert!(checked_target(&root, &root).is_err());
        assert!(checked_target(&root, &root.join("sub")).is_err());
        assert!(checked_target(&root, &base).is_err());
        assert!(checked_target(&root, Path::new("rel")).is_err());
        assert_eq!(
            checked_target(&root, &base.join("new")).unwrap(),
            base.join("new")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn prune_removes_oldest_first() {
        let base = temp("prune");
        fs::write(base.join("old"), [0u8; 10]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(base.join("new"), [0u8; 10]).unwrap();
        prune(&base, 15);
        assert!(!base.join("old").exists());
        assert!(base.join("new").exists());
        fs::remove_dir_all(base).unwrap();
    }
}
