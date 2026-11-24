use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::ImageFormat;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::preferences::PreferencesState;
use crate::tools::ToolsState;

const MAX_SNAPSHOT_B64_BYTES: usize = 22 * 1024 * 1024;
const MAX_OPEN_FLOATING_NOTES: usize = 5;

fn normalize_enhance_mode(raw: Option<&str>) -> &'static str {
    match raw.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("natural") => "natural",
        Some(v) if v.eq_ignore_ascii_case("studio") => "studio",
        _ => "off",
    }
}

static FLOATING_RESIZE_ACTIVE: AtomicBool = AtomicBool::new(false);

struct FloatingResizeGuard;

impl Drop for FloatingResizeGuard {
    fn drop(&mut self) {
        FLOATING_RESIZE_ACTIVE.store(false, Ordering::SeqCst);
    }
}

fn compute_resized_rect(
    orig: (i32, i32, i32, i32),
    corner: &str,
    dx: i32,
    dy: i32,
    min_w: i32,
    max_w: i32,
    min_h: i32,
    max_h: i32,
) -> (i32, i32, i32, i32) {
    let (l, t, r, b) = orig;
    let orig_w = (r - l).max(1);
    let orig_h = (b - t).max(1);
    let (new_w, new_h, x, y) = match corner {
        "se" => {
            let w = (orig_w + dx).clamp(min_w, max_w);
            let h = (orig_h + dy).clamp(min_h, max_h);
            (w, h, l, t)
        }
        "sw" => {
            let w = (orig_w - dx).clamp(min_w, max_w);
            let h = (orig_h + dy).clamp(min_h, max_h);
            (w, h, r - w, t)
        }
        "ne" => {
            let w = (orig_w + dx).clamp(min_w, max_w);
            let h = (orig_h - dy).clamp(min_h, max_h);
            (w, h, l, b - h)
        }
        "nw" => {
            let w = (orig_w - dx).clamp(min_w, max_w);
            let h = (orig_h - dy).clamp(min_h, max_h);
            (w, h, r - w, b - h)
        }
        _ => (orig_w, orig_h, l, t),
    };
    (x, y, new_w, new_h)
}

pub fn sanitize_window_key(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(32)
        .collect();
    if cleaned.is_empty() {
        "item".into()
    } else {
        cleaned
    }
}

fn ensure_floating_window(window: &WebviewWindow) -> Result<(), String> {
    if !window.label().starts_with("float-") {
        return Err("Only floating Peek windows support this action".into());
    }
    Ok(())
}

fn percent_encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn compute_floating_origin(
    app: &AppHandle,
    width: f64,
    height: f64,
    stagger_idx: i32,
) -> Option<(i32, i32)> {
    let main = app.get_webview_window("main")?;
    let preferred_mon = app
        .state::<PreferencesState>()
        .0
        .lock()
        .unwrap()
        .monitor_name
        .clone();
    let monitor = if !preferred_mon.is_empty() {
        main.available_monitors()
            .ok()
            .and_then(|list| {
                list.into_iter()
                    .find(|m| m.name().map(|n| n.as_str()) == Some(&preferred_mon))
            })
            .or_else(|| main.primary_monitor().ok().flatten())
    } else {
        main.primary_monitor().ok().flatten()
    }?;
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let pw = (width * scale).round() as i32;
    let ph = (height * scale).round() as i32;
    let offset = (stagger_idx * 26) as f64 * scale;
    let max_x = area.position.x + (area.size.width as i32 - pw).max(0);
    let max_y = area.position.y + (area.size.height as i32 - ph).max(0);
    let x = (area.position.x + (area.size.width as i32 - pw) / 2 + offset.round() as i32)
        .clamp(area.position.x, max_x);
    let y = (area.position.y + (90.0 * scale).round() as i32 + offset.round() as i32)
        .clamp(area.position.y, max_y);
    Some((x, y))
}

fn setup_floating_frame_at_y(
    app: &AppHandle,
    win: &WebviewWindow,
    width: f64,
    height: f64,
    stagger_idx: i32,
    y_offset_logical: f64,
    corner_radius: f64,
) -> Result<(), String> {
    if let Some((x, _)) = compute_floating_origin(app, width, height, stagger_idx) {
        let scale = app
            .get_webview_window("main")
            .and_then(|m| m.current_monitor().ok().flatten())
            .map(|m| m.scale_factor())
            .unwrap_or(1.0);
        let top_y = app
            .get_webview_window("main")
            .and_then(|m| m.primary_monitor().ok().flatten())
            .map(|m| m.work_area().position.y)
            .unwrap_or(0);
        let y = top_y + ((y_offset_logical + (stagger_idx * 26) as f64) * scale).round() as i32;
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }
    #[cfg(windows)]
    crate::windows::set_webview_transparent_bg(win);
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_focus();

    #[cfg(windows)]
    {
        let target = win.clone();
        let _ = win.run_on_main_thread(move || {
            crate::windows::set_webview_transparent_bg(&target);
            let _ = crate::windows::enforce_frameless_popup(&target, true);
            let _ = crate::windows::apply_rounded_window_region(&target, corner_radius);
        });
    }
    Ok(())
}

fn setup_floating_frame(
    app: &AppHandle,
    win: &WebviewWindow,
    width: f64,
    height: f64,
    stagger_idx: i32,
) -> Result<(), String> {
    setup_floating_frame_at_y(app, win, width, height, stagger_idx, 90.0, 16.0)
}

#[tauri::command]
pub async fn open_floating_camera(
    app: AppHandle,
    device_id: Option<String>,
    mirrored: Option<bool>,
    enhance: Option<String>,
) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("float-camera") {
        let _ = existing.show();
        let _ = existing.set_focus();
        #[cfg(windows)]
        {
            let target = existing.clone();
            let _ = existing.run_on_main_thread(move || {
                crate::windows::set_webview_transparent_bg(&target);
                let _ = crate::windows::enforce_frameless_popup(&target, true);
                let _ = crate::windows::apply_rounded_window_region(&target, 16.0);
            });
        }
        return Ok(());
    }

    let dev = percent_encode_component(device_id.as_deref().unwrap_or(""));
    let mir = if mirrored.unwrap_or(true) { "1" } else { "0" };
    let enh = normalize_enhance_mode(enhance.as_deref());
    let url = format!("index.html#float-camera?device={dev}&mirrored={mir}&enhance={enh}");

    let win = WebviewWindowBuilder::new(&app, "float-camera", WebviewUrl::App(url.into()))
        .title("Peek Camera")
        .additional_browser_args(crate::PEEK_BROWSER_ARGS)
        .inner_size(360.0, 236.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;

    setup_floating_frame(&app, &win, 360.0, 236.0, 0)
}

pub fn open_floating_update_window(app: &AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("float-update") {
        #[cfg(windows)]
        crate::windows::set_webview_transparent_bg(&existing);
        let _ = existing.show();
        let _ = existing.set_focus();
        #[cfg(windows)]
        {
            let target = existing.clone();
            let _ = existing.run_on_main_thread(move || {
                crate::windows::set_webview_transparent_bg(&target);
                let _ = crate::windows::enforce_frameless_popup(&target, true);
                let _ = crate::windows::apply_rounded_window_region(&target, 18.0);
            });
        }
        return Ok(());
    }

    let win = WebviewWindowBuilder::new(
        app,
        "float-update",
        WebviewUrl::App("index.html#float-update".into()),
    )
    .title("Peek Update")
    .additional_browser_args(crate::PEEK_BROWSER_ARGS)
    .inner_size(392.0, 368.0)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .visible(false)
    .build()
    .map_err(|e| e.to_string())?;

    if let Some((x, _)) = compute_floating_origin(app, 392.0, 368.0, 0) {
        if let Some(rect_top_bottom) = {
            #[cfg(windows)]
            {
                crate::island::monitor_notch_rect(app, crate::island::Mode::Idle)
                    .map(|(_, t, _, b, s)| (t, b, s))
            }
            #[cfg(not(windows))]
            {
                None::<(i32, i32, f64)>
            }
        } {
            let (t, b, scale) = rect_top_bottom;
            let _ = win.set_position(PhysicalPosition::new(
                x,
                (b - t).max((40.0 * scale).round() as i32) + t + (12.0 * scale).round() as i32,
            ));
        }
    }
    #[cfg(windows)]
    crate::windows::set_webview_transparent_bg(&win);
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_focus();
    #[cfg(windows)]
    {
        let target = win.clone();
        let _ = win.run_on_main_thread(move || {
            crate::windows::set_webview_transparent_bg(&target);
            let _ = crate::windows::enforce_frameless_popup(&target, true);
            let _ = crate::windows::apply_rounded_window_region(&target, 18.0);
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn open_floating_update(app: AppHandle) -> Result<(), String> {
    open_floating_update_window(&app)
}

#[tauri::command]
pub async fn open_floating_note(
    app: AppHandle,
    tools: tauri::State<'_, Arc<ToolsState>>,
    id: Option<String>,
) -> Result<String, String> {
    let note_id = match id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(existing) => existing.to_string(),
        None => tools.create_empty_note()?.id,
    };

    let safe = sanitize_window_key(&note_id);
    let label = format!("float-note-{safe}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.set_focus();
        #[cfg(windows)]
        {
            let target = existing.clone();
            let _ = existing.run_on_main_thread(move || {
                crate::windows::set_webview_transparent_bg(&target);
                let _ = crate::windows::enforce_frameless_popup(&target, true);
                let _ = crate::windows::apply_rounded_window_region(&target, 16.0);
            });
        }
        return Ok(note_id);
    }

    let open_note_count = app
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("float-note-"))
        .count();
    if open_note_count >= MAX_OPEN_FLOATING_NOTES {
        return Err(format!(
            "Close one of the {MAX_OPEN_FLOATING_NOTES} floating notes first"
        ));
    }

    let encoded_id = percent_encode_component(&note_id);
    let url = format!("index.html#float-note?id={encoded_id}");
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title("Peek Note")
        .additional_browser_args(crate::PEEK_BROWSER_ARGS)
        .inner_size(280.0, 248.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;

    setup_floating_frame(&app, &win, 280.0, 248.0, open_note_count as i32)?;
    Ok(note_id)
}

#[tauri::command]
pub fn close_floating_window(window: WebviewWindow) -> Result<(), String> {
    ensure_floating_window(&window)?;
    window.close().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn start_floating_drag(window: WebviewWindow) -> Result<(), String> {
    ensure_floating_window(&window)?;
    #[cfg(windows)]
    return crate::windows::begin_window_drag(&window);
    #[cfg(not(windows))]
    Ok(())
}

#[tauri::command]
pub fn start_floating_resize(window: WebviewWindow, corner: String) -> Result<(), String> {
    ensure_floating_window(&window)?;
    if !matches!(corner.as_str(), "se" | "sw" | "ne" | "nw") {
        return Err("Unsupported resize corner".into());
    }
    #[cfg(windows)]
    {
        if !crate::windows::left_mouse_down() {
            return Ok(());
        }
        let orig = crate::windows::window_rect(&window)?;
        let Some((start_x, start_y)) = crate::windows::cursor_position() else {
            return Ok(());
        };
        let scale = window.scale_factor().unwrap_or(1.0);
        if FLOATING_RESIZE_ACTIVE.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        tauri::async_runtime::spawn(async move {
            let _guard = FloatingResizeGuard;
            let min_w = (200.0 * scale).round() as i32;
            let max_w = (960.0 * scale).round() as i32;
            let min_h = (132.0 * scale).round() as i32;
            let max_h = (640.0 * scale).round() as i32;
            let mut last = (orig.0, orig.1, orig.2 - orig.0, orig.3 - orig.1);

            while crate::windows::left_mouse_down() {
                if let Some((cx, cy)) = crate::windows::cursor_position() {
                    let next = compute_resized_rect(
                        orig,
                        &corner,
                        cx - start_x,
                        cy - start_y,
                        min_w,
                        max_w,
                        min_h,
                        max_h,
                    );
                    if next != last {
                        last = next;
                        let target = window.clone();
                        let (nx, ny, nw, nh) = next;
                        let _ = window.run_on_main_thread(move || {
                            let _ = crate::windows::set_rounded_window_rect(
                                &target, nx, ny, nw, nh, scale, 16.0,
                            );
                        });
                    }
                }
                tokio::time::sleep(Duration::from_millis(8)).await;
            }
        });
    }
    #[cfg(not(windows))]
    let _ = corner;
    Ok(())
}

#[tauri::command]
pub async fn resize_floating_window(
    window: WebviewWindow,
    width: f64,
    height: f64,
) -> Result<(), String> {
    ensure_floating_window(&window)?;
    let w = width.clamp(200.0, 960.0);
    let h = height.clamp(132.0, 640.0);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            let res = target
                .set_size(LogicalSize::new(w, h))
                .map_err(|e| e.to_string());
            #[cfg(windows)]
            let res = res.and_then(|_| crate::windows::apply_rounded_window_region(&target, 16.0));
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn set_floating_always_on_top(
    window: WebviewWindow,
    always_on_top: bool,
) -> Result<(), String> {
    ensure_floating_window(&window)?;
    window
        .set_always_on_top(always_on_top)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_camera_snapshot(data_url: String) -> Result<String, String> {
    let b64 = data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or_else(|| "Expected PNG snapshot payload".to_string())?;
    if b64.is_empty() || b64.len() > MAX_SNAPSHOT_B64_BYTES {
        return Err("Snapshot size out of range".into());
    }
    let png_bytes = STANDARD
        .decode(b64)
        .map_err(|e| format!("Invalid PNG payload: {e}"))?;

    tokio::task::spawn_blocking(move || {
        let dyn_img = image::load_from_memory_with_format(&png_bytes, ImageFormat::Png)
            .map_err(|e| format!("Could not decode snapshot: {e}"))?;
        let rgba = dyn_img.into_rgba8();
        let dir = crate::capture::pictures_dir()?;
        let filename = format!("Peek-Cam-{}.png", crate::capture::local_timestamp());
        let path = dir.join(&filename);
        fs::write(&path, &png_bytes).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let _ = crate::windows::copy_rgba_to_clipboard(rgba.width(), rgba.height(), rgba.as_raw());
        Ok(filename)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_floating_window_keys_and_bounds() {
        assert_eq!(sanitize_window_key("note-123_abc"), "note-123_abc");
        assert_eq!(sanitize_window_key("../evil?x=1&y=2"), "evilx1y2");
        assert_eq!(sanitize_window_key("!!!"), "item");
        let long = "a".repeat(64);
        assert_eq!(sanitize_window_key(&long).len(), 32);
    }

    #[test]
    fn computes_corner_resized_rects_with_clamped_bounds() {
        let orig = (500, 300, 860, 536); // 360x236
        assert_eq!(
            compute_resized_rect(orig, "se", 40, 20, 200, 960, 132, 640),
            (500, 300, 400, 256)
        );
        // SW anchors right edge (860) and top (300)
        assert_eq!(
            compute_resized_rect(orig, "sw", -40, 30, 200, 960, 132, 640),
            (460, 300, 400, 266)
        );
        // NE anchors left (500) and bottom edge (536)
        assert_eq!(
            compute_resized_rect(orig, "ne", 25, -44, 200, 960, 132, 640),
            (500, 256, 385, 280)
        );
        // NW anchors right (860) & bottom (536) and clamps to min bounds
        assert_eq!(
            compute_resized_rect(orig, "nw", 500, 500, 200, 960, 132, 640),
            (660, 404, 200, 132)
        );
    }

    #[test]
    fn normalizes_camera_enhance_mode() {
        assert_eq!(normalize_enhance_mode(Some("natural")), "natural");
        assert_eq!(normalize_enhance_mode(Some("STUDIO")), "studio");
        assert_eq!(normalize_enhance_mode(Some("off")), "off");
        assert_eq!(normalize_enhance_mode(Some("unknown")), "off");
        assert_eq!(normalize_enhance_mode(None), "off");
    }
}
