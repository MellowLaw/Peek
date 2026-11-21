use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, WebviewWindow};
use tokio::sync::watch;

use crate::preferences::{PreferencesState, SizePreset};

pub const STAGE_LOGICAL_W: f64 = 620.0;
pub const STAGE_LOGICAL_H: f64 = 520.0;
pub const EAR_LOGICAL_PX: f64 = 10.0;
const CANVAS_PAD_X_LOGICAL: f64 = 64.0;
const CANVAS_PAD_Y_LOGICAL: f64 = 32.0;

#[derive(Default)]
pub struct Transition(AtomicU64);

pub struct CurrentMode(pub Mutex<Mode>);

impl Default for CurrentMode {
    fn default() -> Self {
        Self(Mutex::new(Mode::Idle))
    }
}

pub struct CurrentLogicalSize(pub Mutex<(f64, f64)>);

impl Default for CurrentLogicalSize {
    fn default() -> Self {
        Self(Mutex::new((108.0, 30.0)))
    }
}

pub struct VisibilityWatcher(pub watch::Sender<bool>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Hidden,
    Idle,
    Lyrics,
    CompactBadge,
    Preview,
    Activity,
    DragTarget,
    Expanded,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub name: String,
    pub device_name: String,
    pub is_primary: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub work_x: i32,
    pub work_y: i32,
    pub work_width: u32,
    pub work_height: u32,
    pub scale: f64,
}

#[derive(Clone, Copy)]
struct MonitorMetrics {
    left: i32,
    work_width: i32,
    work_height: i32,
    top: i32,
    scale: f64,
}

pub fn logical_dimensions(mode: Mode, preset: SizePreset) -> (f64, f64) {
    match mode {
        Mode::Hidden => (220.0, 4.0),
        Mode::Idle => (108.0, 30.0),
        Mode::Lyrics => (260.0, 38.0),
        Mode::CompactBadge => (236.0, 34.0),
        Mode::Preview => (464.0, 42.0),
        Mode::Activity => (336.0, 52.0),
        Mode::DragTarget => (480.0, 360.0),
        Mode::Expanded => match preset {
            SizePreset::Compact => (480.0, 280.0),
            SizePreset::Standard => (504.0, 320.0),
            SizePreset::Large => (556.0, 360.0),
        },
    }
}

pub fn mode_corner_radius(mode: Mode) -> f64 {
    match mode {
        Mode::Hidden => 0.0,
        Mode::Idle => 16.0,
        Mode::CompactBadge => 18.0,
        Mode::Lyrics => 20.0,
        Mode::Preview => 22.0,
        Mode::Activity => 24.0,
        Mode::DragTarget | Mode::Expanded => 36.0,
    }
}

fn resolve_logical_size(
    mode: Mode,
    preset: SizePreset,
    target_w: Option<f64>,
    target_h: Option<f64>,
) -> (f64, f64) {
    let (def_w, def_h) = logical_dimensions(mode, preset);
    let w = target_w
        .unwrap_or(def_w)
        .clamp(16.0, STAGE_LOGICAL_W - 2.0 * EAR_LOGICAL_PX);
    let h = target_h.unwrap_or(def_h).clamp(2.0, STAGE_LOGICAL_H);
    (w, h)
}

fn even_px(val: f64) -> u32 {
    let n = val.round().max(2.0) as u32;
    if n % 2 == 1 {
        n + 1
    } else {
        n
    }
}

fn ear_logical_for(mode: Mode) -> f64 {
    if matches!(mode, Mode::Hidden) {
        0.0
    } else {
        EAR_LOGICAL_PX
    }
}

fn stage_physical_bounds(metrics: MonitorMetrics) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let stage_w = even_px(STAGE_LOGICAL_W * metrics.scale).min(metrics.work_width.max(2) as u32);
    let stage_h = ((STAGE_LOGICAL_H * metrics.scale).round() as u32)
        .clamp(2, metrics.work_height.max(2) as u32);
    let pos = PhysicalPosition::new(
        metrics.left + (metrics.work_width - stage_w as i32) / 2,
        metrics.top,
    );
    (pos, PhysicalSize::new(stage_w, stage_h))
}

fn region_physical_size(
    mode: Mode,
    logical_w: f64,
    logical_h: f64,
    metrics: MonitorMetrics,
    stage_size: PhysicalSize<u32>,
) -> (u32, u32, i32) {
    let ear_log = ear_logical_for(mode);
    let rw = even_px((logical_w + 2.0 * ear_log) * metrics.scale).min(stage_size.width);
    let rh = ((logical_h * metrics.scale).round() as u32).clamp(2, stage_size.height);
    let ear_px = (ear_log * metrics.scale).round() as i32;
    (rw, rh, ear_px)
}

#[cfg(test)]
fn centered_stage_region_rect(stage_w: u32, region_w: u32, region_h: u32) -> (i32, i32, i32) {
    let sw = (stage_w as i32).max(1);
    let rw = (region_w as i32).clamp(1, sw);
    let left = ((sw - rw) / 2).max(0);
    (left, left + rw, region_h as i32)
}

fn select_monitor(window: &WebviewWindow, preferred_name: &str) -> Result<Monitor, String> {
    if !preferred_name.is_empty() {
        if let Ok(monitors) = window.available_monitors() {
            if let Some(found) = monitors
                .into_iter()
                .find(|m| m.name().map(|n| n.as_str()) == Some(preferred_name))
            {
                return Ok(found);
            }
        }
    }
    window
        .primary_monitor()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Primary display unavailable".to_string())
}

fn monitor_metrics(
    window: &WebviewWindow,
    preferred_monitor: &str,
) -> Result<MonitorMetrics, String> {
    let monitor = select_monitor(window, preferred_monitor)?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    Ok(MonitorMetrics {
        left: area.position.x,
        work_width: area.size.width as i32,
        work_height: area.size.height as i32,
        top: area.position.y,
        scale,
    })
}

/// Return the physical screen rect `(left, top, right, bottom, scale)` of the active visible notch.
#[cfg(windows)]
pub fn monitor_notch_rect(app: &AppHandle, mode: Mode) -> Option<(i32, i32, i32, i32, f64)> {
    let win = app.get_webview_window("main")?;
    let (pref_mon, preset) = {
        let prefs_state = app.state::<PreferencesState>();
        let prefs = prefs_state.0.lock().unwrap();
        (prefs.monitor_name.clone(), prefs.size_preset)
    };
    let m = monitor_metrics(&win, &pref_mon).ok()?;
    let (def_w, def_h) = logical_dimensions(mode, preset);
    let (w, h) = if matches!(mode, Mode::Expanded) {
        let cur = *app.state::<CurrentLogicalSize>().0.lock().unwrap();
        if cur.0 >= 400.0 {
            (cur.0, cur.1.max(240.0))
        } else {
            (def_w, def_h.max(240.0))
        }
    } else {
        (def_w, def_h)
    };
    let (_, stage_size) = stage_physical_bounds(m);
    let (rw, rh, _) = region_physical_size(mode, w, h, m, stage_size);
    let left = m.left + (m.work_width - rw as i32) / 2;
    Some((left, m.top, left + rw as i32, m.top + rh as i32, m.scale))
}

fn apply_settled_region(
    window: &WebviewWindow,
    mode: Mode,
    logical_w: f64,
    logical_h: f64,
    metrics: MonitorMetrics,
) -> Result<(), String> {
    let (_pos, stage_size) = stage_physical_bounds(metrics);
    let (rw, rh, ear_px) = region_physical_size(mode, logical_w, logical_h, metrics, stage_size);
    let rounded_bottom = !matches!(mode, Mode::Hidden);
    let radius_logical = mode_corner_radius(mode);
    #[cfg(windows)]
    crate::windows::set_island_region(
        window,
        stage_size.width,
        rw,
        rh,
        metrics.scale,
        radius_logical,
        ear_px,
        rounded_bottom,
    )?;
    #[cfg(not(windows))]
    {
        let _ = (stage_size, rw, rh, ear_px, rounded_bottom, radius_logical);
    }
    Ok(())
}

pub fn initialize(
    window: &WebviewWindow,
    monitor_name: &str,
    preset: SizePreset,
    always_visible: bool,
) -> Result<(), String> {
    let initial = if always_visible {
        Mode::Idle
    } else {
        Mode::Hidden
    };
    let (w, h) = logical_dimensions(initial, preset);
    let app = window.app_handle();
    *app.state::<CurrentMode>().0.lock().unwrap() = initial;
    *app.state::<CurrentLogicalSize>().0.lock().unwrap() = (w, h);

    let metrics = monitor_metrics(window, monitor_name)?;
    let (pos, stage_size) = stage_physical_bounds(metrics);
    #[cfg(windows)]
    crate::windows::anchor_island_stage(window, pos, stage_size)?;
    #[cfg(not(windows))]
    {
        window.set_size(stage_size).map_err(|e| e.to_string())?;
        window.set_position(pos).map_err(|e| e.to_string())?;
    }
    apply_settled_region(window, initial, w, h, metrics)
}

/// Re-apply `WS_POPUP` borderless styles and the active curved HWND region after Tao's
/// `window.show()` resets `GWL_STYLE`/`GWL_EXSTYLE` and triggers `SWP_FRAMECHANGED`.
pub fn restore_frameless_after_show(window: &WebviewWindow) -> Result<(), String> {
    let app = window.app_handle();
    let preferred_monitor = app
        .state::<PreferencesState>()
        .0
        .lock()
        .unwrap()
        .monitor_name
        .clone();
    let mode = *app.state::<CurrentMode>().0.lock().unwrap();
    let (w, h) = *app.state::<CurrentLogicalSize>().0.lock().unwrap();
    let metrics = monitor_metrics(window, &preferred_monitor)?;
    #[cfg(windows)]
    let drag_active = crate::drop_catcher::is_external_file_drag_active();
    #[cfg(not(windows))]
    let drag_active = false;
    let focusable = matches!(mode, Mode::Expanded) && !drag_active;

    #[cfg(windows)]
    crate::windows::enforce_frameless_popup(window, focusable)?;
    #[cfg(not(windows))]
    let _ = focusable;

    apply_settled_region(window, mode, w, h, metrics)
}

/// Show Peek's main notch window and immediately restore its frameless `WS_POPUP` style and region.
pub fn show_island_window(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    #[cfg(windows)]
    {
        let target = window.clone();
        let _ = window.run_on_main_thread(move || {
            let _ = restore_frameless_after_show(&target);
        });
    }
    Ok(())
}

pub fn start_visibility_watcher(app: AppHandle, always_visible: bool) -> VisibilityWatcher {
    let (tx, mut rx) = watch::channel(always_visible);
    tauri::async_runtime::spawn(async move {
        let mut was_in_zone = false;
        let mut last_emit = Instant::now() - Duration::from_secs(5);
        while rx.wait_for(|always| !*always).await.is_ok() {
            loop {
                if *rx.borrow() {
                    was_in_zone = false;
                    break;
                }
                #[cfg(windows)]
                {
                    let is_hidden = *app.state::<CurrentMode>().0.lock().unwrap() == Mode::Hidden;
                    if is_hidden && !crate::windows::left_mouse_down() {
                        if let (Some(win), Some((cx, cy))) = (
                            app.get_webview_window("main"),
                            crate::windows::cursor_position(),
                        ) {
                            let pref_mon = app
                                .state::<PreferencesState>()
                                .0
                                .lock()
                                .unwrap()
                                .monitor_name
                                .clone();
                            if let Ok(mon) = select_monitor(&win, &pref_mon) {
                                let scale = mon.scale_factor();
                                let area = mon.work_area();
                                let center_x = area.position.x + (area.size.width as i32) / 2;
                                let half_zone = (130.0 * scale).round() as i32;
                                let top_threshold = area.position.y + (6.0 * scale).round() as i32;
                                let in_zone = cy <= top_threshold
                                    && cy >= area.position.y - 4
                                    && (cx - center_x).abs() <= half_zone;
                                if in_zone && !was_in_zone {
                                    if last_emit.elapsed() > Duration::from_millis(350) {
                                        last_emit = Instant::now();
                                        let _ = win.emit("top-edge-hover", ());
                                    }
                                }
                                was_in_zone = in_zone;
                            }
                        }
                    } else {
                        was_in_zone = false;
                    }
                }
                tokio::select! {
                    _ = rx.changed() => {}
                    _ = tokio::time::sleep(Duration::from_millis(60)) => {}
                }
            }
        }
    });
    VisibilityWatcher(tx)
}

pub fn sync_visibility(app: &AppHandle, always_visible: bool) {
    if let Some(watcher) = app.try_state::<VisibilityWatcher>() {
        watcher.0.send_replace(always_visible);
    }
}

pub async fn reanchor(app: &AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    let (preferred_monitor, preset, always_visible) = {
        let prefs_state = app.state::<PreferencesState>();
        let prefs = prefs_state.0.lock().unwrap();
        (
            prefs.monitor_name.clone(),
            prefs.size_preset,
            prefs.always_visible,
        )
    };
    let mut mode = *app.state::<CurrentMode>().0.lock().unwrap();
    if (mode == Mode::Idle || mode == Mode::Lyrics) && !always_visible {
        mode = Mode::Hidden;
    } else if mode == Mode::Hidden && always_visible {
        mode = Mode::Idle;
    }
    let metrics = monitor_metrics(&window, &preferred_monitor)?;
    let (pos, stage_size) = stage_physical_bounds(metrics);
    let target = window.clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .run_on_main_thread(move || {
            #[cfg(windows)]
            let res = crate::windows::anchor_island_stage(&target, pos, stage_size);
            #[cfg(not(windows))]
            let res = target
                .set_size(stage_size)
                .and_then(|_| target.set_position(pos))
                .map_err(|e| e.to_string());
            let _ = send.send(res);
        })
        .map_err(|e| e.to_string())?;
    receive.await.map_err(|e| e.to_string())??;

    let (w, h) = resolve_logical_size(mode, preset, None, None);
    *app.state::<CurrentLogicalSize>().0.lock().unwrap() = (w, h);
    let _ = set_island_mode(window, mode, false).await?;
    Ok(())
}

#[tauri::command]
pub fn list_monitors(window: WebviewWindow) -> Result<Vec<MonitorInfo>, String> {
    let primary_name = window
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().cloned());
    let monitors = window.available_monitors().map_err(|e| e.to_string())?;
    Ok(monitors
        .into_iter()
        .enumerate()
        .map(|(index, m)| {
            let device_name = m.name().cloned().unwrap_or_default();
            let is_primary = primary_name.as_deref() == Some(device_name.as_str())
                || (m.position().x == 0 && m.position().y == 0);
            let short = device_name.trim_start_matches(r"\\.\").to_string();
            let name = if short.is_empty() {
                format!("Display {}", index + 1)
            } else if is_primary {
                format!("{} (Primary)", short)
            } else {
                short
            };
            let pos = m.position();
            let sz = m.size();
            let work = m.work_area();
            MonitorInfo {
                name,
                device_name,
                is_primary,
                x: pos.x,
                y: pos.y,
                width: sz.width,
                height: sz.height,
                work_x: work.position.x,
                work_y: work.position.y,
                work_width: work.size.width,
                work_height: work.size.height,
                scale: m.scale_factor(),
            }
        })
        .collect())
}

/// Phase 1 of GPU-driven spring transition: unmask a centered region covering both previous
/// and target notch dimensions inside Peek's fixed HWND stage (zero `SetWindowPos`, zero resize stutter).
#[tauri::command]
pub async fn prepare_island_transition(
    window: WebviewWindow,
    mode: Mode,
    target_width: Option<f64>,
    target_height: Option<f64>,
    focus: Option<bool>,
) -> Result<u64, String> {
    let app = window.app_handle().clone();
    let (preferred_monitor, preset) = {
        let prefs_state = app.state::<PreferencesState>();
        let prefs = prefs_state.0.lock().unwrap();
        (prefs.monitor_name.clone(), prefs.size_preset)
    };
    let prev_logical = *app.state::<CurrentLogicalSize>().0.lock().unwrap();
    let (next_w, next_h) = resolve_logical_size(mode, preset, target_width, target_height);
    *app.state::<CurrentMode>().0.lock().unwrap() = mode;
    *app.state::<CurrentLogicalSize>().0.lock().unwrap() = (next_w, next_h);

    let expanded = matches!(mode, Mode::Expanded);
    let should_focus = expanded && focus.unwrap_or(true);
    if should_focus {
        #[cfg(windows)]
        {
            let hwnds = crate::windows::peek_hwnds(&app);
            app.state::<crate::windows::ForegroundTracker>()
                .record_if_external(&hwnds);
        }
    }

    let token = window
        .state::<Transition>()
        .0
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let metrics = monitor_metrics(&window, &preferred_monitor)?;
    let (_, stage_size) = stage_physical_bounds(metrics);
    let canvas_w = even_px((prev_logical.0.max(next_w) + CANVAS_PAD_X_LOGICAL) * metrics.scale)
        .min(stage_size.width);
    let canvas_h = (((prev_logical.1.max(next_h) + CANVAS_PAD_Y_LOGICAL) * metrics.scale).round()
        as u32)
        .clamp(2, stage_size.height);

    let (send, receive) = tokio::sync::oneshot::channel();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            if target.state::<Transition>().0.load(Ordering::SeqCst) != token {
                let _ = send.send(Ok(token));
                return;
            }
            #[cfg(windows)]
            let res =
                crate::windows::enforce_frameless_popup(&target, should_focus).and_then(|_| {
                    crate::windows::set_island_region(
                        &target,
                        stage_size.width,
                        canvas_w,
                        canvas_h,
                        metrics.scale,
                        0.0,
                        0,
                        false,
                    )
                });
            #[cfg(not(windows))]
            let res = {
                let _ = (stage_size, canvas_w, canvas_h);
                target
                    .set_focusable(should_focus)
                    .map_err(|error| error.to_string())
            };

            let res = res.and_then(|_| {
                if should_focus {
                    target.set_focus().map_err(|error| error.to_string())?;
                }
                Ok(token)
            });
            let _ = send.send(res);
        })
        .map_err(|error| error.to_string())?;

    receive.await.map_err(|error| error.to_string())?
}

/// Phase 2 of GPU-driven spring transition: snap only the HWND region tightly around the settled notch.
#[tauri::command]
pub async fn settle_island_transition(
    window: WebviewWindow,
    mode: Mode,
    target_width: Option<f64>,
    target_height: Option<f64>,
    token: u64,
) -> Result<bool, String> {
    let app = window.app_handle().clone();
    let (preferred_monitor, preset) = {
        let prefs_state = app.state::<PreferencesState>();
        let prefs = prefs_state.0.lock().unwrap();
        (prefs.monitor_name.clone(), prefs.size_preset)
    };
    let metrics = monitor_metrics(&window, &preferred_monitor)?;
    let (next_w, next_h) = resolve_logical_size(mode, preset, target_width, target_height);
    let expanded = matches!(mode, Mode::Expanded);
    #[cfg(windows)]
    let drag_active = crate::drop_catcher::is_external_file_drag_active();
    #[cfg(not(windows))]
    let drag_active = false;
    let focusable = expanded && !drag_active;

    let (send, receive) = tokio::sync::oneshot::channel();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            if target.state::<Transition>().0.load(Ordering::SeqCst) != token {
                let _ = send.send(Ok(false));
                return;
            }
            #[cfg(windows)]
            let res = crate::windows::enforce_frameless_popup(&target, focusable)
                .and_then(|_| apply_settled_region(&target, mode, next_w, next_h, metrics))
                .map(|_| true);
            #[cfg(not(windows))]
            let res = apply_settled_region(&target, mode, next_w, next_h, metrics).and_then(|_| {
                target
                    .set_focusable(focusable)
                    .map_err(|error| error.to_string())?;
                Ok(true)
            });
            let _ = send.send(res);
        })
        .map_err(|error| error.to_string())?;

    receive.await.map_err(|error| error.to_string())?
}

/// Immediate snap command kept for reanchoring and synchronous callers.
#[tauri::command]
pub async fn set_island_mode(
    window: WebviewWindow,
    mode: Mode,
    animate: bool,
) -> Result<bool, String> {
    let _ = animate;
    let app = window.app_handle().clone();
    let (preferred_monitor, preset) = {
        let prefs_state = app.state::<PreferencesState>();
        let prefs = prefs_state.0.lock().unwrap();
        (prefs.monitor_name.clone(), prefs.size_preset)
    };
    let (w, h) = resolve_logical_size(mode, preset, None, None);
    let metrics = monitor_metrics(&window, &preferred_monitor)?;
    let expanded = matches!(mode, Mode::Expanded);
    if expanded {
        #[cfg(windows)]
        {
            let hwnds = crate::windows::peek_hwnds(&app);
            app.state::<crate::windows::ForegroundTracker>()
                .record_if_external(&hwnds);
        }
    }
    *app.state::<CurrentMode>().0.lock().unwrap() = mode;
    *app.state::<CurrentLogicalSize>().0.lock().unwrap() = (w, h);
    let token = window
        .state::<Transition>()
        .0
        .fetch_add(1, Ordering::SeqCst)
        + 1;

    let (send, receive) = tokio::sync::oneshot::channel();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            if target.state::<Transition>().0.load(Ordering::SeqCst) != token {
                let _ = send.send(Ok(false));
                return;
            }
            #[cfg(windows)]
            let result = crate::windows::enforce_frameless_popup(&target, expanded)
                .and_then(|_| apply_settled_region(&target, mode, w, h, metrics))
                .and_then(|_| {
                    if expanded {
                        target.set_focus().map_err(|error| error.to_string())?;
                    }
                    Ok(true)
                });
            #[cfg(not(windows))]
            let result = apply_settled_region(&target, mode, w, h, metrics).and_then(|_| {
                target
                    .set_focusable(expanded)
                    .map_err(|error| error.to_string())?;
                if expanded {
                    target.set_focus().map_err(|error| error.to_string())?;
                }
                Ok(true)
            });
            let _ = send.send(result);
        })
        .map_err(|error| error.to_string())?;
    receive.await.map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_stage_keeps_exact_center_parity_across_scales() {
        for scale in [1.0_f64, 1.25, 1.5, 2.0] {
            let m = MonitorMetrics {
                left: 0,
                work_width: 1920,
                work_height: 1032,
                top: 0,
                scale,
            };
            let (stage_pos, stage_sz) = stage_physical_bounds(m);
            assert_eq!(stage_sz.width % 2, 0);
            let stage_mid = stage_pos.x + (stage_sz.width as i32) / 2;
            assert_eq!(stage_mid, 960);

            for mode in [
                Mode::Idle,
                Mode::Lyrics,
                Mode::CompactBadge,
                Mode::Preview,
                Mode::Expanded,
            ] {
                let (w, h) = logical_dimensions(mode, SizePreset::Standard);
                let (rw, rh, _) = region_physical_size(mode, w, h, m, stage_sz);
                assert_eq!(rw % 2, 0);
                let (l, r, b) = centered_stage_region_rect(stage_sz.width, rw, rh);
                let region_mid = stage_pos.x + (l + r) / 2;
                assert_eq!(region_mid, 960);
                assert!(b >= 2);
            }
        }
    }
}
