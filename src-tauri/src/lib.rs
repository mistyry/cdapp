pub mod audio;
mod commands;
#[cfg(all(target_os = "macos", debug_assertions))]
mod devshot;
pub mod error;
pub mod kook;
pub mod storage;
mod window_state;
#[cfg(windows)]
mod winfocus;

use std::sync::Mutex;

use commands::{AppState, VoiceState};

/// 启动参数 `--data-dir <目录>` 或环境变量 `CDAPP_DATA_DIR` 指定独立的数据目录，用于测试。
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 启动应用。
///
/// 关闭对客户端无用的浏览器行为：右键菜单、查找、刷新、打印、下载、查看源码等。
/// 开发模式保留右键菜单、调试工具与刷新。Shift+Tab 用于键盘导航，不拦截。
fn prevent_default<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_prevent_default::Flags;
    let builder = tauri_plugin_prevent_default::Builder::new()
        .with_flags(Flags::debug().difference(Flags::FOCUS_MOVE));
    // WebView2 原生开关：发行版关闭浏览器快捷键与默认右键菜单。
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
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(prevent_default())
        .setup(|app| {
            use tauri::Manager;
            if let Some(dir) = data_dir_arg() {
                // 独立目录：固定目录与数据根都放在其中，不读取系统位置的配置。
                let fixed = dir.join("fixed");
                storage::init(&fixed, &dir.join("data"), &fixed);
            } else {
                let data = app.path().app_data_dir()?;
                storage::init(&app.path().app_config_dir()?, &data, &data);
            }
            let slot = commands::VoiceSlot::from_prefs(&audio::store::load());
            app.manage(VoiceState(tokio::sync::Mutex::new(slot)));
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
            commands::set_voice_switches,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
