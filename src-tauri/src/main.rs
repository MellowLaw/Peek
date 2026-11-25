#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activity;
mod capture;
mod dev_monitor;
#[cfg(windows)]
mod drop_catcher;
mod floating;
mod island;
mod local_api;
mod lyrical;
#[cfg(windows)]
mod media;
mod notifications;
mod preferences;
mod shelf;
mod system_controls;
mod tools;
mod updater;
#[cfg(windows)]
mod windows;

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, DragDropEvent, Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::preferences::{Preferences, PreferencesState, ShortcutStates, ShortcutsState};

pub(crate) const PEEK_BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-background-networking --disable-component-update --disable-sync";

static LAST_DRAG_OVER_MS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDragPayload {
    kind: &'static str,
    paths: Vec<String>,
}

#[derive(Clone, Copy)]
enum ShortcutKind {
    Toggle,
    Capture,
}

fn on_shortcut(app: &AppHandle, kind: ShortcutKind) {
    #[cfg(windows)]
    {
        let hwnds = windows::peek_hwnds(app);
        app.state::<windows::ForegroundTracker>()
            .record_if_external(&hwnds);
    }
    match kind {
        ShortcutKind::Toggle => {
            if let Some(window) = app.get_webview_window("main") {
                let result = window.is_visible().and_then(|visible| {
                    if !visible {
                        island::show_island_window(&window)?;
                        let _ = window.emit("peek-visibility", true);
                    }
                    window.emit("toggle-peek", !visible)
                });
                if let Err(error) = result {
                    eprintln!("Shortcut toggle failed: {error}");
                }
            }
        }
        ShortcutKind::Capture => {
            capture::begin_region_from_shortcut(app.clone());
        }
    }
}

pub fn register_shortcuts(app: &AppHandle, prefs: &Preferences) -> ShortcutStates {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut states = ShortcutStates::default();

    let register_one = |combo: &str, kind: ShortcutKind| -> (bool, String) {
        let trimmed = combo.trim();
        if trimmed.is_empty() {
            return (false, "Unassigned".into());
        }
        match gs.on_shortcut(trimmed, move |app, _sc, event| {
            if event.state() == ShortcutState::Released {
                on_shortcut(app, kind);
            }
        }) {
            Ok(()) => (true, String::new()),
            Err(err) => (false, err.to_string()),
        }
    };

    let (ok, err) = register_one(&prefs.toggle_shortcut, ShortcutKind::Toggle);
    states.toggle = ok;
    states.toggle_error = err;

    let (ok, err) = register_one(&prefs.capture_shortcut, ShortcutKind::Capture);
    states.capture = ok;
    states.capture_error = err;

    states
}

#[tauri::command]
fn shortcut_available(state: tauri::State<'_, ShortcutsState>) -> bool {
    state.0.lock().unwrap().toggle
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

fn main() {
    #[cfg(windows)]
    let _single_instance = match windows::acquire_single_instance_guard() {
        Some(guard) => guard,
        None => return,
    };

    tauri::Builder::default()
        .manage(island::Transition::default())
        .manage(island::CurrentMode::default())
        .manage(island::CurrentLogicalSize::default())
        .manage(windows::ForegroundTracker::default())
        .manage(capture::CaptureState::default())
        .manage(system_controls::SystemMetricsState::default())
        .manage(local_api::LocalApiState::default())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::Focused(false) = event {
                let app = window.app_handle();
                let suppressed = {
                    #[cfg(windows)]
                    {
                        windows::is_auto_close_suppressed()
                            || drop_catcher::is_external_file_drag_active()
                    }
                    #[cfg(not(windows))]
                    {
                        false
                    }
                } || app
                    .try_state::<capture::CaptureState>()
                    .is_some_and(|c| c.is_overlay_active());
                if !suppressed {
                    let _ = window.emit("peek-blur", ());
                }
                return;
            }
            if let WindowEvent::DragDrop(drag) = event {
                let payload = match drag {
                    DragDropEvent::Enter { paths, .. } => {
                        LAST_DRAG_OVER_MS.store(activity::now_ms(), Ordering::Relaxed);
                        Some(FileDragPayload {
                            kind: "enter",
                            paths: paths
                                .iter()
                                .filter_map(|p| p.to_str().map(str::to_string))
                                .collect(),
                        })
                    }
                    DragDropEvent::Over { .. } => {
                        let now = activity::now_ms();
                        let prev = LAST_DRAG_OVER_MS.load(Ordering::Relaxed);
                        if now.saturating_sub(prev) >= 120 {
                            LAST_DRAG_OVER_MS.store(now, Ordering::Relaxed);
                            Some(FileDragPayload {
                                kind: "over",
                                paths: Vec::new(),
                            })
                        } else {
                            None
                        }
                    }
                    DragDropEvent::Leave => Some(FileDragPayload {
                        kind: "leave",
                        paths: Vec::new(),
                    }),
                    DragDropEvent::Drop { paths, .. } => {
                        shelf::ingest_dropped_paths(window.app_handle(), paths);
                        Some(FileDragPayload {
                            kind: "drop",
                            paths: paths
                                .iter()
                                .filter_map(|p| p.to_str().map(str::to_string))
                                .collect(),
                        })
                    }
                    _ => None,
                };
                if let Some(p) = payload {
                    let _ = window.emit("file-drag", p);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            island::set_island_mode,
            island::prepare_island_transition,
            island::settle_island_transition,
            island::focus_island_window,
            island::list_monitors,
            shortcut_available,
            preferences::get_preferences,
            preferences::save_preferences,
            preferences::shortcut_states,
            preferences::get_autostart_status,
            activity::get_activity_hub,
            activity::dismiss_activity,
            activity::dismiss_notification,
            activity::clear_notifications,
            lyrical::get_lyrical_snapshot,
            lyrical::open_in_lyrical,
            shelf::get_shelf,
            shelf::add_shelf_files,
            shelf::pick_shelf_files,
            shelf::remove_shelf_item,
            shelf::clear_shelf,
            shelf::reveal_shelf_file,
            shelf::copy_shelf_files,
            shelf::move_shelf_files,
            shelf::rename_shelf_file,
            shelf::zip_shelf_files,
            shelf::extract_shelf_zip,
            shelf::convert_shelf_image,
            shelf::start_file_drag,
            shelf::start_qr_transfer,
            shelf::start_qr_receive,
            shelf::get_qr_transfer,
            shelf::allow_lan_firewall,
            shelf::stop_qr_transfer,
            capture::capture_full_display,
            capture::capture_active_window,
            capture::begin_screen_overlay,
            capture::get_overlay_state,
            capture::switch_overlay_mode,
            capture::complete_overlay_region,
            capture::complete_overlay_fullscreen,
            capture::complete_overlay_window,
            capture::complete_overlay_color,
            capture::cancel_overlay,
            capture::ocr_image_file,
            capture::reveal_capture_file,
            capture::open_recordings_folder,
            capture::open_captures_folder,
            capture::list_audio_inputs,
            capture::start_recording,
            capture::pause_recording,
            capture::resume_recording,
            capture::stop_recording,
            capture::get_recording_status,
            dev_monitor::get_dev_monitor,
            dev_monitor::watch_dev_monitor,
            dev_monitor::install_claude_statusline,
            dev_monitor::remove_claude_statusline,
            system_controls::get_audio_snapshot,
            system_controls::set_master_volume,
            system_controls::set_master_mute,
            system_controls::set_mic_mute,
            system_controls::set_default_output_device,
            system_controls::set_app_audio,
            system_controls::get_brightness_displays,
            system_controls::set_display_brightness,
            system_controls::get_system_metrics,
            system_controls::snap_active_window,
            system_controls::open_windows_setting,
            system_controls::get_connectivity_state,
            system_controls::toggle_radio,
            floating::open_floating_camera,
            floating::open_floating_note,
            floating::open_floating_update,
            floating::close_floating_window,
            floating::start_floating_drag,
            floating::start_floating_resize,
            floating::resize_floating_window,
            floating::set_floating_always_on_top,
            floating::save_camera_snapshot,
            local_api::get_local_api_status,
            notifications::get_windows_notification_status,
            tools::get_clipboard_history,
            tools::copy_clipboard_item,
            tools::toggle_pin_clipboard_item,
            tools::delete_clipboard_item,
            tools::clear_clipboard_history,
            tools::get_notes_and_snippets,
            tools::save_note,
            tools::delete_note,
            tools::save_snippet,
            tools::delete_snippet,
            tools::copy_tool_text,
            tools::get_timer_state,
            tools::start_timer,
            tools::pause_timer,
            tools::resume_timer,
            tools::restart_timer,
            tools::reset_timer,
            tools::add_reminder,
            tools::remove_reminder,
            tools::scan_installed_apps,
            tools::get_app_icons,
            tools::launch_app,
            #[cfg(windows)]
            media::watch_media,
            #[cfg(windows)]
            media::control_media,
            #[cfg(windows)]
            media::select_media_session,
            #[cfg(windows)]
            media::seek_media,
            updater::get_update_state,
            updater::check_for_updates,
            updater::install_available_update,
            updater::dismiss_update_prompt,
            updater::skip_available_update,
            quit_app
        ])
        .setup(|app| {
            let initial_prefs = preferences::load_from_disk(app.handle()).unwrap_or_default();
            let config_dir = app.path().app_config_dir().ok();
            let updater_state = std::sync::Arc::new(updater::UpdaterState::new(
                app.package_info().version.to_string(),
            ));
            app.manage(updater_state.clone());
            updater::spawn_startup_check(app.handle().clone(), updater_state);
            let activity_mgr = activity::ActivityManager::start(
                app.handle().clone(),
                config_dir.clone(),
                initial_prefs.privacy_policy(),
            );
            app.manage(activity_mgr);
            app.manage(shelf::ShelfState::load(config_dir.clone()));
            app.manage(lyrical::LyricalState::load(config_dir.clone()));
            app.manage(notifications::start(
                app.handle().clone(),
                initial_prefs.notifications_enabled,
            ));
            app.manage(tools::ToolsState::start(
                app.handle().clone(),
                config_dir,
                initial_prefs.clipboard_history_enabled,
            ));
            app.manage(PreferencesState(std::sync::Mutex::new(
                initial_prefs.clone(),
            )));
            #[cfg(windows)]
            windows::sync_autostart_if_needed(initial_prefs.autostart);
            if initial_prefs.local_api_enabled {
                let app_handle = app.handle().clone();
                let port = initial_prefs.local_api_port;
                let token = initial_prefs.local_api_token.clone();
                tauri::async_runtime::spawn(async move {
                    local_api::sync_config(app_handle, true, port, token).await;
                });
            }

            let window = app
                .get_webview_window("main")
                .ok_or("Peek window unavailable")?;
            island::initialize(
                &window,
                &initial_prefs.monitor_name,
                initial_prefs.size_preset,
                initial_prefs.always_visible,
            )
            .map_err(std::io::Error::other)?;
            app.manage(island::start_visibility_watcher(
                app.handle().clone(),
                initial_prefs.always_visible,
            ));
            app.manage(island::start_expanded_watchdog(app.handle().clone()));

            #[cfg(windows)]
            {
                app.manage(media::start(window.clone()));
                drop_catcher::start(app.handle().clone());
            }
            app.manage(dev_monitor::start(app.handle().clone()));

            let shortcut_states = register_shortcuts(app.handle(), &initial_prefs);
            app.manage(ShortcutsState(std::sync::Mutex::new(shortcut_states)));

            let show = MenuItem::with_id(app, "show", "Show Peek", true, None::<&str>)?;
            let hide = MenuItem::with_id(app, "hide", "Hide Peek", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Peek", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &hide, &settings, &quit])?;
            let tray_icon = load_tray_icon()
                .or_else(|| app.default_window_icon().cloned())
                .ok_or_else(|| std::io::Error::other("Missing Peek tray icon"))?;
            TrayIconBuilder::new()
                .icon(tray_icon)
                .tooltip("Peek")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    } else if let Some(window) = app.get_webview_window("main") {
                        let result = if event.id.as_ref() == "settings" {
                            island::show_island_window(&window)
                                .and_then(|_| window.emit("peek-visibility", true))
                                .and_then(|_| window.emit("open-settings", ()))
                        } else if event.id.as_ref() == "show" {
                            island::show_island_window(&window)
                                .and_then(|_| window.emit("peek-visibility", true))
                        } else {
                            window
                                .hide()
                                .and_then(|_| window.emit("peek-visibility", false))
                        };
                        if let Err(error) = result {
                            eprintln!("Tray action failed: {error}");
                        }
                    }
                })
                .build(app)?;
            #[cfg(windows)]
            windows::set_webview_transparent_bg(&window);
            window.show()?;
            island::restore_frameless_after_show(&window).map_err(std::io::Error::other)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Peek could not start");
}

fn load_tray_icon() -> Option<Image<'static>> {
    const ICON_PNG: &[u8] = include_bytes!("../icons/64x64.png");
    let decoded = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (width, height) = decoded.dimensions();
    Some(Image::new_owned(decoded.into_raw(), width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_decodes_valid_rgba() {
        let icon = load_tray_icon().expect("64x64 tray icon should decode");
        assert_eq!(icon.width(), 64);
        assert_eq!(icon.height(), 64);
        let rgba = icon.rgba();
        assert_eq!(rgba.len(), 64 * 64 * 4);
        // Top-left corner outside the squircle and gripping paws remains transparent.
        assert_eq!(rgba[3], 0);
        assert!(rgba.chunks_exact(4).any(|px| px[3] == 255));
    }
}
