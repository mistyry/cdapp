use std::sync::Mutex;

use tauri::{AppHandle, State};
use tokio::sync::watch;

use crate::error::{AppError, AppResult};
use crate::kook::api::Api;
use crate::kook::auth;
use crate::kook::voice::VoiceSession;

mod dm;
mod gateway;
mod guild;
mod message;
mod session;
mod sound;
mod storage;
mod upload;
mod voice;

pub use dm::*;
pub use gateway::*;
pub use guild::*;
pub use message::*;
pub use session::*;
pub use sound::*;
pub use storage::*;
pub use upload::*;
pub use voice::*;

#[derive(Default)]
pub struct AppState {
    pub token: Option<String>,
    pub session_id: Option<String>,
    pub gateway_stop: Option<watch::Sender<bool>>,
    /// 唤醒处于退避等待的网关，立即重连。
    pub gateway_wake: Option<std::sync::Arc<tokio::sync::Notify>>,
    /// 当前网关代次。每次启动递增，旧网关的迟到事件按代次丢弃，同一凭据重复启动也能区分。
    pub gateway_gen: u64,
    /// 进行中的上传，按前端上传 ID 取消。
    pub uploads: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Notify>>,
    /// 命令边界的权限依据，退出登录时随状态清空。
    pub gate: crate::kook::gate::Gate,
}

/// 当前语音会话与代次。切换时需跨 await 等待旧连接释放，因此单独使用 tokio 锁。
/// 代次每次加入递增，随事件发给前端，旧会话的迟到事件据此丢弃。
#[derive(Default)]
pub struct VoiceSlot {
    pub session: Option<VoiceSession>,
    pub generation: u64,
    /// 已处理的最新前端请求序号。请求可能乱序拿到锁，序号更小的加入已被后来的加入或退出取代。
    pub latest: u64,
    /// 输入设备 ID，None 为系统默认。本机偏好，保存在 audio.json。
    pub input: Option<String>,
    /// 输出设备 ID，None 为系统默认。本机偏好，保存在 audio.json。
    pub output: Option<String>,
    /// 输入增益，与当前会话共享，修改立即生效。
    pub gain: crate::audio::dsp::Gain,
    /// 成员音量百分比，按用户 ID 记录。跨会话保留，只保存在内存中。
    pub volumes: std::collections::HashMap<String, u16>,
    /// 总音量百分比，None 为 100。
    pub master: Option<u16>,
    /// 提示音音量百分比。
    pub effect_volume: u16,
    /// 关闭的提示音编号。
    pub effects_off: Vec<u8>,
    /// 麦克风与耳机开关，下次启动沿用。
    pub muted: bool,
    pub deafened: bool,
    /// 进行中的麦克风测试。
    pub test: Option<crate::audio::loopback::Loopback>,
}

impl VoiceSlot {
    /// 按保存的本机偏好初始化。
    #[must_use]
    pub fn from_prefs(p: &crate::audio::store::AudioPrefs) -> Self {
        let slot = Self {
            input: p.input.clone(),
            output: p.output.clone(),
            master: Some(p.master),
            effect_volume: p.effect_volume,
            effects_off: p.effects_off.clone(),
            muted: p.muted,
            deafened: p.deafened,
            ..Self::default()
        };
        slot.gain.set(p.gain);
        slot
    }

    /// 当前本机偏好。
    #[must_use]
    pub fn prefs(&self) -> crate::audio::store::AudioPrefs {
        crate::audio::store::AudioPrefs {
            input: self.input.clone(),
            output: self.output.clone(),
            gain: self.gain.percent(),
            master: self.master.unwrap_or(100),
            effect_volume: self.effect_volume,
            effects_off: self.effects_off.clone(),
            muted: self.muted,
            deafened: self.deafened,
        }
    }

    /// 在后台保存本机偏好，不在持锁期间写盘。失败只记录日志，不影响当前设置生效。
    pub fn persist(&self) {
        crate::audio::store::save_later(self.prefs());
    }
}

impl VoiceSlot {
    /// 登记加入请求。已被更新的请求取代时返回 false，不再启动会话。
    pub fn admit_join(&mut self, request: u64) -> bool {
        if request <= self.latest {
            return false;
        }
        self.latest = request;
        true
    }

    /// 退出登录时清除账号相关的成员音量。
    /// 设备、输入增益与总音量属于本机设置，保留不变。
    pub fn reset_account(&mut self) {
        self.volumes.clear();
    }

    /// 登记退出请求。序号更小且尚未拿到锁的加入随之作废。
    pub fn note_leave(&mut self, request: Option<u64>) {
        if let Some(r) = request {
            self.latest = self.latest.max(r);
        }
    }
}

#[derive(Default)]
pub struct VoiceState(pub tokio::sync::Mutex<VoiceSlot>);

fn api(state: &State<'_, Mutex<AppState>>) -> AppResult<Api> {
    let token = state
        .lock()
        .map_err(|_| AppError::Other("状态锁损坏".into()))?
        .token
        .clone();
    let token = token.ok_or(AppError::NotLoggedIn)?;
    Api::new(Some(token))
}

fn lock(state: &Mutex<AppState>) -> AppResult<std::sync::MutexGuard<'_, AppState>> {
    state
        .lock()
        .map_err(|_| AppError::Other("状态锁损坏".into()))
}

/// 凭据被服务端确认失效：仅当失效的仍是当前凭据时清除登录状态与凭据文件，旧账号的迟到响应不影响新登录。
/// 返回是否确实结束了当前登录。凭据文件删除在状态锁内进行，不会删掉并发登录刚保存的新凭据。
fn expire(state: &Mutex<AppState>, token: &str) -> bool {
    let Ok(mut s) = state.lock() else {
        return false;
    };
    if token.is_empty() || s.token.as_deref() != Some(token) {
        return false;
    }
    if let Some(stop) = s.gateway_stop.take() {
        let _ = stop.send(true);
    }
    *s = AppState {
        gateway_gen: s.gateway_gen,
        ..AppState::default()
    };
    if let Err(e) = auth::clear_token() {
        log::warn!("清除凭据失败：{e}");
    }
    true
}

/// 以当前凭据执行请求，遇到凭据失效时结束登录状态。
async fn with_api<T, F, Fut>(state: &State<'_, Mutex<AppState>>, f: F) -> AppResult<T>
where
    F: FnOnce(Api) -> Fut,
    Fut: Future<Output = AppResult<T>>,
{
    let api = api(state)?;
    let token = api.token().unwrap_or_default().to_owned();
    let r = f(api).await;
    if matches!(r, Err(AppError::Unauthorized(_))) {
        expire(state.inner(), &token);
    }
    r
}

/// 记录权限依据。仅当请求所用凭据仍是当前凭据时写入，旧账号的迟到结果不进入新登录。
fn note(state: &Mutex<AppState>, token: &str, f: impl FnOnce(&mut crate::kook::gate::Gate)) {
    if let Ok(mut s) = state.lock()
        && !token.is_empty()
        && s.token.as_deref() == Some(token)
    {
        f(&mut s.gate);
    }
}

/// 当前凭据。
fn token_of(state: &Mutex<AppState>) -> AppResult<String> {
    lock(state)?.token.clone().ok_or(AppError::NotLoggedIn)
}

/// 按门禁判断频道操作，未通过时请求不发出。
fn check(
    state: &Mutex<AppState>,
    channel_id: &str,
    need: crate::kook::gate::Need,
) -> AppResult<()> {
    lock(state)?.gate.check(channel_id, need)
}

/// 凭据失效后在后台退出语音。
fn stop_voice(app: &AppHandle) {
    use tauri::Manager;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let old = app.state::<VoiceState>().0.lock().await.session.take();
        if let Some(v) = old {
            v.leave().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::VoiceSlot;

    #[test]
    fn logout_resets_account_volumes() {
        let mut slot = VoiceSlot::default();
        slot.volumes.insert("u1".into(), 0);
        slot.master = Some(30);
        slot.output = Some("dev".into());
        slot.reset_account();
        assert!(slot.volumes.is_empty());
        assert_eq!(slot.master, Some(30));
        assert_eq!(slot.output.as_deref(), Some("dev"));
    }

    #[test]
    fn stale_voice_join_is_rejected_after_newer_leave_or_join() {
        let mut slot = VoiceSlot::default();
        // 较早的加入在更晚的退出之后才拿到锁。
        slot.note_leave(Some(2));
        assert!(!slot.admit_join(1));
        // 较早的加入在更晚的加入之后才拿到锁。
        assert!(slot.admit_join(4));
        assert!(!slot.admit_join(3));
        // 无序号的退出不影响后续加入。
        slot.note_leave(None);
        assert!(slot.admit_join(5));
    }
}
