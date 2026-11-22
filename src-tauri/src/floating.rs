use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::ImageFormat;
use std::fs;
use std::sync::Arc;
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::preferences::PreferencesState;
use crate::tools::ToolsState;

const MAX_SNAPSHOT_B64_BYTES: usize = 22 * 1024 * 1024;
const MAX_OPEN_FLOATING_NOTES: usize = 5;

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

fn setup_floating_frame(
    app: &AppHandle,
    win: &WebviewWindow,
    width: f64,
    height: f64,
    stagger_idx: i32,
) -> Result<(), String> {
    if let Some((x, y)) = compute_floating_origin(app, width, height, stagger_idx) {
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_focus();

    #[cfg(windows)]
    {
        let target = win.clone();
        let _ = win.run_on_main_thread(move || {
            let _ = crate::windows::enforce_frameless_popup(&target, true);
            let _ = crate::windows::apply_rounded_window_region(&target, 16.0);
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn open_floating_camera(
    app: AppHandle,
    device_id: Option<String>,
    mirrored: Option<bool>,
) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("float-camera") {
        let _ = existing.show();
        let _ = existing.set_focus();
        #[cfg(windows)]
        {
            let target = existing.clone();
            let _ = existing.run_on_main_thread(move || {
                let _ = crate::windows::enforce_frameless_popup(&target, true);
                let _ = crate::windows::apply_rounded_window_region(&target, 16.0);
            });
        }
        return Ok(());
    }

    let dev = percent_encode_component(device_id.as_deref().unwrap_or(""));
    let mir = if mirrored.unwrap_or(true) { "1" } else { "0" };
    let url = format!("index.html#float-camera?device={dev}&mirrored={mir}");

    let win = WebviewWindowBuilder::new(&app, "float-camera", WebviewUrl::App(url.into()))
        .title("Peek Camera")
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
pub async fn resize_floating_window(
    window: WebviewWindow,
    width: f64,
    height: f64,
) -> Result<(), String> {
    ensure_floating_window(&window)?;
    let w = width.clamp(220.0, 640.0);
    let h = height.clamp(150.0, 480.0);

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
}
