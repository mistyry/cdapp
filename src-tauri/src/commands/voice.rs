use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::{AppState, VoiceState, api, check, expire, lock};
use crate::audio::capture::{InputDevice, Source};
use crate::audio::output::{self, Deafen, OutputDevice};
use crate::error::{AppError, AppResult};
use crate::kook::gate::Need;
use crate::kook::voice::{EndReason, MicPrefs, Sink, VoiceEvent, VoiceSession};

/// 通知当前语音会话系统已唤醒或网络已恢复。
#[tauri::command]
pub async fn voice_wake(voice: State<'_, VoiceState>) -> AppResult<()> {
    if let Some(s) = voice.0.lock().await.session.as_ref() {
        s.control().wake();
    }
    Ok(())
}

/// 语音事件，`generation` 为会话代次。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoicePayload {
    generation: u64,
    channel_id: String,
    event: VoiceEvent,
}

/// 加入时的麦克风与耳机状态。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinAudio {
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    deafened: bool,
    /// 关闭耳机前麦克风是否静音。
    mic_before_deaf: Option<bool>,
}

/// 加入语音频道，返回会话代次。先完全释放旧会话，再在后台加入。
/// 进度、成员与结束原因以 `voice://event` 广播。
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
    // 服务端不校验查看与连接权限，须在申请令牌前拦截。
    check(&state, &channel_id, Need::Connect)?;
    let token = api.token().unwrap_or_default().to_owned();
    let mut slot = voice.0.lock().await;
    // 前端按序号发出请求，但拿锁顺序不定。已被后发的加入或退出取代时不再加入。
    if !slot.admit_join(request) {
        return Err(AppError::Cancelled);
    }
    if let Some(old) = slot.session.take() {
        old.leave().await;
    }
    // 加入前结束麦克风测试，释放设备。
    if let Some(t) = slot.test.take() {
        let _ = tauri::async_runtime::spawn_blocking(move || drop(t)).await;
    }
    // 等锁与释放期间可能已退出登录。
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
        // 凭据失效时与其他命令一致，结束当前登录。旧账号的迟到结果不影响新登录。
        if unauthorized && expire(&app.state::<Mutex<AppState>>(), &token) {
            let _ = app.emit("session://expired", message);
        }
    });
    let JoinAudio {
        muted,
        deafened,
        mic_before_deaf,
    } = audio.unwrap_or_default();
    // 启用测试特性时 MicPrefs 另有测试字段。
    #[allow(clippy::needless_update)]
    let mic = MicPrefs {
        // 闭听加入时麦克风也静音。
        muted: muted || deafened,
        source: mic_source(slot.input.clone()),
        gain: slot.gain.clone(),
        denoise: slot.denoise.clone(),
        deafen: Deafen::with_mic_before(deafened, mic_before_deaf),
        output: output::Sink::Device(slot.output.clone()),
        volumes: slot.volumes.clone(),
        master: slot.master.unwrap_or(100),
        password: password.filter(|p| !p.is_empty()),
        ..MicPrefs::default()
    };
    // 降噪在启动时已开启而尚无许可，在后台获取，取得后采集线程自动启用。
    if slot.denoise.on() && slot.denoise.mode() == crate::audio::denoise::Mode::Kook {
        spawn_license(api.clone());
    }
    slot.session = Some(VoiceSession::start_with(
        api, channel_id, generation, sink, mic,
    ));
    Ok(generation)
}

/// 退出语音，等待旧连接释放。`request` 为前端请求序号，序号更小的在途加入随之作废。
/// 退出登录等无序号的调用只释放当前会话。
#[tauri::command]
pub async fn voice_leave(request: Option<u64>, voice: State<'_, VoiceState>) -> AppResult<()> {
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

/// 采集来源。启用 `test-media` 特性时，设置 `CDAPP_MIC_TONE=<频率>` 可用合成音代替麦克风。
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

/// 列出输入设备。只读取设备列表，不打开设备，不触发系统权限请求。
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
    /// 当前选择，None 为系统默认。
    selected: Option<String>,
}

/// 选择输入设备，None 为系统默认。正在采集时立即切换。
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

/// 设置麦克风静音。取消静音也用于失败后的重试。
/// `generation` 须为当前会话代次，避免操作到切换后的新会话。
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

/// 当前会话的控制句柄。代次不符或会话已结束时返回错误。
async fn control(
    voice: &State<'_, VoiceState>,
    generation: u64,
) -> AppResult<crate::kook::voice::VoiceControl> {
    match &voice.0.lock().await.session {
        Some(s) if s.generation == generation && !s.is_finished() => Ok(s.control()),
        _ => Err(AppError::Other("语音已断开".into())),
    }
}

/// 设置耳机关闭。关闭耳机时同时静音麦克风。
#[tauri::command]
pub async fn voice_set_deafen(
    generation: u64,
    deafened: bool,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    control(&voice, generation).await?.deafen(deafened).await
}

/// 列出输出设备，不打开设备。
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

/// 选择输出设备，None 为系统默认。正在播放时立即切换。
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

/// 设置成员音量，上限 200%。100% 不记录。
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

/// 设置总音量，上限 200%。
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

/// 设置麦克风输入增益，上限 200%，100% 为原始音量。正在采集时立即生效。
/// 返回实际生效的值。
#[tauri::command]
pub async fn voice_set_gain(percent: u16, voice: State<'_, VoiceState>) -> AppResult<u16> {
    let slot = voice.0.lock().await;
    slot.gain.set(percent);
    slot.persist();
    Ok(slot.gain.percent())
}

/// 在后台获取 KOOK AI 降噪许可。已有许可时不请求。
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

/// 降噪开启且使用 KOOK AI 降噪 2.0 时，在后台获取许可。未登录时跳过。
async fn need_license(state: &State<'_, Mutex<AppState>>, voice: &State<'_, VoiceState>) {
    let d = voice.0.lock().await.denoise.clone();
    if d.on()
        && d.mode() == crate::audio::denoise::Mode::Kook
        && let Ok(a) = api(state)
    {
        spawn_license(a);
    }
}

/// 降噪因故障关闭时保存关闭状态，并以 `denoise://off` 通知界面，内容为原因。
pub fn watch_denoise(app: &AppHandle, denoise: &crate::audio::denoise::Denoise) {
    let app = app.clone();
    denoise.on_fail(Box::new(move |reason| {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            use tauri::Manager;
            app.state::<VoiceState>().0.lock().await.persist();
            if let Err(e) = app.emit("denoise://off", reason) {
                log::warn!("事件广播失败：{e}");
            }
        });
    }));
}

/// 开启或关闭降噪。`mode` 为空时沿用当前方式。
/// 使用 KOOK AI 降噪 2.0 时先获取许可，正在采集时立即生效。
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
    slot.denoise.set_mode(mode);
    slot.denoise.set(on);
    slot.persist();
    Ok(())
}

/// 本机音频偏好，启动时读取一次。
#[tauri::command]
pub async fn audio_prefs(
    voice: State<'_, VoiceState>,
) -> AppResult<crate::audio::store::AudioPrefs> {
    Ok(voice.0.lock().await.prefs())
}

/// 保存麦克风与耳机开关，下次启动沿用。
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

/// 开始或停止麦克风测试。开始时把将要推送的音频按总音量播放到所选扬声器，电平经 `mic-test://event` 上报。
/// 在语音频道中时不允许开始，避免两路采集同时占用麦克风。
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
    // 停止时等待线程释放设备，不阻塞异步运行时。
    if let Some(t) = old {
        let _ = tauri::async_runtime::spawn_blocking(move || drop(t)).await;
    }
    Ok(())
}
