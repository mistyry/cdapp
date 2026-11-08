//! 提示音命令。前端按场景请求播放，开关与音量在此判断。

use tauri::State;

use super::VoiceState;
use crate::audio::effect;
use crate::error::{AppError, AppResult};
use crate::kook::sound;

/// 提示音编号与文件，编号与官方 `audio_setting` 一致。
const EFFECTS: &[(u8, &str)] = &[
    (1, "user-join.mp3"),
    (2, "user-leave.mp3"),
    (3, "exit-channel.mp3"),
    (4, "mkf-mute.mp3"),
    (5, "mkf-cancel-mute.mp3"),
    (8, "head-mute.mp3"),
    (9, "cancel-head-mute.mp3"),
    (10, "mention.mp3"),
    (13, "mention-self.mp3"),
];

const USER_JOIN: u8 = 1;
const MENTION: u8 = 10;
const MENTION_SELF: u8 = 13;

fn file_of(id: u8) -> Option<&'static str> {
    EFFECTS.iter().find(|x| x.0 == id).map(|x| x.1)
}

/// 按开关确定实际播放的编号。提及我的消息关闭时改用普通消息通知。
#[must_use]
pub fn resolve(id: u8, off: &[u8]) -> Option<u8> {
    if id == MENTION_SELF && off.contains(&MENTION_SELF) {
        return resolve(MENTION, off);
    }
    (file_of(id).is_some() && !off.contains(&id)).then_some(id)
}

/// 播放提示音。`preview` 为真时忽略开关，用于设置页试听。
#[tauri::command]
pub async fn effect_play(
    id: u8,
    preview: Option<bool>,
    voice: State<'_, VoiceState>,
) -> AppResult<()> {
    let (device, volume, off) = {
        let slot = voice.0.lock().await;
        (
            slot.output.clone(),
            slot.effect_volume,
            slot.effects_off.clone(),
        )
    };
    let id = if preview.unwrap_or(false) {
        Some(id).filter(|&i| file_of(i).is_some())
    } else {
        resolve(id, &off)
    };
    let Some(path) = id.and_then(file_of).and_then(sound::system_file) else {
        return Ok(());
    };
    if !path.is_file() {
        return Ok(());
    }
    effect::play(&path, device, f32::from(volume) / 100.0);
    Ok(())
}

/// 播放成员的进场音效，失败时播放用户连接提示音。受用户连接开关控制。
#[tauri::command]
pub async fn effect_play_item(item: u64, voice: State<'_, VoiceState>) -> AppResult<()> {
    let (device, volume, off) = {
        let slot = voice.0.lock().await;
        (
            slot.output.clone(),
            slot.effect_volume,
            slot.effects_off.clone(),
        )
    };
    if resolve(USER_JOIN, &off).is_none() {
        return Ok(());
    }
    let api = crate::kook::api::Api::new(None)?;
    let path = match sound::item_sound(&api, item).await {
        Some(p) => Some(p),
        None => file_of(USER_JOIN).and_then(sound::system_file),
    };
    if let Some(path) = path.filter(|p| p.is_file()) {
        effect::play(&path, device, f32::from(volume) / 100.0);
    }
    Ok(())
}

/// 设置提示音音量，上限 100。
#[tauri::command]
pub async fn set_effect_volume(percent: u16, voice: State<'_, VoiceState>) -> AppResult<()> {
    let mut slot = voice.0.lock().await;
    slot.effect_volume = percent.min(100);
    slot.persist();
    Ok(())
}

/// 开启或关闭一项提示音。
#[tauri::command]
pub async fn set_effect_enabled(id: u8, on: bool, voice: State<'_, VoiceState>) -> AppResult<()> {
    if file_of(id).is_none() {
        return Err(AppError::Other("未知的提示音".into()));
    }
    let mut slot = voice.0.lock().await;
    slot.effects_off.retain(|&x| x != id);
    if !on {
        slot.effects_off.push(id);
        slot.effects_off.sort_unstable();
    }
    slot.persist();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_self_falls_back() {
        assert_eq!(resolve(13, &[]), Some(13));
        assert_eq!(resolve(13, &[13]), Some(10));
        assert_eq!(resolve(13, &[10, 13]), None);
        assert_eq!(resolve(4, &[4]), None);
        assert_eq!(resolve(11, &[]), None);
    }
}
