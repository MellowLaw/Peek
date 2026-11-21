use base64::{engine::general_purpose::STANDARD, Engine};
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    ImageEncoder, RgbaImage,
};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use windows_core::Interface;

use crate::activity::{ActivityManager, OngoingKind, Priority};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CaptureResult {
    Image {
        path: String,
        thumbnail: String,
        width: u32,
        height: u32,
    },
    Ocr {
        text: String,
    },
    Color {
        hex: String,
        rgb: String,
        r: u8,
        g: u8,
        b: u8,
    },
    Video {
        path: String,
    },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayPayload {
    pub mode: String,
    pub preview_data_url: String,
    pub virtual_width: u32,
    pub virtual_height: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    pub active: bool,
    pub paused: bool,
    pub elapsed_seconds: u64,
    pub ffmpeg_available: bool,
    pub mode: String,
}

struct OverlaySession {
    mode: String,
    vx: i32,
    vy: i32,
    vw: u32,
    vh: u32,
    add_to_shelf: bool,
    include_mic: bool,
    mic_device: String,
    image: RgbaImage,
    preview_data_url: String,
}

struct ActiveSegment {
    child: Child,
    started_at: std::time::Instant,
}

struct RecordingSession {
    temp_dir: PathBuf,
    output_path: PathBuf,
    segments: Vec<PathBuf>,
    active_segment: Option<ActiveSegment>,
    accumulated_secs: u64,
    paused: bool,
    mode: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    include_mic: bool,
    mic_device: String,
    add_to_shelf: bool,
    ffmpeg_path: PathBuf,
}

pub struct CaptureState {
    overlay: Mutex<Option<OverlaySession>>,
    recording: Arc<Mutex<Option<RecordingSession>>>,
}

impl Default for CaptureState {
    fn default() -> Self {
        Self {
            overlay: Mutex::new(None),
            recording: Arc::new(Mutex::new(None)),
        }
    }
}

impl CaptureState {
    pub fn is_overlay_active(&self) -> bool {
        self.overlay.lock().unwrap().is_some()
    }
}

fn local_timestamp() -> String {
    #[cfg(windows)]
    unsafe {
        let st = windows::Win32::System::SystemInformation::GetLocalTime();
        return format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        );
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("{secs}")
    }
}

fn pictures_dir() -> Result<PathBuf, String> {
    let base = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("Pictures").join("Peek");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn videos_dir() -> Result<PathBuf, String> {
    let base = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("Videos").join("Peek");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn encode_png_bytes(img: &RgbaImage, fast: bool) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let enc = if fast {
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::NoFilter)
    } else {
        PngEncoder::new(&mut bytes)
    };
    enc.write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn rgba_to_data_url(img: &RgbaImage, fast: bool) -> Result<String, String> {
    let bytes = encode_png_bytes(img, fast)?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

fn rgba_thumbnail_url(img: &RgbaImage) -> Result<String, String> {
    let thumb = image::DynamicImage::ImageRgba8(img.clone()).thumbnail(320, 180);
    let mut bytes = Vec::new();
    thumb
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

#[cfg(windows)]
pub fn capture_rect_rgba(x: i32, y: i32, width: i32, height: i32) -> Result<RgbaImage, String> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };

    if width <= 0 || height <= 0 {
        return Err("Capture dimensions must be positive".into());
    }
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.0.is_null() {
            return Err("Could not acquire screen device context".into());
        }
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        if mem_dc.0.is_null() {
            let _ = ReleaseDC(None, screen_dc);
            return Err("Could not create compatible DC".into());
        }
        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        if bitmap.0.is_null() {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
            return Err("Could not allocate capture bitmap".into());
        }
        let prev = SelectObject(mem_dc, HGDIOBJ(bitmap.0));
        let blt = BitBlt(
            mem_dc,
            0,
            0,
            width,
            height,
            Some(screen_dc),
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        );

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        let lines = if blt.is_ok() {
            GetDIBits(
                mem_dc,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };

        SelectObject(mem_dc, prev);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);

        if blt.is_err() || lines == 0 {
            return Err("Screen capture failed".into());
        }

        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        RgbaImage::from_raw(width as u32, height as u32, pixels)
            .ok_or_else(|| "Could not assemble captured pixels".into())
    }
}

pub fn map_ratio_rect(
    vw: u32,
    vh: u32,
    x_ratio: f64,
    y_ratio: f64,
    w_ratio: f64,
    h_ratio: f64,
) -> Result<(u32, u32, u32, u32), String> {
    if vw == 0 || vh == 0 {
        return Err("Empty virtual desktop bounds".into());
    }
    let rx = x_ratio.clamp(0.0, 1.0);
    let ry = y_ratio.clamp(0.0, 1.0);
    let rw = w_ratio.clamp(0.0, 1.0 - rx);
    let rh = h_ratio.clamp(0.0, 1.0 - ry);

    let px = (rx * vw as f64).round() as u32;
    let py = (ry * vh as f64).round() as u32;
    let pw = (rw * vw as f64).round() as u32;
    let ph = (rh * vh as f64).round() as u32;
    if pw < 4 || ph < 4 {
        return Err("Selected region is too small".into());
    }
    let clamped_w = pw.min(vw.saturating_sub(px));
    let clamped_h = ph.min(vh.saturating_sub(py));
    Ok((px, py, clamped_w, clamped_h))
}

pub fn format_pixel_color(r: u8, g: u8, b: u8) -> (String, String) {
    (
        format!("#{:02X}{:02X}{:02X}", r, g, b),
        format!("rgb({r}, {g}, {b})"),
    )
}

fn finalize_screenshot(
    app: &AppHandle,
    img: &RgbaImage,
    add_to_shelf: bool,
) -> Result<CaptureResult, String> {
    let dir = pictures_dir()?;
    let path = dir.join(format!("Peek-{}.png", local_timestamp()));
    let png_bytes = encode_png_bytes(img, false)?;
    fs::write(&path, &png_bytes).map_err(|e| e.to_string())?;

    #[cfg(windows)]
    let _ = crate::windows::copy_rgba_to_clipboard(img.width(), img.height(), img.as_raw());

    if add_to_shelf {
        crate::shelf::ingest_dropped_paths(app, std::slice::from_ref(&path));
    }

    let path_str = path.to_string_lossy().to_string();
    let thumbnail = rgba_thumbnail_url(img)?;
    let res = CaptureResult::Image {
        path: path_str,
        thumbnail,
        width: img.width(),
        height: img.height(),
    };
    app.state::<ActivityManager>().push_activity(
        "Capture",
        "Screenshot saved",
        "Copied to clipboard",
        Priority::Normal,
        3500,
        Some("capture"),
        false,
        false,
    );
    let _ = app.emit("capture-result", &res);
    Ok(res)
}

#[cfg(windows)]
async fn run_ocr_png(png_bytes: Vec<u8>) -> Result<String, String> {
    tokio::task::spawn_blocking(move || tauri::async_runtime::block_on(ocr_png_bytes(png_bytes)))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(windows)]
async fn ocr_png_bytes(png_bytes: Vec<u8>) -> Result<String, String> {
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::{DataWriter, IOutputStream, InMemoryRandomAccessStream};

    let stream = InMemoryRandomAccessStream::new().map_err(|e| e.to_string())?;
    let out_stream: IOutputStream = stream.cast().map_err(|e| e.to_string())?;
    let writer = DataWriter::CreateDataWriter(&out_stream).map_err(|e| e.to_string())?;
    writer.WriteBytes(&png_bytes).map_err(|e| e.to_string())?;
    writer
        .StoreAsync()
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;
    let _ = writer.DetachStream();
    stream.Seek(0).map_err(|e| e.to_string())?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| "Windows OCR language pack unavailable".to_string())?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;

    if let Ok(lines) = result.Lines() {
        let mut collected = Vec::new();
        for line in lines {
            if let Ok(text) = line.Text() {
                let s = text.to_string_lossy();
                if !s.trim().is_empty() {
                    collected.push(s);
                }
            }
        }
        if !collected.is_empty() {
            return Ok(collected.join("\n"));
        }
    }
    Ok(result
        .Text()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default())
}

fn resolve_monitor_rect(
    app: &AppHandle,
    monitor_name: &str,
) -> Result<(i32, i32, i32, i32), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window missing".to_string())?;
    let chosen = if !monitor_name.is_empty() {
        main.available_monitors()
            .ok()
            .and_then(|list| {
                list.into_iter()
                    .find(|m| m.name().map(|n| n.as_str()) == Some(monitor_name))
            })
            .or_else(|| main.primary_monitor().ok().flatten())
    } else {
        main.primary_monitor().ok().flatten()
    }
    .ok_or_else(|| "No monitor available".to_string())?;
    let pos = chosen.position();
    let sz = chosen.size();
    Ok((pos.x, pos.y, sz.width as i32, sz.height as i32))
}

#[cfg(windows)]
fn resolve_active_window_rect(app: &AppHandle) -> Result<(i32, i32, i32, i32), String> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let peek_hwnds = crate::windows::peek_hwnds(app);
    let raw_hwnd = app
        .state::<crate::windows::ForegroundTracker>()
        .best_external(&peek_hwnds)
        .ok_or_else(|| "No active window found to capture".to_string())?;
    let hwnd = HWND(raw_hwnd as _);
    let mut rect = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_err()
        {
            GetWindowRect(hwnd, &mut rect).map_err(|e| e.to_string())?;
        }
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return Err("Target window has empty dimensions".into());
    }
    Ok((rect.left, rect.top, w, h))
}

#[tauri::command]
pub async fn capture_full_display(
    app: AppHandle,
    monitor_name: String,
    add_to_shelf: bool,
) -> Result<CaptureResult, String> {
    let (x, y, w, h) = resolve_monitor_rect(&app, &monitor_name)?;
    let main = app.get_webview_window("main");
    let was_visible = main
        .as_ref()
        .and_then(|m| m.is_visible().ok())
        .unwrap_or(false);
    if let Some(m) = &main {
        let _ = m.hide();
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    #[cfg(windows)]
    let img = capture_rect_rgba(x, y, w, h);
    #[cfg(not(windows))]
    let img: Result<RgbaImage, String> = Err("Windows only".into());
    if was_visible {
        if let Some(m) = &main {
            let _ = crate::island::show_island_window(m);
        }
    }
    let img = img?;
    finalize_screenshot(&app, &img, add_to_shelf)
}

#[tauri::command]
pub async fn capture_active_window(
    app: AppHandle,
    add_to_shelf: bool,
) -> Result<CaptureResult, String> {
    #[cfg(windows)]
    let (x, y, w, h) = resolve_active_window_rect(&app)?;
    #[cfg(not(windows))]
    let (x, y, w, h) = (0, 0, 0, 0);

    let main = app.get_webview_window("main");
    let was_visible = main
        .as_ref()
        .and_then(|m| m.is_visible().ok())
        .unwrap_or(false);
    if let Some(m) = &main {
        let _ = m.hide();
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    #[cfg(windows)]
    let img = capture_rect_rgba(x, y, w, h);
    #[cfg(not(windows))]
    let img: Result<RgbaImage, String> = Err("Windows only".into());
    if was_visible {
        if let Some(m) = &main {
            let _ = crate::island::show_island_window(m);
        }
    }
    let img = img?;
    finalize_screenshot(&app, &img, add_to_shelf)
}

#[tauri::command]
pub async fn begin_screen_overlay(
    app: AppHandle,
    mode: String,
    add_to_shelf: bool,
    include_mic: bool,
    mic_device: String,
) -> Result<(), String> {
    #[cfg(windows)]
    let (vx, vy, vw, vh) = unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    #[cfg(not(windows))]
    let (vx, vy, vw, vh) = (0, 0, 0, 0);

    let main = app.get_webview_window("main");
    if let Some(m) = &main {
        let _ = m.hide();
    }
    tokio::time::sleep(Duration::from_millis(120)).await;

    #[cfg(windows)]
    let img = match capture_rect_rgba(vx, vy, vw, vh) {
        Ok(i) => i,
        Err(e) => {
            if let Some(m) = &main {
                let _ = crate::island::show_island_window(m);
            }
            return Err(e);
        }
    };
    #[cfg(not(windows))]
    return Err("Windows only".into());

    let preview_data_url = rgba_to_data_url(&img, true)?;
    let payload = OverlayPayload {
        mode: mode.clone(),
        preview_data_url: preview_data_url.clone(),
        virtual_width: vw as u32,
        virtual_height: vh as u32,
    };

    *app.state::<CaptureState>().overlay.lock().unwrap() = Some(OverlaySession {
        mode,
        vx,
        vy,
        vw: vw as u32,
        vh: vh as u32,
        add_to_shelf,
        include_mic,
        mic_device,
        image: img,
        preview_data_url,
    });

    let overlay = if let Some(existing) = app.get_webview_window("capture-overlay") {
        existing
    } else {
        WebviewWindowBuilder::new(
            &app,
            "capture-overlay",
            WebviewUrl::App("index.html#capture-overlay".into()),
        )
        .title("Peek Capture")
        .decorations(false)
        .transparent(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?
    };

    #[cfg(windows)]
    crate::windows::set_window_bounds_raw(&overlay, vx, vy, vw, vh)?;
    let _ = overlay.show();
    let _ = overlay.set_focus();
    let _ = overlay.emit("overlay-ready", payload);
    Ok(())
}

#[tauri::command]
pub fn get_overlay_state(state: tauri::State<'_, CaptureState>) -> Option<OverlayPayload> {
    state
        .overlay
        .lock()
        .unwrap()
        .as_ref()
        .map(|o| OverlayPayload {
            mode: o.mode.clone(),
            preview_data_url: o.preview_data_url.clone(),
            virtual_width: o.vw,
            virtual_height: o.vh,
        })
}

fn close_overlay_restore_main(app: &AppHandle) {
    if let Some(ov) = app.get_webview_window("capture-overlay") {
        let _ = ov.hide();
    }
    if let Some(m) = app.get_webview_window("main") {
        let _ = crate::island::show_island_window(&m);
    }
}

#[tauri::command]
pub async fn complete_overlay_region(
    app: AppHandle,
    x_ratio: f64,
    y_ratio: f64,
    w_ratio: f64,
    h_ratio: f64,
) -> Result<Option<CaptureResult>, String> {
    let session = app
        .state::<CaptureState>()
        .overlay
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "No active capture overlay".to_string())?;
    close_overlay_restore_main(&app);

    let (px, py, pw, ph) =
        map_ratio_rect(session.vw, session.vh, x_ratio, y_ratio, w_ratio, h_ratio)?;

    if session.mode == "record-region" {
        let even_w = (pw & !1).max(16);
        let even_h = (ph & !1).max(16);
        start_recording_rect(
            &app,
            "region",
            session.vx + px as i32,
            session.vy + py as i32,
            even_w,
            even_h,
            session.include_mic,
            session.mic_device,
            session.add_to_shelf,
        )?;
        return Ok(None);
    }

    let cropped = image::imageops::crop_imm(&session.image, px, py, pw, ph).to_image();
    if session.mode == "ocr" {
        let png = encode_png_bytes(&cropped, false)?;
        #[cfg(windows)]
        let text = run_ocr_png(png).await?;
        #[cfg(not(windows))]
        let text = String::new();
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            #[cfg(windows)]
            let _ = crate::windows::copy_text_to_clipboard(&trimmed);
        }
        let res = CaptureResult::Ocr {
            text: if trimmed.is_empty() {
                "No text detected".into()
            } else {
                trimmed
            },
        };
        app.state::<ActivityManager>().push_activity(
            "Capture",
            "OCR complete",
            "Text copied to clipboard",
            Priority::Normal,
            3500,
            Some("capture"),
            false,
            false,
        );
        let _ = app.emit("capture-result", &res);
        return Ok(Some(res));
    }

    let res = finalize_screenshot(&app, &cropped, session.add_to_shelf)?;
    Ok(Some(res))
}

#[tauri::command]
pub async fn complete_overlay_color(
    app: AppHandle,
    x_ratio: f64,
    y_ratio: f64,
) -> Result<CaptureResult, String> {
    let session = app
        .state::<CaptureState>()
        .overlay
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "No active color picker".to_string())?;
    close_overlay_restore_main(&app);

    let px = ((x_ratio.clamp(0.0, 1.0) * (session.vw.saturating_sub(1)) as f64).round() as u32)
        .min(session.vw.saturating_sub(1));
    let py = ((y_ratio.clamp(0.0, 1.0) * (session.vh.saturating_sub(1)) as f64).round() as u32)
        .min(session.vh.saturating_sub(1));
    let pixel = session.image.get_pixel(px, py);
    let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
    let (hex, rgb) = format_pixel_color(r, g, b);

    #[cfg(windows)]
    let _ = crate::windows::copy_text_to_clipboard(&hex);

    let res = CaptureResult::Color {
        hex: hex.clone(),
        rgb,
        r,
        g,
        b,
    };
    app.state::<ActivityManager>().push_activity(
        "Capture",
        "Color copied",
        hex,
        Priority::Normal,
        3000,
        Some("capture"),
        false,
        false,
    );
    let _ = app.emit("capture-result", &res);
    Ok(res)
}

#[tauri::command]
pub fn cancel_overlay(app: AppHandle) {
    app.state::<CaptureState>().overlay.lock().unwrap().take();
    close_overlay_restore_main(&app);
}

#[tauri::command]
pub async fn ocr_image_file(app: AppHandle, path: String) -> Result<CaptureResult, String> {
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let png = encode_png_bytes(&img, false)?;
    #[cfg(windows)]
    let text = run_ocr_png(png).await?;
    #[cfg(not(windows))]
    let text = String::new();
    let trimmed = text.trim().to_string();
    if !trimmed.is_empty() {
        #[cfg(windows)]
        let _ = crate::windows::copy_text_to_clipboard(&trimmed);
    }
    let res = CaptureResult::Ocr {
        text: if trimmed.is_empty() {
            "No text detected".into()
        } else {
            trimmed
        },
    };
    let _ = app.emit("capture-result", &res);
    Ok(res)
}

fn is_allowed_capture_path(candidate: &Path, pictures: &Path, videos: &Path) -> bool {
    let Ok(canon_candidate) = fs::canonicalize(candidate) else {
        return false;
    };
    let in_pictures = fs::canonicalize(pictures)
        .map(|root| canon_candidate.starts_with(root))
        .unwrap_or(false);
    let in_videos = fs::canonicalize(videos)
        .map(|root| canon_candidate.starts_with(root))
        .unwrap_or(false);
    in_pictures || in_videos
}

#[tauri::command]
pub fn reveal_capture_file(path: String) -> Result<(), String> {
    let pb = PathBuf::from(&path);
    if !pb.exists() {
        return Err("Capture file missing".into());
    }
    let pic = pictures_dir()?;
    let vid = videos_dir()?;
    if !is_allowed_capture_path(&pb, &pic, &vid) {
        return Err("Capture path must be inside Pictures\\Peek or Videos\\Peek".into());
    }
    Command::new("explorer.exe")
        .arg(format!("/select,{}", pb.display()))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn open_recordings_folder() -> Result<(), String> {
    let dir = videos_dir()?;
    Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    let direct = Command::new("ffmpeg")
        .arg("-version")
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if direct.is_ok_and(|s| s.success()) {
        return Some(PathBuf::from("ffmpeg"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let winget_dir = PathBuf::from(local)
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        if let Ok(entries) = fs::read_dir(winget_dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().contains("FFmpeg") {
                    if let Ok(sub) = fs::read_dir(entry.path()) {
                        for inner in sub.flatten() {
                            let exe = inner.path().join("bin").join("ffmpeg.exe");
                            if exe.is_file() {
                                return Some(exe);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

#[tauri::command]
pub fn list_audio_inputs() -> Vec<String> {
    let Some(ffmpeg) = find_ffmpeg() else {
        return Vec::new();
    };
    let Ok(output) = Command::new(ffmpeg)
        .args(["-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stderr);
    let mut devices = Vec::new();
    for line in text.lines() {
        if line.contains("(audio)") {
            if let Some(start) = line.find('"') {
                if let Some(end) = line[start + 1..].find('"') {
                    let name = line[start + 1..start + 1 + end].to_string();
                    if !devices.contains(&name) {
                        devices.push(name);
                    }
                }
            }
        }
    }
    devices
}

fn format_mm_ss(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn spawn_ffmpeg_segment(
    ffmpeg: &Path,
    out_file: &Path,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    include_mic: bool,
    mic_device: &str,
) -> Result<Child, String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "-y",
            "-f",
            "gdigrab",
            "-framerate",
            "30",
            "-offset_x",
            &x.to_string(),
            "-offset_y",
            &y.to_string(),
            "-video_size",
            &format!("{w}x{h}"),
            "-i",
            "desktop",
        ]);
    if include_mic && !mic_device.trim().is_empty() {
        cmd.args([
            "-f",
            "dshow",
            "-i",
            &format!("audio={mic_device}"),
            "-c:a",
            "aac",
        ]);
    }
    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
    ])
    .arg(out_file);

    cmd.spawn()
        .map_err(|e| format!("Could not start screen recording: {e}"))
}

fn finish_child_segment(mut active: ActiveSegment) -> u64 {
    let elapsed = active.started_at.elapsed().as_secs();
    if let Some(mut stdin) = active.child.stdin.take() {
        let _ = stdin.write_all(b"q\n");
        let _ = stdin.flush();
    }
    for _ in 0..50 {
        if let Ok(Some(_)) = active.child.try_wait() {
            return elapsed;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    let _ = active.child.kill();
    let _ = active.child.wait();
    elapsed
}

fn start_recording_rect(
    app: &AppHandle,
    mode: &str,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    include_mic: bool,
    mic_device: String,
    add_to_shelf: bool,
) -> Result<(), String> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        "FFmpeg was not found on this PC. Install FFmpeg to enable screen recording.".to_string()
    })?;
    let state = app.state::<CaptureState>();
    {
        if state.recording.lock().unwrap().is_some() {
            return Err("A recording is already active".into());
        }
    }

    let out_dir = videos_dir()?;
    let stamp = local_timestamp();
    let output_path = out_dir.join(format!("Peek-{stamp}.mp4"));
    let temp_dir = std::env::temp_dir().join(format!("peek-rec-{stamp}"));
    fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    let seg_path = temp_dir.join("seg_000.mp4");

    let even_w = (width & !1).max(16);
    let even_h = (height & !1).max(16);
    let child = spawn_ffmpeg_segment(
        &ffmpeg,
        &seg_path,
        x,
        y,
        even_w,
        even_h,
        include_mic,
        &mic_device,
    )?;

    let session = RecordingSession {
        temp_dir,
        output_path,
        segments: vec![seg_path],
        active_segment: Some(ActiveSegment {
            child,
            started_at: std::time::Instant::now(),
        }),
        accumulated_secs: 0,
        paused: false,
        mode: mode.into(),
        x,
        y,
        width: even_w,
        height: even_h,
        include_mic,
        mic_device,
        add_to_shelf,
        ffmpeg_path: ffmpeg,
    };
    *state.recording.lock().unwrap() = Some(session);

    let act = app.state::<ActivityManager>();
    act.set_ongoing(OngoingKind::Recording, "Recording", "00:00", false);

    let rec_arc = state.recording.clone();
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let status_snapshot = {
                let guard = rec_arc.lock().unwrap();
                let Some(rec) = guard.as_ref() else {
                    break;
                };
                let cur_secs = rec
                    .active_segment
                    .as_ref()
                    .map(|a| a.started_at.elapsed().as_secs())
                    .unwrap_or(0);
                (
                    rec.paused,
                    rec.accumulated_secs + cur_secs,
                    rec.mode.clone(),
                )
            };
            let (paused, secs, rec_mode) = status_snapshot;
            app_handle.state::<ActivityManager>().set_ongoing(
                OngoingKind::Recording,
                if paused { "Paused" } else { "Recording" },
                format_mm_ss(secs),
                paused,
            );
            let _ = app_handle.emit(
                "recording-status",
                RecordingStatus {
                    active: true,
                    paused,
                    elapsed_seconds: secs,
                    ffmpeg_available: true,
                    mode: rec_mode,
                },
            );
        }
    });

    Ok(())
}

#[tauri::command]
pub fn start_recording(
    app: AppHandle,
    mode: String,
    monitor_name: String,
    include_mic: bool,
    mic_device: String,
    add_to_shelf: bool,
) -> Result<(), String> {
    let (x, y, w, h) = match mode.as_str() {
        "window" => {
            #[cfg(windows)]
            {
                resolve_active_window_rect(&app)?
            }
            #[cfg(not(windows))]
            {
                return Err("Windows only".into());
            }
        }
        _ => resolve_monitor_rect(&app, &monitor_name)?,
    };
    start_recording_rect(
        &app,
        &mode,
        x,
        y,
        w as u32,
        h as u32,
        include_mic,
        mic_device,
        add_to_shelf,
    )
}

#[tauri::command]
pub async fn pause_recording(app: AppHandle) -> Result<(), String> {
    let active = {
        let state = app.state::<CaptureState>();
        let mut guard = state.recording.lock().unwrap();
        let rec = guard
            .as_mut()
            .ok_or_else(|| "No active recording".to_string())?;
        if rec.paused {
            return Ok(());
        }
        rec.paused = true;
        rec.active_segment.take()
    };
    if let Some(seg) = active {
        let secs = tokio::task::spawn_blocking(move || finish_child_segment(seg))
            .await
            .map_err(|e| e.to_string())?;
        let state = app.state::<CaptureState>();
        let mut guard = state.recording.lock().unwrap();
        if let Some(rec) = guard.as_mut() {
            rec.accumulated_secs += secs;
            let total = rec.accumulated_secs;
            app.state::<ActivityManager>().set_ongoing(
                OngoingKind::Recording,
                "Paused",
                format_mm_ss(total),
                true,
            );
        }
    }
    Ok(())
}

#[tauri::command]
pub fn resume_recording(app: AppHandle) -> Result<(), String> {
    let state = app.state::<CaptureState>();
    let mut guard = state.recording.lock().unwrap();
    let rec = guard
        .as_mut()
        .ok_or_else(|| "No active recording".to_string())?;
    if !rec.paused {
        return Ok(());
    }
    let idx = rec.segments.len();
    let seg_path = rec.temp_dir.join(format!("seg_{:03}.mp4", idx));
    let child = spawn_ffmpeg_segment(
        &rec.ffmpeg_path,
        &seg_path,
        rec.x,
        rec.y,
        rec.width,
        rec.height,
        rec.include_mic,
        &rec.mic_device,
    )?;
    rec.segments.push(seg_path);
    rec.active_segment = Some(ActiveSegment {
        child,
        started_at: std::time::Instant::now(),
    });
    rec.paused = false;
    let total = rec.accumulated_secs;
    app.state::<ActivityManager>().set_ongoing(
        OngoingKind::Recording,
        "Recording",
        format_mm_ss(total),
        false,
    );
    Ok(())
}

#[tauri::command]
pub async fn stop_recording(app: AppHandle) -> Result<CaptureResult, String> {
    let mut rec = app
        .state::<CaptureState>()
        .recording
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "No active recording".to_string())?;

    app.state::<ActivityManager>()
        .clear_ongoing(OngoingKind::Recording);

    if let Some(active) = rec.active_segment.take() {
        let _ = tokio::task::spawn_blocking(move || finish_child_segment(active))
            .await
            .map_err(|e| e.to_string())?;
    }

    let valid_segments: Vec<PathBuf> = rec
        .segments
        .into_iter()
        .filter(|p| p.exists() && fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false))
        .collect();
    if valid_segments.is_empty() {
        let _ = fs::remove_dir_all(&rec.temp_dir);
        return Err("Recording produced no video frames".into());
    }

    let out_path = rec.output_path.clone();
    let temp_dir = rec.temp_dir.clone();
    let ffmpeg = rec.ffmpeg_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        if valid_segments.len() == 1 {
            if fs::rename(&valid_segments[0], &out_path).is_err() {
                fs::copy(&valid_segments[0], &out_path).map_err(|e| e.to_string())?;
            }
        } else {
            let list_path = temp_dir.join("concat.txt");
            let mut list_body = String::new();
            for seg in &valid_segments {
                let escaped = seg.to_string_lossy().replace('\'', "'\\''");
                list_body.push_str(&format!("file '{}'\n", escaped));
            }
            fs::write(&list_path, list_body).map_err(|e| e.to_string())?;
            let status = Command::new(ffmpeg)
                .creation_flags(CREATE_NO_WINDOW)
                .args([
                    "-y",
                    "-f",
                    "concat",
                    "-safe",
                    "0",
                    "-i",
                    &list_path.to_string_lossy(),
                    "-c",
                    "copy",
                ])
                .arg(&out_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("Could not assemble recording segments".into());
            }
        }
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;

    if rec.add_to_shelf {
        crate::shelf::ingest_dropped_paths(&app, std::slice::from_ref(&rec.output_path));
    }
    let path_str = rec.output_path.to_string_lossy().to_string();
    let res = CaptureResult::Video { path: path_str };
    let _ = app.emit(
        "recording-status",
        RecordingStatus {
            active: false,
            paused: false,
            elapsed_seconds: 0,
            ffmpeg_available: true,
            mode: String::new(),
        },
    );
    app.state::<ActivityManager>().push_activity(
        "Capture",
        "Recording saved",
        "Saved to Videos/Peek",
        Priority::Normal,
        3500,
        Some("capture"),
        false,
        false,
    );
    let _ = app.emit("capture-result", &res);
    Ok(res)
}

#[tauri::command]
pub fn get_recording_status(state: tauri::State<'_, CaptureState>) -> RecordingStatus {
    let guard = state.recording.lock().unwrap();
    if let Some(rec) = guard.as_ref() {
        let cur = rec
            .active_segment
            .as_ref()
            .map(|a| a.started_at.elapsed().as_secs())
            .unwrap_or(0);
        RecordingStatus {
            active: true,
            paused: rec.paused,
            elapsed_seconds: rec.accumulated_secs + cur,
            ffmpeg_available: true,
            mode: rec.mode.clone(),
        }
    } else {
        RecordingStatus {
            active: false,
            paused: false,
            elapsed_seconds: 0,
            ffmpeg_available: find_ffmpeg().is_some(),
            mode: String::new(),
        }
    }
}

pub fn begin_region_from_shortcut(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let _ = begin_screen_overlay(app, "region".into(), false, false, String::new()).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_normalized_ratios_safely_across_virtual_desktop() {
        let (x, y, w, h) = map_ratio_rect(3840, 1080, 0.5, 0.25, 0.25, 0.5).unwrap();
        assert_eq!((x, y, w, h), (1920, 270, 960, 540));

        // Reject tiny accidental clicks
        assert!(map_ratio_rect(1920, 1080, 0.1, 0.1, 0.0001, 0.0001).is_err());
    }

    #[test]
    fn formats_pixel_hex_and_rgb() {
        let (hex, rgb) = format_pixel_color(10, 165, 255);
        assert_eq!(hex, "#0AA5FF");
        assert_eq!(rgb, "rgb(10, 165, 255)");
    }

    #[test]
    fn restricts_reveal_capture_file_to_capture_roots() {
        let root = tempfile::TempDir::new().unwrap();
        let pics = root.path().join("Pictures");
        let vids = root.path().join("Videos");
        let outside = root.path().join("Outside");
        fs::create_dir_all(&pics).unwrap();
        fs::create_dir_all(&vids).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let shot = pics.join("Peek-1.png");
        let rec = vids.join("Peek-1.mp4");
        let secret = outside.join("secret.txt");
        fs::write(&shot, b"png").unwrap();
        fs::write(&rec, b"mp4").unwrap();
        fs::write(&secret, b"nope").unwrap();

        assert!(is_allowed_capture_path(&shot, &pics, &vids));
        assert!(is_allowed_capture_path(&rec, &pics, &vids));
        assert!(!is_allowed_capture_path(&secret, &pics, &vids));
        assert!(!is_allowed_capture_path(
            &pics.join("../Outside/secret.txt"),
            &pics,
            &vids
        ));
    }
}
