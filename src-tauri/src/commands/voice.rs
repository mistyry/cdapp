use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::{AppState, VoiceState, api, check_loaded, expire, lock};
use crate::audio::capture::{InputDevice, Source};
use crate::audio::output::{self, Deafen, OutputDevice};
use crate::error::{AppError, AppResult};
use crate::kook::gate::Need;
use crate::kook::voice::{EndReason, MicPrefs, Sink, VoiceEvent, VoiceSession};

#[tauri::command]
pub async fn golive_allowed(state: State<'_, Mutex<AppState>>) -> AppResult<bool> {
    api(&state)?.golive_owned().await
}

#[tauri::command]
pub async fn voice_wake(voice: State<'_, VoiceState>) -> AppResult<()> {
    if let Some(s) = voice.0.lock().await.session.as_ref() {
        s.control().wake();
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoicePayload {
    generation: u64,
    channel_id: String,
    event: VoiceEvent,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinAudio {
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    deafened: bool,
    mic_before_deaf: Option<bool>,
}

#[tauri::command]
pub async fn voice_join(
    channel_id: String,
    request: u64,
    audio: Option<JoinAudio>,
    password: Option<String>,
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    voice: State<'_, VoiceState>,
) -> AppResult<u64> {
    let api = api(&state)?;
    check_loaded(&state, &api, &channel_id, Need::Connect).await?;
    let token = api.token().unwrap_or_default().to_owned();
    super::stop_share(&app).await;
    let mut slot = voice.0.lock().await;
    if !slot.admit_join(request) {
        return Err(AppError::Cancelled);
    }
    if let Some(old) = slot.session.take() {
        old.leave().await;
    }
    if let Some(t) = slot.test.take() {
        let _ = tauri::async_runtime::spawn_blocking(move || drop(t)).await;
    }
    if lock(&state)?.token.as_deref() != Some(token.as_str()) {
        return Err(AppError::NotLoggedIn);
    }
    slot.generation += 1;
    let generation = slot.generation;
    let channel = channel_id.clone();
    let sink: Sink = Arc::new(move |event: VoiceEvent| {
        use tauri::Manager;
        let unauthorized = matches!(
            event,
            VoiceEvent::State {
                reason: Some(EndReason::Unauthorized),
                ..
            }
        );
        let message = match &event {
            VoiceEvent::State { message, .. } => message.clone().unwrap_or_default(),
            _ => String::new(),
        };
        let payload = VoicePayload {
            generation,
            channel_id: channel.clone(),
            event,
        };
        if let Err(e) = app.emit("voice://event", payload) {
            log::warn!("事件广播失败：{e}");
        }
        if unauthorized && expire(&app.state::<Mutex<AppState>>(), &token) {
            let _ = app.emit("session://expired", message);
        }
    });
    let JoinAudio {
        muted,
        deafened,
        mic_before_deaf,
    } = audio.unwrap_or_default();
    #[allow(clippy::needless_update)]
    let mic = MicPrefs {
        muted: muted || deafened,
        source: mic_source(slot.input.clone()),
        gain: slot.gain.clone(),
        denoise: slot.denoise.clone(),
        deafen: Deafen::with_mic_before(deafened, mic_before_deaf),
        output: output::Sink::Device(slot.output.clone()),
        volumes: slot.volumes.clone(),
        denoised: slot.denoised.clone(),
        master: slot.master.unwrap_or(100),
        password: password.filter(|p| !p.is_empty()),
        ..MicPrefs::default()
    };
    crate::audio::denoise::retry_kook();
    let peers_denoised = !slot.denoised.is_empty();
    if (slot.denoise.on() || peers_denoised)
        && slot.denoise.mode() == crate::audio::denoise::Mode::Kook
    {
        spawn_license(api.clone());
    }
    slot.session = Some(VoiceSession::start_with(
        api, channel_id, generation, sink, mic,
    ));
    Ok(generation)
}

#[tauri::command]
pub async fn voice_leave(
    request: Option<u64>,
    app: AppHandle,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    super::stop_share(&app).await;
    let old = {
        let mut slot = voice.0.lock().await;
        slot.note_leave(request);
        slot.session.take()
    };
    if let Some(s) = old {
        s.leave().await;
    }
    Ok(())
}

fn mic_source(input: Option<String>) -> Source {
    #[cfg(feature = "test-media")]
    if let Some(f) = std::env::var("CDAPP_MIC_TONE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|f| *f > 0.0 && *f < 20_000.0)
    {
        return Source::Tone(f);
    }
    Source::Device(input)
}

#[tauri::command]
pub async fn audio_inputs(voice: State<'_, VoiceState>) -> AppResult<AudioInputs> {
    let selected = voice.0.lock().await.input.clone();
    let devices = tauri::async_runtime::spawn_blocking(crate::audio::capture::input_devices)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
        .map_err(|e| AppError::Other(format!("无法读取音频设备：{e}")))?;
    Ok(AudioInputs { devices, selected })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputs {
    devices: Vec<InputDevice>,
    selected: Option<String>,
}

#[tauri::command]
pub async fn set_audio_input(id: Option<String>, voice: State<'_, VoiceState>) -> AppResult<()> {
    let mut slot = voice.0.lock().await;
    slot.input = id.filter(|s| !s.is_empty());
    slot.persist();
    if let Some(s) = slot.session.as_ref().filter(|s| !s.is_finished()) {
        s.control().set_source(mic_source(slot.input.clone()));
    }
    Ok(())
}

#[tauri::command]
pub async fn voice_set_mic(
    generation: u64,
    paused: bool,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let control = match &voice.0.lock().await.session {
        Some(s) if s.generation == generation && !s.is_finished() => s.control(),
        _ => return Err(AppError::Other("语音已断开".into())),
    };
    control.pause(paused).await
}

async fn control(
    voice: &State<'_, VoiceState>,
    generation: u64,
) -> AppResult<crate::kook::voice::VoiceControl> {
    match &voice.0.lock().await.session {
        Some(s) if s.generation == generation && !s.is_finished() => Ok(s.control()),
        _ => Err(AppError::Other("语音已断开".into())),
    }
}

#[tauri::command]
pub async fn voice_set_deafen(
    generation: u64,
    deafened: bool,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    control(&voice, generation).await?.deafen(deafened).await
}

#[tauri::command]
pub async fn audio_outputs(voice: State<'_, VoiceState>) -> AppResult<AudioOutputs> {
    let selected = voice.0.lock().await.output.clone();
    let devices = tauri::async_runtime::spawn_blocking(output::output_devices)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
        .map_err(|e| AppError::Other(format!("无法读取音频设备：{e}")))?;
    Ok(AudioOutputs { devices, selected })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOutputs {
    devices: Vec<OutputDevice>,
    selected: Option<String>,
}

#[tauri::command]
pub async fn set_audio_output(id: Option<String>, voice: State<'_, VoiceState>) -> AppResult<()> {
    let mut slot = voice.0.lock().await;
    slot.output = id.filter(|s| !s.is_empty());
    slot.persist();
    if let Some(s) = slot.session.as_ref().filter(|s| !s.is_finished()) {
        s.control()
            .set_output(output::Sink::Device(slot.output.clone()));
    }
    Ok(())
}

#[tauri::command]
pub async fn voice_set_volume(
    user_id: String,
    percent: u16,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let percent = percent.min(crate::audio::mixer::MAX_VOLUME);
    let mut slot = voice.0.lock().await;
    if percent == 100 {
        slot.volumes.remove(&user_id);
    } else {
        slot.volumes.insert(user_id.clone(), percent);
    }
    if let Some(s) = slot.session.as_ref().filter(|s| !s.is_finished()) {
        s.control().set_volume(user_id, percent);
    }
    Ok(())
}

#[tauri::command]
pub async fn voice_set_peer_denoise(
    user_id: String,
    on: bool,
    state: State<'_, Mutex<AppState>>,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let mut slot = voice.0.lock().await;
    if on {
        slot.denoised.insert(user_id.clone());
    } else {
        slot.denoised.remove(&user_id);
    }
    if let Some(s) = slot.session.as_ref().filter(|s| !s.is_finished()) {
        s.control().set_peer_denoise(user_id, on);
    }
    let kook = on && slot.denoise.mode() == crate::audio::denoise::Mode::Kook;
    drop(slot);
    if kook && let Ok(a) = api(&state) {
        spawn_license(a);
    }
    Ok(())
}

#[tauri::command]
pub async fn voice_set_master(percent: u16, voice: State<'_, VoiceState>) -> AppResult<()> {
    let percent = percent.min(crate::audio::mixer::MAX_VOLUME);
    let mut slot = voice.0.lock().await;
    slot.master = Some(percent);
    slot.persist();
    if let Some(t) = &slot.test {
        t.set_master(percent);
    }
    if let Some(s) = slot.session.as_ref().filter(|s| !s.is_finished()) {
        s.control().set_master(percent);
    }
    Ok(())
}

#[tauri::command]
pub async fn voice_set_gain(percent: u16, voice: State<'_, VoiceState>) -> AppResult<u16> {
    let slot = voice.0.lock().await;
    slot.gain.set(percent);
    slot.persist();
    Ok(slot.gain.percent())
}

fn spawn_license(api: crate::kook::api::Api) {
    if crate::audio::denoise::has_license() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::kook::denoise_asset::ensure_license(&api).await {
            log::warn!("降噪许可获取失败：{e}");
        }
    });
}

async fn need_license(state: &State<'_, Mutex<AppState>>, voice: &State<'_, VoiceState>) {
    let d = voice.0.lock().await.denoise.clone();
    if d.on()
        && d.mode() == crate::audio::denoise::Mode::Kook
        && let Ok(a) = api(state)
    {
        spawn_license(a);
    }
}

pub fn watch_denoise(app: &AppHandle, denoise: &crate::audio::denoise::Denoise) {
    let app = app.clone();
    denoise.on_fail(Box::new(move |reason| {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = app.emit("denoise://fallback", reason) {
                log::warn!("事件广播失败：{e}");
            }
        });
    }));
}

#[tauri::command]
pub async fn voice_set_denoise(
    on: bool,
    mode: Option<crate::audio::denoise::Mode>,
    state: State<'_, Mutex<AppState>>,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let mode = match mode {
        Some(m) => m,
        None => voice.0.lock().await.denoise.mode(),
    };
    if on && mode == crate::audio::denoise::Mode::Kook {
        crate::kook::denoise_asset::ensure_license(&api(&state)?).await?;
    }
    let slot = voice.0.lock().await;
    crate::audio::denoise::retry_kook();
    slot.denoise.set_mode(mode);
    slot.denoise.set(on);
    slot.persist();
    Ok(())
}

#[tauri::command]
pub async fn audio_prefs(
    voice: State<'_, VoiceState>,
) -> AppResult<crate::audio::store::AudioPrefs> {
    Ok(voice.0.lock().await.prefs())
}

#[tauri::command]
pub async fn set_voice_switches(
    muted: bool,
    deafened: bool,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let mut slot = voice.0.lock().await;
    if slot.muted != muted || slot.deafened != deafened {
        slot.muted = muted;
        slot.deafened = deafened;
        slot.persist();
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum MicTestEvent {
    Level { level: u8 },
    Failed { message: String },
}

#[tauri::command]
pub async fn mic_test(
    on: bool,
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    if on {
        need_license(&state, &voice).await;
    }
    let old = {
        let mut slot = voice.0.lock().await;
        let old = slot.test.take();
        if on {
            if slot.session.as_ref().is_some_and(|s| !s.is_finished()) {
                return Err(AppError::Other("语音通话中无法测试麦克风".into()));
            }
            let (a, b) = (app.clone(), app);
            slot.test = Some(crate::audio::loopback::Loopback::start(
                slot.input.clone(),
                slot.output.clone(),
                slot.gain.clone(),
                slot.denoise.clone(),
                slot.master.unwrap_or(100),
                move |level| {
                    let _ = a.emit("mic-test://event", MicTestEvent::Level { level });
                },
                move |message| {
                    let _ = b.emit("mic-test://event", MicTestEvent::Failed { message });
                },
            ));
        }
        old
    };
    if let Some(t) = old {
        let _ = tauri::async_runtime::spawn_blocking(move || drop(t)).await;
    }
    Ok(())
}
