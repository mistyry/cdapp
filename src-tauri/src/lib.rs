pub mod audio;
mod commands;
#[cfg(all(target_os = "macos", debug_assertions))]
mod devshot;
pub mod error;
pub mod kook;
#[cfg(windows)]
mod winfocus;

use std::sync::Mutex;

use commands::{AppState, VoiceState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 启动应用。
///
/// # Panics
///
/// 应用初始化失败时触发。
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use tauri::Manager;
            let dir = app.path().app_data_dir()?;
            kook::auth::init(&dir);
            audio::store::init(&dir);
            let slot = commands::VoiceSlot::from_prefs(&audio::store::load());
            app.manage(VoiceState(tokio::sync::Mutex::new(slot)));
            #[cfg(all(target_os = "macos", debug_assertions))]
            devshot::init(app.handle());
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                winfocus::init(&window);
            }
            Ok(())
        })
        .manage(Mutex::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::restore_session,
            commands::login_qr_code,
            commands::login_qr_poll,
            commands::login_qr_cancel,
            commands::logout,
            commands::me,
            commands::guilds,
            commands::guild_folders,
            commands::channels,
            commands::activate_guild,
            commands::guild_access,
            commands::emojis,
            commands::members,
            commands::subscribe_members,
            commands::messages,
            commands::messages_after,
            commands::locate_message,
            commands::send_message,
            commands::update_message,
            commands::delete_message,
            commands::reaction,
            commands::mark_read,
            commands::unread,
            commands::conversations,
            commands::dm_chats,
            commands::open_dm,
            commands::dm_messages,
            commands::mark_dm_read,
            commands::send_direct,
            commands::update_dm_message,
            commands::delete_dm_message,
            commands::dm_reaction,
            commands::upload,
            commands::upload_cancel,
            commands::gateway_start,
            commands::gateway_reconnect,
            commands::gateway_stop,
            commands::voice_join,
            commands::voice_leave,
            commands::voice_wake,
            commands::voice_set_mic,
            commands::audio_inputs,
            commands::set_audio_input,
            commands::voice_set_deafen,
            commands::audio_outputs,
            commands::set_audio_output,
            commands::voice_set_volume,
            commands::voice_set_master,
            commands::voice_set_gain,
            commands::audio_prefs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
