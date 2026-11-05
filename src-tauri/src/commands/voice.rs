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
        deafen: Deafen::with_mic_before(deafened, mic_before_deaf),
        output: output::Sink::Device(slot.output.clone()),
        volumes: slot.volumes.clone(),
        master: slot.master.unwrap_or(100),
        password: password.filter(|p| !p.is_empty()),
        ..MicPrefs::default()
    };
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
    if let Some(s) = slot.session.as_ref().filter(|s| !s.is_finished()) {
        s.control().set_master(percent);
    }
    Ok(())
}
