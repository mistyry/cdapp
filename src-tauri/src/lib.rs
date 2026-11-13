pub mod audio;
mod commands;
#[cfg(all(target_os = "macos", debug_assertions))]
mod devshot;
pub mod error;
pub mod kook;
#[cfg(windows)]
pub mod share;
pub mod storage;
mod window_state;
#[cfg(windows)]
mod winfocus;

use std::sync::Mutex;

use commands::{AppState, VoiceState};

fn data_dir_arg() -> Option<std::path::PathBuf> {
    let mut args = std::env::args().skip(1);
    let mut found = None;
    while let Some(a) = args.next() {
        if let Some(v) = a.strip_prefix("--data-dir=") {
            found = Some(v.to_owned());
        } else if a == "--data-dir" {
            found = args.next();
        }
    }
    let dir = found
        .or_else(|| std::env::var("CDAPP_DATA_DIR").ok())
        .filter(|s| !s.is_empty())?;
    let path = std::path::PathBuf::from(dir);
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir().ok().map(|c| c.join(path))
    }
}

fn log_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};
    let default = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let level = std::env::var("CDAPP_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    let mut targets = vec![Target::new(TargetKind::LogDir { file_name: None })];
    if cfg!(debug_assertions) {
        targets.push(Target::new(TargetKind::Stdout));
    }
    tauri_plugin_log::Builder::new()
        .targets(targets)
        .level(log::LevelFilter::Warn)
        .level_for("cdapp_lib", level)
        .level_for("app", level)
        .level_for("webrtc::peer_connection::driver", log::LevelFilter::Off)
        .level_for("rtc_mdns", log::LevelFilter::Off)
        .level_for("rtc_dtls::config", log::LevelFilter::Error)
        .max_file_size(2_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(5))
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn prevent_default<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_prevent_default::Flags;
    let builder = tauri_plugin_prevent_default::Builder::new()
        .with_flags(Flags::debug().difference(Flags::FOCUS_MOVE));
    #[cfg(windows)]
    let builder = builder.platform(
        tauri_plugin_prevent_default::PlatformOptions::new()
            .browser_accelerator_keys(cfg!(debug_assertions))
            .default_context_menus(cfg!(debug_assertions)),
    );
    builder.build()
}

/// # Panics
///
/// 应用初始化失败时触发。
#[allow(clippy::too_many_lines)]
pub fn run() {
    kook::emoji_cache::protocol(tauri::Builder::default())
        .plugin(log_plugin())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(prevent_default())
        .setup(|app| {
            use tauri::Manager;
            if let Some(dir) = data_dir_arg() {
                let fixed = dir.join("fixed");
                storage::init(&fixed, &dir.join("data"), &fixed);
            } else {
                let data = app.path().app_data_dir()?;
                storage::init(&app.path().app_config_dir()?, &data, &data);
            }
            let slot = commands::VoiceSlot::from_prefs(&audio::store::load());
            commands::watch_denoise(app.handle(), &slot.denoise);
            app.manage(VoiceState(tokio::sync::Mutex::new(slot)));
            #[cfg(windows)]
            app.manage(commands::ShareState::default());
            #[cfg(windows)]
            app.manage(commands::WatchState::default());
            let handle = app.handle().clone();
            audio::watch::start(move |input, output| {
                use tauri::Emitter;
                if let Err(e) = handle.emit(
                    "audio://devices",
                    serde_json::json!({ "input": input, "output": output }),
                ) {
                    log::warn!("事件广播失败：{e}");
                }
            });
            #[cfg(all(target_os = "macos", debug_assertions))]
            devshot::init(app.handle());
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(windows)]
                winfocus::init(&window);
                window_state::init(&window);
            }
            Ok(())
        })
        .manage(Mutex::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::restore_session,
            commands::storage_info,
            commands::storage_relocate,
            commands::storage_switch,
            commands::storage_clear_cache,
            commands::account_state,
            commands::set_account_state,
            commands::restart,
            commands::sayings,
            commands::sound_sync,
            commands::denoise_sync,
            commands::share_sdk_sync,
            #[cfg(windows)]
            commands::share_preview_open,
            #[cfg(windows)]
            commands::share_preview_close,
            #[cfg(windows)]
            commands::share_sources,
            #[cfg(windows)]
            commands::share_thumbnail,
            #[cfg(windows)]
            commands::share_start,
            #[cfg(windows)]
            commands::share_stop,
            #[cfg(windows)]
            commands::share_update,
            #[cfg(windows)]
            commands::watch_start,
            #[cfg(windows)]
            commands::watch_stop,
            #[cfg(windows)]
            commands::watch_key_frame,
            #[cfg(windows)]
            commands::watch_redirect,
            #[cfg(windows)]
            commands::watch_pip_open,
            #[cfg(windows)]
            commands::watch_pip_close,
            commands::effect_play,
            commands::effect_play_item,
            commands::set_effect_volume,
            commands::set_effect_enabled,
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
            commands::channel_guild,
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
            commands::client_log,
            commands::voice_join,
            commands::voice_leave,
            commands::voice_wake,
            commands::golive_allowed,
            commands::voice_set_mic,
            commands::audio_inputs,
            commands::set_audio_input,
            commands::voice_set_deafen,
            commands::audio_outputs,
            commands::set_audio_output,
            commands::voice_set_volume,
            commands::voice_set_peer_denoise,
            commands::guild_mute,
            commands::guild_mutes,
            commands::voice_kick,
            commands::voice_move,
            commands::guild_kick,
            commands::guild_ban,
            commands::voice_set_master,
            commands::mic_test,
            commands::voice_set_gain,
            commands::voice_set_denoise,
            commands::audio_prefs,
            commands::set_voice_switches,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                tauri::async_runtime::block_on(commands::stop_share(app));
            }
        });
}
