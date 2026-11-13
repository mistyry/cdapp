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
mod moderation;
mod session;
mod share;
mod sound;
mod storage;
mod upload;
mod voice;

pub use dm::*;
pub use gateway::*;
pub use guild::*;
pub use message::*;
pub use moderation::*;
pub use session::*;
pub use share::*;
pub use sound::*;
pub use storage::*;
pub use upload::*;
pub use voice::*;

#[derive(Default)]
pub struct AppState {
    pub token: Option<String>,
    pub session_id: Option<String>,
    pub gateway_stop: Option<watch::Sender<bool>>,
    pub gateway_wake: Option<std::sync::Arc<tokio::sync::Notify>>,
    pub gateway_gen: u64,
    pub uploads: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Notify>>,
    pub gate: crate::kook::gate::Gate,
}

#[derive(Default)]
pub struct VoiceSlot {
    pub session: Option<VoiceSession>,
    pub generation: u64,
    pub latest: u64,
    pub input: Option<String>,
    pub output: Option<String>,
    pub gain: crate::audio::dsp::Gain,
    pub denoise: crate::audio::denoise::Denoise,
    pub volumes: std::collections::HashMap<String, u16>,
    pub denoised: std::collections::HashSet<String>,
    pub master: Option<u16>,
    pub effect_volume: u16,
    pub effects_off: Vec<u8>,
    pub muted: bool,
    pub deafened: bool,
    pub test: Option<crate::audio::loopback::Loopback>,
}

impl VoiceSlot {
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
        slot.denoise.set(p.denoise);
        slot.denoise.set_mode(p.denoise_mode);
        slot
    }

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
            denoise: self.denoise.on(),
            denoise_mode: self.denoise.mode(),
        }
    }

    pub fn persist(&self) {
        crate::audio::store::save_later(self.prefs());
    }
}

impl VoiceSlot {
    pub fn admit_join(&mut self, request: u64) -> bool {
        if request <= self.latest {
            return false;
        }
        self.latest = request;
        true
    }

    pub fn reset_account(&mut self) {
        self.volumes.clear();
        self.denoised.clear();
    }

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

fn reset(s: &mut AppState) {
    if let Some(stop) = s.gateway_stop.take() {
        let _ = stop.send(true);
    }
    *s = AppState {
        gateway_gen: s.gateway_gen,
        ..AppState::default()
    };
}

fn expire(state: &Mutex<AppState>, token: &str) -> bool {
    let Ok(mut s) = state.lock() else {
        return false;
    };
    if token.is_empty() || s.token.as_deref() != Some(token) {
        return false;
    }
    reset(&mut s);
    if let Err(e) = auth::clear_token() {
        log::warn!("清除凭据失败：{e}");
    }
    true
}

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

fn note(state: &Mutex<AppState>, token: &str, f: impl FnOnce(&mut crate::kook::gate::Gate)) {
    if let Ok(mut s) = state.lock()
        && !token.is_empty()
        && s.token.as_deref() == Some(token)
    {
        f(&mut s.gate);
    }
}

fn token_of(state: &Mutex<AppState>) -> AppResult<String> {
    lock(state)?.token.clone().ok_or(AppError::NotLoggedIn)
}

fn check(
    state: &Mutex<AppState>,
    channel_id: &str,
    need: crate::kook::gate::Need,
) -> AppResult<()> {
    lock(state)?.gate.check(channel_id, need)
}

async fn ensure_basis(state: &Mutex<AppState>, api: &Api, channel_id: &str) -> AppResult<()> {
    use crate::kook::gate::Missing;
    let token = api.token().unwrap_or_default().to_owned();
    let missing = lock(state)?.gate.missing(channel_id);
    let guild = match missing {
        None => return Ok(()),
        Some(Missing::Access(g)) => g,
        Some(Missing::Channel) => {
            let g = api.channel_guild(channel_id).await?;
            let ticket = lock(state)?.gate.ticket();
            let list = api.channels(&g).await?;
            note(state, &token, |gate| gate.set_channels(&g, &list, ticket));
            if lock(state)?.gate.missing(channel_id) != Some(Missing::Access(g.clone())) {
                return Ok(());
            }
            g
        }
    };
    log::info!("加入前补取服务器权限依据");
    let ticket = lock(state)?.gate.ticket();
    let access = api.guild_access(&guild).await?;
    note(state, &token, |gate| {
        gate.set_access(&guild, access, ticket);
    });
    Ok(())
}

async fn check_loaded(
    state: &Mutex<AppState>,
    api: &Api,
    channel_id: &str,
    need: crate::kook::gate::Need,
) -> AppResult<()> {
    if let Err(e) = ensure_basis(state, api, channel_id).await {
        log::warn!("补取频道权限依据失败：{e}");
    }
    check(state, channel_id, need)
}

fn stop_voice(app: &AppHandle) {
    use tauri::Manager;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        stop_share(&app).await;
        let old = app.state::<VoiceState>().0.lock().await.session.take();
        if let Some(v) = old {
            v.leave().await;
        }
    });
}

pub(crate) async fn stop_share(app: &AppHandle) {
    #[cfg(windows)]
    {
        use tauri::Manager;
        let old = app.state::<WatchState>().0.lock().await.session.take();
        if let Some(w) = old {
            w.stop().await;
        }
        let old = app.state::<ShareState>().0.lock().await.session.take();
        if let Some(s) = old {
            s.stop().await;
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(test)]
mod tests {
    use super::{AppState, VoiceSlot, reset};

    #[test]
    fn reset_stops_gateway_and_clears_token() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut s = AppState {
            token: Some("t".into()),
            session_id: Some("s".into()),
            gateway_stop: Some(tx),
            gateway_gen: 7,
            ..AppState::default()
        };
        reset(&mut s);
        assert!(*rx.borrow());
        assert!(s.token.is_none());
        assert!(s.session_id.is_none());
        assert!(s.gateway_stop.is_none());
        assert_eq!(s.gateway_gen, 7);
    }

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
        slot.note_leave(Some(2));
        assert!(!slot.admit_join(1));
        assert!(slot.admit_join(4));
        assert!(!slot.admit_join(3));
        slot.note_leave(None);
        assert!(slot.admit_join(5));
    }
}
