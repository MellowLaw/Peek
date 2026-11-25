use base64::{engine::general_purpose::STANDARD, Engine};
use image::{codecs::jpeg::JpegEncoder, ImageFormat, ImageReader};
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    net::UdpSocket,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::activity::{ActivityManager, Priority};
use crate::preferences::write_json_atomic;

const SHELF_FILE: &str = "shelf.json";
const MAX_THUMBNAIL_BYTES: u64 = 25 * 1024 * 1024;
const MAX_SHELF_ITEMS: usize = 200;
const MAX_RECEIVE_BYTES: u64 = 250 * 1024 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfEntry {
    pub id: String,
    pub path: String,
    pub name: String,
    pub extension: String,
    pub size_bytes: u64,
    pub modified_ms: u64,
    pub is_dir: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfItem {
    pub id: String,
    pub path: String,
    pub name: String,
    pub extension: String,
    pub size_bytes: u64,
    pub modified_ms: u64,
    pub is_dir: bool,
    pub exists: bool,
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrTransferInfo {
    pub file_name: String,
    pub url: String,
    pub qr_data_url: String,
    pub expires_in_seconds: u64,
    pub mode: String,
    pub save_dir: Option<String>,
    pub received_count: u32,
    pub last_received: Option<String>,
    pub firewall_blocked: bool,
}

struct ShelfInner {
    entries: Vec<ShelfEntry>,
    thumbs: HashMap<(String, u64), Option<String>>,
    next_id: u64,
    qr_stop: Option<oneshot::Sender<()>>,
    qr_active: Option<(u64, QrTransferInfo)>,
    qr_seq: u64,
}

pub struct ShelfState {
    inner: Mutex<ShelfInner>,
    config_dir: Option<PathBuf>,
}

impl ShelfState {
    pub fn load(config_dir: Option<PathBuf>) -> Self {
        let entries: Vec<ShelfEntry> = config_dir
            .as_ref()
            .and_then(|dir| fs::read(dir.join(SHELF_FILE)).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let next_id = entries.len() as u64 + 1;
        Self {
            inner: Mutex::new(ShelfInner {
                entries,
                thumbs: HashMap::new(),
                next_id,
                qr_stop: None,
                qr_active: None,
                qr_seq: 0,
            }),
            config_dir,
        }
    }

    fn save_entries(&self, entries: &[ShelfEntry]) -> Result<(), String> {
        if let Some(dir) = &self.config_dir {
            write_json_atomic(dir, SHELF_FILE, entries)?;
        }
        Ok(())
    }

    pub fn add_paths(&self, paths: &[PathBuf]) -> Result<(Vec<ShelfItem>, usize), String> {
        let mut added = 0usize;
        let snapshot_entries = {
            let mut guard = self.inner.lock().unwrap();
            for p in paths {
                let Ok(meta) = fs::metadata(p) else {
                    continue;
                };
                let canonical = p.to_string_lossy().to_string();
                if guard
                    .entries
                    .iter()
                    .any(|e| e.path.eq_ignore_ascii_case(&canonical))
                {
                    continue;
                }
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| canonical.clone());
                let extension = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_default();
                let modified_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let seq = guard.next_id;
                guard.next_id += 1;
                guard.entries.insert(
                    0,
                    ShelfEntry {
                        id: format!("sh-{}-{}", modified_ms, seq),
                        path: canonical,
                        name,
                        extension,
                        size_bytes: meta.len(),
                        modified_ms,
                        is_dir: meta.is_dir(),
                    },
                );
                added += 1;
            }
            guard.entries.truncate(MAX_SHELF_ITEMS);
            guard.entries.clone()
        };
        self.save_entries(&snapshot_entries)?;
        Ok((self.build_items(), added))
    }

    pub fn build_items(&self) -> Vec<ShelfItem> {
        let entries = self.inner.lock().unwrap().entries.clone();
        let mut items = Vec::with_capacity(entries.len());
        for entry in entries {
            let path = PathBuf::from(&entry.path);
            let meta = fs::metadata(&path).ok();
            let exists = meta.is_some();
            let (size_bytes, modified_ms, is_dir) = if let Some(m) = meta {
                let m_ms = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(entry.modified_ms);
                (m.len(), m_ms, m.is_dir())
            } else {
                (entry.size_bytes, entry.modified_ms, entry.is_dir)
            };

            let thumbnail = if exists {
                let key = (entry.path.clone(), modified_ms);
                let cached = self.inner.lock().unwrap().thumbs.get(&key).cloned();
                match cached {
                    Some(hit) => hit,
                    None => {
                        let generated = if !is_dir
                            && size_bytes <= MAX_THUMBNAIL_BYTES
                            && is_image_ext(&entry.extension)
                        {
                            make_thumbnail(&path).or_else(|| make_shell_thumbnail(&path))
                        } else {
                            make_shell_thumbnail(&path)
                        };
                        self.inner
                            .lock()
                            .unwrap()
                            .thumbs
                            .insert(key, generated.clone());
                        generated
                    }
                }
            } else {
                None
            };

            items.push(ShelfItem {
                id: entry.id,
                path: entry.path,
                name: entry.name,
                extension: entry.extension,
                size_bytes,
                modified_ms,
                is_dir,
                exists,
                thumbnail,
            });
        }
        items
    }
}

fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif")
}

fn make_thumbnail(path: &Path) -> Option<String> {
    let img = ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumb = img.thumbnail(256, 256);
    let mut png_bytes = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
        .ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(png_bytes)
    ))
}

#[cfg(windows)]
pub(crate) fn extract_shell_icon_data_url(
    path: &Path,
    size_px: i32,
    icon_only: bool,
) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, SIZE};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };

    fn extract_on_sta(path: &Path, size_px: i32, icon_only: bool) -> Option<String> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if hr == RPC_E_CHANGED_MODE {
                let owned = path.to_path_buf();
                return std::thread::spawn(move || extract_on_sta(&owned, size_px, icon_only))
                    .join()
                    .ok()
                    .flatten();
            }
            let need_uninit = hr.is_ok();
            let res = (|| -> Option<String> {
                let hpath = HSTRING::from(path.as_os_str());
                let factory: IShellItemImageFactory =
                    SHCreateItemFromParsingName(&hpath, None).ok()?;
                let clamped = size_px.clamp(16, 512);
                let size = SIZE {
                    cx: clamped,
                    cy: clamped,
                };
                let flags = if icon_only {
                    SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK
                } else {
                    SIIGBF_BIGGERSIZEOK
                };
                let hbm = factory
                    .GetImage(size, flags)
                    .or_else(|_| factory.GetImage(size, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK))
                    .ok()?;

                let mut bmp = BITMAP::default();
                let obj_ok = GetObjectW(
                    HGDIOBJ(hbm.0),
                    std::mem::size_of::<BITMAP>() as i32,
                    Some(&mut bmp as *mut _ as *mut _),
                );
                if obj_ok == 0 || bmp.bmWidth <= 0 || bmp.bmHeight <= 0 {
                    let _ = DeleteObject(HGDIOBJ(hbm.0));
                    return None;
                }
                let w = bmp.bmWidth as u32;
                let h = bmp.bmHeight as u32;

                let screen_dc = GetDC(None);
                let mem_dc = if screen_dc.0.is_null() {
                    CreateCompatibleDC(None)
                } else {
                    CreateCompatibleDC(Some(screen_dc))
                };
                let mut bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w as i32,
                        biHeight: -(h as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut pixels = vec![0u8; (w as usize) * (h as usize) * 4];
                let lines = if !mem_dc.0.is_null() {
                    GetDIBits(
                        mem_dc,
                        hbm,
                        0,
                        h,
                        Some(pixels.as_mut_ptr() as *mut _),
                        &mut bmi,
                        DIB_RGB_COLORS,
                    )
                } else {
                    0
                };
                if !mem_dc.0.is_null() {
                    let _ = DeleteDC(mem_dc);
                }
                if !screen_dc.0.is_null() {
                    let _ = ReleaseDC(None, screen_dc);
                }
                let _ = DeleteObject(HGDIOBJ(hbm.0));

                if lines == 0 {
                    return None;
                }

                let mut has_alpha = false;
                for chunk in pixels.chunks_exact_mut(4) {
                    chunk.swap(0, 2); // BGRA -> RGBA
                    if chunk[3] != 0 {
                        has_alpha = true;
                    }
                }
                if !has_alpha {
                    for chunk in pixels.chunks_exact_mut(4) {
                        chunk[3] = 255;
                    }
                } else {
                    for chunk in pixels.chunks_exact_mut(4) {
                        let a = chunk[3] as u32;
                        if a > 0 && a < 255 {
                            chunk[0] = ((chunk[0] as u32 * 255) / a).min(255) as u8;
                            chunk[1] = ((chunk[1] as u32 * 255) / a).min(255) as u8;
                            chunk[2] = ((chunk[2] as u32 * 255) / a).min(255) as u8;
                        }
                    }
                }

                let rgba = image::RgbaImage::from_raw(w, h, pixels)?;
                let mut png_bytes = Vec::new();
                image::DynamicImage::ImageRgba8(rgba)
                    .write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
                    .ok()?;
                Some(format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(png_bytes)
                ))
            })();
            if need_uninit {
                CoUninitialize();
            }
            res
        }
    }

    extract_on_sta(path, size_px, icon_only)
}

#[cfg(windows)]
fn make_shell_thumbnail(path: &Path) -> Option<String> {
    extract_shell_icon_data_url(path, 256, false)
}

#[cfg(not(windows))]
fn make_shell_thumbnail(_path: &Path) -> Option<String> {
    None
}

fn unique_dest_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    let ext = path.extension().and_then(|e| e.to_str());
    for n in 1..1000 {
        let next_name = match ext {
            Some(e) if !e.is_empty() => format!("{stem} ({n}).{e}"),
            _ => format!("{stem} ({n})"),
        };
        let next = dir.join(next_name);
        if !next.exists() {
            return next;
        }
    }
    dir.join(format!("{file_name}.copy"))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn validate_single_filename(new_name: &str) -> Result<String, String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err("Enter a valid filename".into());
    }
    if trimmed.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0']) {
        return Err("Filename contains disallowed characters".into());
    }
    Ok(trimmed.to_string())
}

pub fn rename_path_safe(src: &Path, new_name: &str) -> Result<PathBuf, String> {
    let clean = validate_single_filename(new_name)?;
    let parent = src
        .parent()
        .ok_or_else(|| "File has no parent directory".to_string())?;
    let dest = parent.join(&clean);
    if dest == src {
        return Ok(dest);
    }
    if dest.exists() {
        return Err("A file with that name already exists".into());
    }
    fs::rename(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

fn append_to_zip<W: Write + std::io::Seek>(
    zip: &mut ZipWriter<W>,
    src: &Path,
    archive_prefix: &str,
    options: SimpleFileOptions,
) -> Result<(), String> {
    if src.is_dir() {
        let dir_name = if archive_prefix.ends_with('/') {
            archive_prefix.to_string()
        } else {
            format!("{archive_prefix}/")
        };
        zip.add_directory(&dir_name, options)
            .map_err(|e| e.to_string())?;
        for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let child = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let child_prefix = format!("{archive_prefix}/{name}");
            append_to_zip(zip, &child, &child_prefix, options)?;
        }
    } else {
        zip.start_file(archive_prefix, options)
            .map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(File::open(src).map_err(|e| e.to_string())?);
        std::io::copy(&mut reader, zip).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn create_zip_from_paths(paths: &[PathBuf], dest_zip: &Path) -> Result<(), String> {
    let file = File::create(dest_zip).map_err(|e| e.to_string())?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for p in paths {
        if !p.exists() {
            return Err(format!("Missing source file: {}", p.display()));
        }
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "Invalid source filename".to_string())?;
        append_to_zip(&mut zip, p, name, options)?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn extract_zip_safe(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx).map_err(|e| e.to_string())?;
        let entry_name = entry.name().map(|s| s.into_owned()).unwrap_or_default();
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| format!("Unsafe zip path rejected: {}", entry_name))?;
        let out_path = dest_dir.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = File::create(&out_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn convert_image_sibling(
    src: &Path,
    target_format: &str,
    quality: u8,
) -> Result<PathBuf, String> {
    let ext = match target_format.to_ascii_lowercase().as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "webp" => "webp",
        _ => return Err("Unsupported image format".into()),
    };
    let parent = src
        .parent()
        .ok_or_else(|| "Image has no parent folder".to_string())?;
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted");
    let dest = unique_dest_path(parent, &format!("{stem}_converted.{ext}"));
    let img = ImageReader::open(src)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    match ext {
        "png" => img
            .save_with_format(&dest, ImageFormat::Png)
            .map_err(|e| e.to_string())?,
        "webp" => img
            .save_with_format(&dest, ImageFormat::WebP)
            .map_err(|e| e.to_string())?,
        "jpg" => {
            let rgb = img.to_rgb8();
            let mut out = File::create(&dest).map_err(|e| e.to_string())?;
            let mut enc = JpegEncoder::new_with_quality(&mut out, quality.clamp(30, 100));
            enc.encode_image(&rgb).map_err(|e| e.to_string())?;
        }
        _ => unreachable!(),
    }
    Ok(dest)
}

#[cfg(windows)]
fn open_shell_picker(pick_folders: bool, multi: bool) -> Result<Vec<PathBuf>, String> {
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST,
        FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
    };

    let _suppress = crate::windows::suppress_auto_close();
    unsafe {
        let init_hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let should_uninit = init_hr.is_ok();
        let res = (|| -> Result<Vec<PathBuf>, String> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
            let mut opts = dialog.GetOptions().map_err(|e| e.to_string())?;
            opts |= FOS_PATHMUSTEXIST;
            if pick_folders {
                opts |= FOS_PICKFOLDERS;
            } else {
                opts |= FOS_FILEMUSTEXIST;
                if multi {
                    opts |= FOS_ALLOWMULTISELECT;
                }
            }
            dialog.SetOptions(opts).map_err(|e| e.to_string())?;
            if let Err(err) = dialog.Show(None) {
                if err.code() == windows_core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                    return Ok(Vec::new());
                }
                return Err(err.to_string());
            }
            let mut out = Vec::new();
            if multi && !pick_folders {
                let array = dialog.GetResults().map_err(|e| e.to_string())?;
                let count = array.GetCount().map_err(|e| e.to_string())?;
                for idx in 0..count {
                    let item = array.GetItemAt(idx).map_err(|e| e.to_string())?;
                    let pwstr = item
                        .GetDisplayName(SIGDN_FILESYSPATH)
                        .map_err(|e| e.to_string())?;
                    let path_str = pwstr.to_string().map_err(|e| e.to_string())?;
                    CoTaskMemFree(Some(pwstr.0 as _));
                    out.push(PathBuf::from(path_str));
                }
            } else {
                let item = dialog.GetResult().map_err(|e| e.to_string())?;
                let pwstr = item
                    .GetDisplayName(SIGDN_FILESYSPATH)
                    .map_err(|e| e.to_string())?;
                let path_str = pwstr.to_string().map_err(|e| e.to_string())?;
                CoTaskMemFree(Some(pwstr.0 as _));
                out.push(PathBuf::from(path_str));
            }
            Ok(out)
        })();
        if should_uninit {
            CoUninitialize();
        }
        res
    }
}

pub fn ingest_dropped_paths(app: &AppHandle, raw_paths: &[PathBuf]) {
    if raw_paths.is_empty() {
        return;
    }
    let state = app.state::<ShelfState>();
    if let Ok((items, added)) = state.add_paths(raw_paths) {
        let _ = app.emit("shelf-updated", &items);
        if added > 0 {
            let label = if added == 1 {
                raw_paths[0]
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("File")
                    .to_string()
            } else {
                format!("{added} files")
            };
            app.state::<ActivityManager>().push_activity(
                "Files",
                "File added",
                label,
                Priority::Normal,
                3200,
                Some("files"),
                false,
                false,
            );
        }
    }
}

#[tauri::command]
pub fn get_shelf(state: tauri::State<'_, ShelfState>) -> Vec<ShelfItem> {
    state.build_items()
}

#[tauri::command]
pub fn add_shelf_files(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
    paths: Vec<String>,
) -> Result<Vec<ShelfItem>, String> {
    let pb: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let (items, _) = state.add_paths(&pb)?;
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub async fn pick_shelf_files(app: AppHandle) -> Result<Vec<ShelfItem>, String> {
    #[cfg(windows)]
    let picked = tokio::task::spawn_blocking(|| open_shell_picker(false, true))
        .await
        .map_err(|e| e.to_string())??;
    #[cfg(not(windows))]
    let picked: Vec<PathBuf> = Vec::new();

    let state = app.state::<ShelfState>();
    if picked.is_empty() {
        return Ok(state.build_items());
    }
    let (items, _) = state.add_paths(&picked)?;
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub fn remove_shelf_item(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
    id: String,
) -> Result<Vec<ShelfItem>, String> {
    // Only remove Peek's shelf reference; original files on disk remain untouched.
    let entries = {
        let mut guard = state.inner.lock().unwrap();
        guard.entries.retain(|e| e.id != id);
        guard.entries.clone()
    };
    state.save_entries(&entries)?;
    let items = state.build_items();
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub fn clear_shelf(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
) -> Result<Vec<ShelfItem>, String> {
    // Clearing the shelf drops references only; original files remain intact.
    {
        state.inner.lock().unwrap().entries.clear();
    }
    state.save_entries(&[])?;
    let items = Vec::new();
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub fn reveal_shelf_file(state: tauri::State<'_, ShelfState>, id: String) -> Result<(), String> {
    let path = {
        let guard = state.inner.lock().unwrap();
        guard
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.path.clone())
            .ok_or_else(|| "Shelf item not found".to_string())?
    };
    let pb = PathBuf::from(&path);
    if !pb.exists() {
        return Err("Original file is missing".into());
    }
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", pb.display()))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn collect_entries_by_ids(state: &ShelfState, ids: &[String]) -> Result<Vec<ShelfEntry>, String> {
    let guard = state.inner.lock().unwrap();
    let chosen: Vec<ShelfEntry> = guard
        .entries
        .iter()
        .filter(|e| ids.contains(&e.id))
        .cloned()
        .collect();
    if chosen.is_empty() {
        return Err("Select at least one existing file".into());
    }
    for item in &chosen {
        if !Path::new(&item.path).exists() {
            return Err(format!("Original file missing: {}", item.name));
        }
    }
    Ok(chosen)
}

#[tauri::command]
pub async fn copy_shelf_files(app: AppHandle, ids: Vec<String>) -> Result<String, String> {
    let entries = collect_entries_by_ids(&app.state::<ShelfState>(), &ids)?;
    #[cfg(windows)]
    let dests = tokio::task::spawn_blocking(|| open_shell_picker(true, false))
        .await
        .map_err(|e| e.to_string())??;
    #[cfg(not(windows))]
    let dests: Vec<PathBuf> = Vec::new();
    let Some(dest_dir) = dests.into_iter().next() else {
        return Err("Canceled".into());
    };
    let count = entries.len();
    for item in entries {
        let src = PathBuf::from(&item.path);
        let dst = unique_dest_path(&dest_dir, &item.name);
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        }
    }
    Ok(format!(
        "Copied {} item(s) to {}",
        count,
        dest_dir.display()
    ))
}

#[tauri::command]
pub async fn move_shelf_files(app: AppHandle, ids: Vec<String>) -> Result<Vec<ShelfItem>, String> {
    let entries = collect_entries_by_ids(&app.state::<ShelfState>(), &ids)?;
    #[cfg(windows)]
    let dests = tokio::task::spawn_blocking(|| open_shell_picker(true, false))
        .await
        .map_err(|e| e.to_string())??;
    #[cfg(not(windows))]
    let dests: Vec<PathBuf> = Vec::new();
    let Some(dest_dir) = dests.into_iter().next() else {
        return Err("Canceled".into());
    };

    let mut moved_map: HashMap<String, PathBuf> = HashMap::new();
    for item in entries {
        let src = PathBuf::from(&item.path);
        let dst = unique_dest_path(&dest_dir, &item.name);
        if fs::rename(&src, &dst).is_err() {
            if src.is_dir() {
                copy_dir_recursive(&src, &dst)?;
                fs::remove_dir_all(&src).map_err(|e| e.to_string())?;
            } else {
                fs::copy(&src, &dst).map_err(|e| e.to_string())?;
                fs::remove_file(&src).map_err(|e| e.to_string())?;
            }
        }
        moved_map.insert(item.id, dst);
    }

    let state = app.state::<ShelfState>();
    let updated_entries = {
        let mut guard = state.inner.lock().unwrap();
        for entry in &mut guard.entries {
            if let Some(new_path) = moved_map.get(&entry.id) {
                entry.path = new_path.to_string_lossy().to_string();
                entry.name = new_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| entry.name.clone());
            }
        }
        guard.entries.clone()
    };
    state.save_entries(&updated_entries)?;
    let items = state.build_items();
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub fn rename_shelf_file(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
    id: String,
    new_name: String,
) -> Result<Vec<ShelfItem>, String> {
    let old_path = {
        let guard = state.inner.lock().unwrap();
        guard
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| PathBuf::from(&e.path))
            .ok_or_else(|| "Shelf item not found".to_string())?
    };
    if !old_path.exists() {
        return Err("Original file is missing".into());
    }
    let new_path = rename_path_safe(&old_path, &new_name)?;
    let updated_entries = {
        let mut guard = state.inner.lock().unwrap();
        if let Some(entry) = guard.entries.iter_mut().find(|e| e.id == id) {
            entry.path = new_path.to_string_lossy().to_string();
            entry.name = new_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
                .unwrap_or(new_name);
            entry.extension = new_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
        }
        guard.entries.clone()
    };
    state.save_entries(&updated_entries)?;
    let items = state.build_items();
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub async fn zip_shelf_files(
    app: AppHandle,
    ids: Vec<String>,
    archive_name: String,
) -> Result<Vec<ShelfItem>, String> {
    let state = app.state::<ShelfState>();
    let entries = collect_entries_by_ids(&state, &ids)?;
    let base_name = validate_single_filename(if archive_name.trim().is_empty() {
        "PeekArchive"
    } else {
        archive_name.trim()
    })?;
    let zip_filename = if base_name.to_ascii_lowercase().ends_with(".zip") {
        base_name
    } else {
        format!("{base_name}.zip")
    };
    let first = PathBuf::from(&entries[0].path);
    let parent = first
        .parent()
        .ok_or_else(|| "No parent folder for archive".to_string())?;
    let dest_zip = unique_dest_path(parent, &zip_filename);
    let sources: Vec<PathBuf> = entries.into_iter().map(|e| PathBuf::from(e.path)).collect();
    let dest_clone = dest_zip.clone();
    tokio::task::spawn_blocking(move || create_zip_from_paths(&sources, &dest_clone))
        .await
        .map_err(|e| e.to_string())??;
    let (items, _) = state.add_paths(&[dest_zip])?;
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub async fn extract_shelf_zip(app: AppHandle, id: String) -> Result<Vec<ShelfItem>, String> {
    let state = app.state::<ShelfState>();
    let zip_path = {
        let guard = state.inner.lock().unwrap();
        guard
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| PathBuf::from(&e.path))
            .ok_or_else(|| "Archive not found".to_string())?
    };
    if !zip_path.exists() {
        return Err("Archive file is missing".into());
    }
    let parent = zip_path
        .parent()
        .ok_or_else(|| "Archive has no parent folder".to_string())?;
    let stem = zip_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Extracted");
    let dest_dir = unique_dest_path(parent, stem);
    let dest_clone = dest_dir.clone();
    tokio::task::spawn_blocking(move || extract_zip_safe(&zip_path, &dest_clone))
        .await
        .map_err(|e| e.to_string())??;
    let (items, _) = state.add_paths(&[dest_dir])?;
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[tauri::command]
pub async fn convert_shelf_image(
    app: AppHandle,
    id: String,
    format: String,
    quality: u8,
) -> Result<Vec<ShelfItem>, String> {
    let state = app.state::<ShelfState>();
    let src_path = {
        let guard = state.inner.lock().unwrap();
        guard
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| PathBuf::from(&e.path))
            .ok_or_else(|| "Image not found".to_string())?
    };
    if !src_path.exists() {
        return Err("Original image is missing".into());
    }
    let out_path =
        tokio::task::spawn_blocking(move || convert_image_sibling(&src_path, &format, quality))
            .await
            .map_err(|e| e.to_string())??;
    let (items, _) = state.add_paths(&[out_path])?;
    let _ = app.emit("shelf-updated", &items);
    Ok(items)
}

#[cfg(windows)]
mod ole_drag {
    use windows::core::w;
    use windows::Win32::Foundation::{
        GlobalFree, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
        DV_E_FORMATETC, E_NOTIMPL, HGLOBAL, POINT, S_FALSE, S_OK,
    };
    use windows::Win32::System::Com::{
        IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumFORMATETC_Impl,
        IEnumSTATDATA, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0,
        TYMED_HGLOBAL,
    };
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{
        DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, OleUninitialize, CF_HDROP,
        DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE,
    };
    use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, ReleaseCapture, VK_LBUTTON,
    };
    use windows::Win32::UI::Shell::DROPFILES;
    use windows_core::{implement, Ref, HRESULT};

    #[implement(IEnumFORMATETC)]
    struct HdropEnum {
        index: std::sync::atomic::AtomicUsize,
        prefer_cf: u16,
    }

    impl IEnumFORMATETC_Impl for HdropEnum_Impl {
        fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> HRESULT {
            if celt == 0 || rgelt.is_null() {
                return S_FALSE;
            }
            let formats = [hdrop_format(), preferred_effect_format(self.prefer_cf)];
            let mut fetched = 0u32;
            while fetched < celt {
                let idx = self.index.load(std::sync::atomic::Ordering::SeqCst);
                if idx >= formats.len() {
                    break;
                }
                self.index
                    .store(idx + 1, std::sync::atomic::Ordering::SeqCst);
                unsafe {
                    *rgelt.add(fetched as usize) = formats[idx];
                }
                fetched += 1;
            }
            unsafe {
                if !pceltfetched.is_null() {
                    *pceltfetched = fetched;
                }
            }
            if fetched == celt {
                S_OK
            } else {
                S_FALSE
            }
        }

        fn Skip(&self, celt: u32) -> windows_core::Result<()> {
            let cur = self
                .index
                .fetch_add(celt as usize, std::sync::atomic::Ordering::SeqCst);
            if cur + celt as usize <= 2 {
                Ok(())
            } else {
                Err(windows_core::Error::from(S_FALSE))
            }
        }

        fn Reset(&self) -> windows_core::Result<()> {
            self.index.store(0, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn Clone(&self) -> windows_core::Result<IEnumFORMATETC> {
            Ok(HdropEnum {
                index: std::sync::atomic::AtomicUsize::new(
                    self.index.load(std::sync::atomic::Ordering::SeqCst),
                ),
                prefer_cf: self.prefer_cf,
            }
            .into())
        }
    }

    fn hdrop_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn preferred_effect_format(cf: u16) -> FORMATETC {
        FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn matches_hdrop(fmt: &FORMATETC) -> bool {
        fmt.cfFormat == CF_HDROP.0 && (fmt.tymed & (TYMED_HGLOBAL.0 as u32)) != 0
    }

    fn matches_prefer_effect(fmt: &FORMATETC, prefer_cf: u16) -> bool {
        prefer_cf != 0 && fmt.cfFormat == prefer_cf && (fmt.tymed & (TYMED_HGLOBAL.0 as u32)) != 0
    }

    fn build_hdrop_hglobal(paths: &[String]) -> windows_core::Result<HGLOBAL> {
        let mut wide_chars: Vec<u16> = Vec::new();
        for p in paths {
            wide_chars.extend(p.encode_utf16());
            wide_chars.push(0);
        }
        wide_chars.push(0);

        let header_size = std::mem::size_of::<DROPFILES>();
        let total = header_size + wide_chars.len() * 2;
        unsafe {
            let hmem = GlobalAlloc(GMEM_MOVEABLE, total)?;
            let ptr = GlobalLock(hmem) as *mut u8;
            if ptr.is_null() {
                let _ = GlobalFree(Some(hmem));
                return Err(windows_core::Error::from(DV_E_FORMATETC));
            }
            let header = DROPFILES {
                pFiles: header_size as u32,
                pt: POINT { x: 0, y: 0 },
                fNC: false.into(),
                fWide: true.into(),
            };
            std::ptr::copy_nonoverlapping(&header as *const _ as *const u8, ptr, header_size);
            std::ptr::copy_nonoverlapping(
                wide_chars.as_ptr() as *const u8,
                ptr.add(header_size),
                wide_chars.len() * 2,
            );
            let _ = GlobalUnlock(hmem);
            Ok(hmem)
        }
    }

    fn build_drop_effect_hglobal(prefer_move: bool) -> windows_core::Result<HGLOBAL> {
        let effect_val: u32 = if prefer_move {
            DROPEFFECT_MOVE.0
        } else {
            DROPEFFECT_COPY.0
        };
        unsafe {
            let hmem = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>())?;
            let ptr = GlobalLock(hmem) as *mut u32;
            if ptr.is_null() {
                let _ = GlobalFree(Some(hmem));
                return Err(windows_core::Error::from(DV_E_FORMATETC));
            }
            *ptr = effect_val;
            let _ = GlobalUnlock(hmem);
            Ok(hmem)
        }
    }

    #[implement(IDataObject)]
    struct FileDataObject {
        paths: Vec<String>,
        prefer_move: bool,
        prefer_cf: u16,
    }

    impl IDataObject_Impl for FileDataObject_Impl {
        fn GetData(&self, pformatetcin: *const FORMATETC) -> windows_core::Result<STGMEDIUM> {
            if pformatetcin.is_null() {
                return Err(windows_core::Error::from(DV_E_FORMATETC));
            }
            let fmt = unsafe { &*pformatetcin };
            let hglobal = if matches_hdrop(fmt) {
                build_hdrop_hglobal(&self.paths)?
            } else if matches_prefer_effect(fmt, self.prefer_cf) {
                build_drop_effect_hglobal(self.prefer_move)?
            } else {
                return Err(windows_core::Error::from(DV_E_FORMATETC));
            };
            Ok(STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 { hGlobal: hglobal },
                pUnkForRelease: core::mem::ManuallyDrop::new(None),
            })
        }

        fn GetDataHere(
            &self,
            _pformatetc: *const FORMATETC,
            _pmedium: *mut STGMEDIUM,
        ) -> windows_core::Result<()> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
            if pformatetc.is_null() {
                return DV_E_FORMATETC;
            }
            let fmt = unsafe { &*pformatetc };
            if matches_hdrop(fmt) || matches_prefer_effect(fmt, self.prefer_cf) {
                S_OK
            } else {
                DV_E_FORMATETC
            }
        }

        fn GetCanonicalFormatEtc(
            &self,
            _pformatectin: *const FORMATETC,
            pformatetcout: *mut FORMATETC,
        ) -> HRESULT {
            if !pformatetcout.is_null() {
                unsafe { (*pformatetcout).ptd = std::ptr::null_mut() };
            }
            windows::Win32::Foundation::DATA_S_SAMEFORMATETC
        }

        fn SetData(
            &self,
            _pformatetc: *const FORMATETC,
            _pmedium: *const STGMEDIUM,
            _frelease: windows_core::BOOL,
        ) -> windows_core::Result<()> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn EnumFormatEtc(&self, dwdirection: u32) -> windows_core::Result<IEnumFORMATETC> {
            if dwdirection != DATADIR_GET.0 as u32 {
                return Err(windows_core::Error::from(E_NOTIMPL));
            }
            Ok(HdropEnum {
                index: std::sync::atomic::AtomicUsize::new(0),
                prefer_cf: self.prefer_cf,
            }
            .into())
        }

        fn DAdvise(
            &self,
            _pformatetc: *const FORMATETC,
            _advf: u32,
            _padvsink: Ref<IAdviseSink>,
        ) -> windows_core::Result<u32> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn DUnadvise(&self, _dwconnection: u32) -> windows_core::Result<()> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn EnumDAdvise(&self) -> windows_core::Result<IEnumSTATDATA> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }
    }

    #[implement(IDropSource)]
    struct FileDropSource;

    impl IDropSource_Impl for FileDropSource_Impl {
        fn QueryContinueDrag(
            &self,
            fescapepressed: windows_core::BOOL,
            grfkeystate: MODIFIERKEYS_FLAGS,
        ) -> HRESULT {
            if fescapepressed.as_bool() {
                return DRAGDROP_S_CANCEL;
            }
            if (grfkeystate.0 & MK_LBUTTON.0) == 0 {
                return DRAGDROP_S_DROP;
            }
            S_OK
        }

        fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    pub fn perform_drag_drop(paths: Vec<String>, prefer_move: bool) -> Result<bool, String> {
        let _drag_guard = crate::drop_catcher::begin_internal_drag();
        let _suppress = crate::windows::suppress_auto_close();
        unsafe {
            if (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) == 0 {
                return Ok(false);
            }
            let ole_init = OleInitialize(None).is_ok();
            let prefer_cf = RegisterClipboardFormatW(w!("Preferred DropEffect")) as u16;
            let data_obj: IDataObject = FileDataObject {
                paths,
                prefer_move,
                prefer_cf,
            }
            .into();
            let drop_src: IDropSource = FileDropSource.into();
            let mut effect = DROPEFFECT(0);
            let _ = ReleaseCapture();
            let hr = DoDragDrop(
                &data_obj,
                &drop_src,
                DROPEFFECT_MOVE | DROPEFFECT_COPY | DROPEFFECT_LINK,
                &mut effect,
            );
            if ole_init {
                OleUninitialize();
            }
            if hr == DRAGDROP_S_DROP {
                Ok(true)
            } else if hr == DRAGDROP_S_CANCEL || hr.is_ok() {
                Ok(false)
            } else {
                Err(format!("Drag-and-drop failed: {:?}", hr))
            }
        }
    }
}

#[tauri::command]
pub async fn start_file_drag(
    window: WebviewWindow,
    ids: Vec<String>,
    prefer_move: Option<bool>,
) -> Result<Vec<ShelfItem>, String> {
    let state = window.state::<ShelfState>();
    let entries = collect_entries_by_ids(&state, &ids)?;
    let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
    let prefer_move = prefer_move.unwrap_or(true);

    #[cfg(windows)]
    {
        let (tx, rx) = oneshot::channel();
        window
            .run_on_main_thread(move || {
                let _ = tx.send(ole_drag::perform_drag_drop(paths, prefer_move));
            })
            .map_err(|e| e.to_string())?;
        let dropped = rx.await.map_err(|e| e.to_string())??;
        if dropped {
            // If Windows Explorer moved any dragged file off its original path, clear its shelf entry.
            let moved_ids: Vec<String> = entries
                .iter()
                .filter(|e| !Path::new(&e.path).exists())
                .map(|e| e.id.clone())
                .collect();
            if !moved_ids.is_empty() {
                let remaining = {
                    let mut guard = state.inner.lock().unwrap();
                    guard.entries.retain(|e| !moved_ids.contains(&e.id));
                    guard.entries.clone()
                };
                state.save_entries(&remaining)?;
            }
        }
        let items = state.build_items();
        let _ = window.app_handle().emit("shelf-updated", &items);
        Ok(items)
    }
    #[cfg(not(windows))]
    {
        let _ = (paths, prefer_move);
        Err("OLE drag-out is only available on Windows".into())
    }
}

fn detect_lan_ip() -> String {
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".into()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn sanitize_content_disposition_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_control() || matches!(c, '"' | '\\' | ';') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned
    }
}

fn render_qr_data_url(url: &str) -> Result<String, String> {
    let code = QrCode::new(url.as_bytes()).map_err(|e| e.to_string())?;
    let svg_xml = code
        .render::<svg::Color>()
        .min_dimensions(180, 180)
        .dark_color(svg::Color("#080808"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg_xml.as_bytes())
    ))
}

fn percent_decode_utf8(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn sanitize_upload_filename(raw: &str) -> String {
    let decoded = percent_decode_utf8(raw);
    let base = decoded.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let mut trimmed = cleaned
        .trim_matches(|c: char| c.is_whitespace() || c == '.')
        .to_string();
    if trimmed.is_empty() {
        return "phone-file".into();
    }

    let first_stem = trimmed.split('.').next().unwrap_or("");
    let upper_stem = first_stem.to_ascii_uppercase();
    let is_reserved = matches!(
        upper_stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if is_reserved {
        trimmed.insert(0, '_');
    }

    const MAX_CHARS: usize = 120;
    if trimmed.chars().count() > MAX_CHARS {
        let p = Path::new(&trimmed);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        let ext_part = if !ext.is_empty() && ext.chars().count() <= 16 {
            format!(".{ext}")
        } else {
            String::new()
        };
        let keep_stem = MAX_CHARS.saturating_sub(ext_part.chars().count()).max(8);
        let stem_src = if ext_part.is_empty() {
            trimmed.as_str()
        } else {
            p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(trimmed.as_str())
        };
        let trunc_stem: String = stem_src.chars().take(keep_stem).collect();
        let trunc_stem = trunc_stem.trim_end_matches(|c: char| c.is_whitespace() || c == '.');
        let base_stem = if trunc_stem.is_empty() {
            "phone-file"
        } else {
            trunc_stem
        };
        return format!("{base_stem}{ext_part}");
    }

    trimmed
}

fn received_files_dir() -> Result<PathBuf, String> {
    let base = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("Downloads").join("Peek");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

async fn read_http_headers(socket: &mut tokio::net::TcpStream) -> Option<(Vec<u8>, usize)> {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some((buf, pos + 4));
        }
        if buf.len() >= MAX_HTTP_HEADER_BYTES {
            return None;
        }
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remain.is_zero() {
            return None;
        }
        let Ok(Ok(n)) = tokio::time::timeout(remain, socket.read(&mut tmp)).await else {
            return None;
        };
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

fn render_receive_page(upload_route: &str) -> String {
    const PEEK_ICON_SVG: &str = include_str!("../icons/peek-icon.svg");
    let icon_b64 = STANDARD.encode(PEEK_ICON_SVG.as_bytes());
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1" />
<title>Send to Peek</title>
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml;base64,{icon_b64}" />
<style>
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; padding: 24px 16px; background: #080808; color: #f4f4f4; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  .wrap {{ max-width: 420px; margin: 0 auto; display: grid; gap: 14px; }}
  .card {{ background: #141414; border: 1px solid rgba(255,255,255,0.08); border-radius: 18px; padding: 18px; display: grid; gap: 12px; }}
  .brand-row {{ display: flex; align-items: center; gap: 10px; }}
  .brand-icon {{ width: 28px; height: 28px; flex: none; display: block; }}
  .brand-icon svg {{ width: 100%; height: 100%; display: block; }}
  h1 {{ margin: 0; font-size: 18px; font-weight: 700; letter-spacing: -0.02em; }}
  p {{ margin: 0; font-size: 13px; color: #9e9e9e; line-height: 1.4; }}
  .pick {{ display: block; width: 100%; text-align: center; background: #f4f4f4; color: #080808; font-weight: 700; font-size: 15px; padding: 14px 18px; border-radius: 999px; cursor: pointer; }}
  input[type=file] {{ display: none; }}
  .list {{ display: grid; gap: 8px; }}
  .item {{ background: #1a1a1a; border-radius: 12px; padding: 10px 12px; display: grid; gap: 6px; }}
  .row {{ display: flex; justify-content: space-between; align-items: center; gap: 8px; font-size: 13px; }}
  .name {{ overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 600; }}
  .st {{ color: #9e9e9e; font-size: 12px; flex: none; font-variant-numeric: tabular-nums; }}
  .st.ok {{ color: #f4f4f4; }}
  .st.err {{ color: #f87171; }}
  .bar {{ height: 4px; background: #262626; border-radius: 999px; overflow: hidden; }}
  .fill {{ height: 100%; width: 0%; background: #f4f4f4; transition: width 120ms linear; }}
</style>
</head>
<body>
<div class="wrap">
  <div class="card">
    <div class="brand-row">
      <span class="brand-icon" aria-hidden="true">{peek_icon_svg}</span>
      <h1>Send to Desktop</h1>
    </div>
    <p>Files save straight to Downloads\Peek and appear in Peek Files.</p>
    <label class="pick">Choose files or photos<input id="picker" type="file" multiple /></label>
  </div>
  <div id="list" class="list"></div>
</div>
<script>
const UPLOAD_URL = "{upload_route}";
const picker = document.getElementById("picker");
const list = document.getElementById("list");
picker.addEventListener("change", async () => {{
  const files = Array.from(picker.files || []);
  picker.value = "";
  for (const file of files) {{
    await sendOne(file);
  }}
}});
function sendOne(file) {{
  return new Promise((resolve) => {{
    const card = document.createElement("div");
    card.className = "item";
    const row = document.createElement("div");
    row.className = "row";
    const nameEl = document.createElement("span");
    nameEl.className = "name";
    nameEl.textContent = file.name || "file";
    const st = document.createElement("span");
    st.className = "st";
    st.textContent = "0%";
    row.append(nameEl, st);
    const bar = document.createElement("div");
    bar.className = "bar";
    const fill = document.createElement("div");
    fill.className = "fill";
    bar.append(fill);
    card.append(row, bar);
    list.prepend(card);
    const xhr = new XMLHttpRequest();
    xhr.open("POST", UPLOAD_URL + "?name=" + encodeURIComponent(file.name || "phone-file"), true);
    xhr.upload.onprogress = (ev) => {{
      if (ev.lengthComputable && ev.total > 0) {{
        const pct = Math.min(100, Math.round((ev.loaded / ev.total) * 100));
        fill.style.width = pct + "%";
        st.textContent = pct + "%";
      }}
    }};
    xhr.onload = () => {{
      if (xhr.status >= 200 && xhr.status < 300) {{
        fill.style.width = "100%";
        st.className = "st ok";
        st.textContent = "Sent";
      }} else {{
        st.className = "st err";
        st.textContent = "Failed (" + xhr.status + ")";
      }}
      resolve();
    }};
    xhr.onerror = () => {{
      st.className = "st err";
      st.textContent = "Network error";
      resolve();
    }};
    xhr.send(file);
  }});
}}
</script>
</body>
</html>"#,
        upload_route = upload_route,
        icon_b64 = icon_b64,
        peek_icon_svg = PEEK_ICON_SVG,
    )
}

fn escape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn format_bytes_human(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn render_send_page(files: &[(String, u64, String)]) -> String {
    const PEEK_ICON_SVG: &str = include_str!("../icons/peek-icon.svg");
    let icon_b64 = STANDARD.encode(PEEK_ICON_SVG.as_bytes());
    let mut rows = String::new();
    for (name, size, href) in files {
        let safe_name = escape_html(name);
        let safe_href = escape_html(href);
        let size_label = format_bytes_human(*size);
        rows.push_str(&format!(
            r#"<div class="item"><div class="info"><div class="name" title="{safe_name}">{safe_name}</div><div class="sz">{size_label}</div></div><a class="dl" href="{safe_href}" download="{safe_name}">Download</a></div>"#
        ));
    }
    let count_note = if files.len() == 1 {
        "1 file ready to save on your phone.".to_string()
    } else {
        format!("{} files ready to save on your phone.", files.len())
    };
    let all_btn = if files.len() > 1 {
        r#"<button id="dlAll" class="pick" type="button">Download all</button>"#
    } else {
        ""
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1" />
<title>Download from Peek</title>
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml;base64,{icon_b64}" />
<style>
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; padding: 24px 16px; background: #080808; color: #f4f4f4; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  .wrap {{ max-width: 420px; margin: 0 auto; display: grid; gap: 14px; }}
  .card {{ background: #141414; border: 1px solid rgba(255,255,255,0.08); border-radius: 18px; padding: 18px; display: grid; gap: 12px; }}
  .brand-row {{ display: flex; align-items: center; gap: 10px; }}
  .brand-icon {{ width: 28px; height: 28px; flex: none; display: block; }}
  .brand-icon svg {{ width: 100%; height: 100%; display: block; }}
  h1 {{ margin: 0; font-size: 18px; font-weight: 700; letter-spacing: -0.02em; }}
  p {{ margin: 0; font-size: 13px; color: #9e9e9e; line-height: 1.4; }}
  .pick {{ display: block; width: 100%; border: 0; text-align: center; background: #f4f4f4; color: #080808; font-weight: 700; font-size: 14px; padding: 12px 16px; border-radius: 999px; cursor: pointer; }}
  .list {{ display: grid; gap: 8px; }}
  .item {{ background: #141414; border: 1px solid rgba(255,255,255,0.08); border-radius: 14px; padding: 12px; display: flex; align-items: center; justify-content: space-between; gap: 10px; }}
  .info {{ min-width: 0; display: grid; gap: 2px; }}
  .name {{ overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13.5px; font-weight: 600; }}
  .sz {{ font-size: 11.5px; color: #9e9e9e; font-variant-numeric: tabular-nums; }}
  .dl {{ flex: none; text-decoration: none; background: #f4f4f4; color: #080808; font-weight: 700; font-size: 12.5px; padding: 8px 13px; border-radius: 999px; }}
</style>
</head>
<body>
<div class="wrap">
  <div class="card">
    <div class="brand-row">
      <span class="brand-icon" aria-hidden="true">{peek_icon_svg}</span>
      <h1>Download from Desktop</h1>
    </div>
    <p>{count_note}</p>
    {all_btn}
  </div>
  <div class="list">{rows}</div>
</div>
<script>
const btn = document.getElementById("dlAll");
if (btn) {{
  btn.addEventListener("click", () => {{
    const links = Array.from(document.querySelectorAll("a.dl"));
    links.forEach((a, idx) => {{
      setTimeout(() => a.click(), idx * 450);
    }});
  }});
}}
</script>
</body>
</html>"#,
        icon_b64 = icon_b64,
        peek_icon_svg = PEEK_ICON_SVG,
        count_note = count_note,
        all_btn = all_btn,
        rows = rows,
    )
}

fn firewall_blocked_now() -> bool {
    #[cfg(windows)]
    {
        crate::windows::is_exe_inbound_firewall_blocked()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

async fn handle_qr_receive_connection(
    mut socket: tokio::net::TcpStream,
    page: &str,
    page_route: &str,
    up_route: &str,
    dest_dir: &Path,
) -> Option<(PathBuf, String)> {
    let (buf, header_end) = read_http_headers(&mut socket).await?;
    let header_str = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_str.lines();
    let first_line = lines.next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let full_target = parts.next().unwrap_or("");
    let (req_path, query) = full_target.split_once('?').unwrap_or((full_target, ""));

    if method == "GET" && constant_time_eq(req_path.as_bytes(), page_route.as_bytes()) {
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            page.len(),
            page
        );
        let _ = socket.write_all(resp.as_bytes()).await;
        return None;
    }

    if method == "POST" && constant_time_eq(req_path.as_bytes(), up_route.as_bytes()) {
        let mut content_len: Option<u64> = None;
        let mut expect_continue = false;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_len = v.trim().parse::<u64>().ok();
                } else if k.trim().eq_ignore_ascii_case("expect")
                    && v.to_ascii_lowercase().contains("100-continue")
                {
                    expect_continue = true;
                }
            }
        }

        let Some(total_len) = content_len.filter(|&l| l > 0) else {
            let _ = socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\nConnection: close\r\n\r\nMissing payload").await;
            return None;
        };
        if total_len > MAX_RECEIVE_BYTES {
            let _ = socket.write_all(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 14\r\nConnection: close\r\n\r\nFile too large").await;
            return None;
        }

        let raw_name = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("name="))
            .unwrap_or("phone-file");
        let safe_name = sanitize_upload_filename(raw_name);

        let Ok(temp) = tempfile::NamedTempFile::new_in(dest_dir) else {
            let _ = socket.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 11\r\nConnection: close\r\n\r\nDisk error\n").await;
            return None;
        };
        let mut writer = BufWriter::with_capacity(64 * 1024, temp);

        let trailing = &buf[header_end..];
        let mut remaining = total_len;
        if !trailing.is_empty() {
            let take = (trailing.len() as u64).min(remaining) as usize;
            if writer.write_all(&trailing[..take]).is_err() {
                return None;
            }
            remaining -= take as u64;
        } else if expect_continue {
            let _ = socket.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await;
        }

        let mut chunk = vec![0u8; 64 * 1024];
        while remaining > 0 {
            let want = chunk.len().min(remaining as usize);
            let Ok(Ok(read_n)) =
                tokio::time::timeout(Duration::from_secs(30), socket.read(&mut chunk[..want]))
                    .await
            else {
                return None;
            };
            if read_n == 0 {
                return None;
            }
            if writer.write_all(&chunk[..read_n]).is_err() {
                return None;
            }
            remaining -= read_n as u64;
        }

        let dir_save = dest_dir.to_path_buf();
        let name_save = safe_name.clone();
        let saved_res = tokio::task::spawn_blocking(move || {
            writer.flush().map_err(|e| e.to_string())?;
            let temp = writer
                .into_inner()
                .map_err(|e| e.into_error().to_string())?;
            let final_path = unique_dest_path(&dir_save, &name_save);
            temp.persist(&final_path).map_err(|e| e.error.to_string())?;
            let final_name = final_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&name_save)
                .to_string();
            Ok::<(PathBuf, String), String>((final_path, final_name))
        })
        .await;

        let Ok(Ok((final_path, final_name))) = saved_res else {
            let _ = socket.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 10\r\nConnection: close\r\n\r\nSave error").await;
            return None;
        };

        let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}").await;
        return Some((final_path, final_name));
    }

    let _ = socket
        .write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found",
        )
        .await;
    None
}

#[tauri::command]
pub async fn start_qr_transfer(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
    id: String,
) -> Result<QrTransferInfo, String> {
    let entry = {
        let mut guard = state.inner.lock().unwrap();
        if let Some(prev) = guard.qr_stop.take() {
            let _ = prev.send(());
        }
        guard.qr_active = None;
        guard
            .entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or_else(|| "Shelf item not found".to_string())?
    };
    let file_path = PathBuf::from(&entry.path);
    if !file_path.is_file() {
        return Err("QR transfer requires an existing single file".into());
    }

    let listener = TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let ip = detect_lan_ip();
    #[cfg(windows)]
    let token = crate::windows::generate_random_hex(16);
    #[cfg(not(windows))]
    let token = "localtoken".to_string();

    let safe_name: String = entry
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let route = format!("/dl/{token}/{safe_name}");
    let url = format!("http://{ip}:{port}{route}");
    let qr_data_url = render_qr_data_url(&url)?;

    let info = QrTransferInfo {
        file_name: entry.name.clone(),
        url,
        qr_data_url,
        expires_in_seconds: 300,
        mode: "send".into(),
        save_dir: None,
        received_count: 0,
        last_received: None,
        firewall_blocked: firewall_blocked_now(),
    };

    let (stop_tx, mut stop_rx) = oneshot::channel();
    let seq = {
        let mut guard = state.inner.lock().unwrap();
        guard.qr_seq += 1;
        let seq = guard.qr_seq;
        guard.qr_stop = Some(stop_tx);
        guard.qr_active = Some((seq, info.clone()));
        seq
    };
    let _ = app.emit("qr-transfer-updated", Some(&info));

    let expected_route = route;
    let file_name = entry.name.clone();
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = &mut deadline => break,
                accepted = listener.accept() => {
                    let Ok((mut socket, _)) = accepted else { continue; };
                    let target = file_path.clone();
                    let route_check = expected_route.clone();
                    let dl_name = file_name.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut buf = [0u8; 2048];
                        let Ok(Ok(n)) = tokio::time::timeout(
                            Duration::from_secs(5),
                            socket.read(&mut buf),
                        )
                        .await
                        else {
                            return;
                        };
                        let req = String::from_utf8_lossy(&buf[..n]);
                        let first_line = req.lines().next().unwrap_or("");
                        let mut parts = first_line.split_whitespace();
                        let method = parts.next().unwrap_or("");
                        let path = parts.next().unwrap_or("");
                        if method != "GET" || !constant_time_eq(path.as_bytes(), route_check.as_bytes()) {
                            let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found").await;
                            return;
                        }
                        let Ok(mut f) = File::open(&target) else {
                            let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 7\r\nConnection: close\r\n\r\nMissing").await;
                            return;
                        };
                        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                        let clean_header_name = sanitize_content_disposition_name(&dl_name);
                        let header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {len}\r\nContent-Disposition: attachment; filename=\"{clean_header_name}\"\r\nConnection: close\r\n\r\n"
                        );
                        if socket.write_all(header.as_bytes()).await.is_err() {
                            return;
                        }
                        let mut chunk = vec![0u8; 64 * 1024];
                        loop {
                            let Ok(read_n) = f.read(&mut chunk) else { break; };
                            if read_n == 0 {
                                break;
                            }
                            if socket.write_all(&chunk[..read_n]).await.is_err() {
                                break;
                            }
                        }
                    });
                }
            }
        }
        let st = app_for_task.state::<ShelfState>();
        let mut guard = st.inner.lock().unwrap();
        if guard.qr_active.as_ref().is_some_and(|(s, _)| *s == seq) {
            guard.qr_active = None;
            guard.qr_stop = None;
            drop(guard);
            let _ = app_for_task.emit("qr-transfer-updated", Option::<QrTransferInfo>::None);
        }
    });

    Ok(info)
}

async fn handle_qr_send_connection(
    mut socket: tokio::net::TcpStream,
    page: &str,
    page_route: &str,
    dl_prefix: &str,
    files: &[(PathBuf, String)],
) {
    let Some((buf, header_end)) = read_http_headers(&mut socket).await else {
        return;
    };
    let header_str = String::from_utf8_lossy(&buf[..header_end]);
    let first_line = header_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let full_target = parts.next().unwrap_or("");
    let (req_path, _) = full_target.split_once('?').unwrap_or((full_target, ""));

    if method != "GET" {
        let _ = socket
            .write_all(
                b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 18\r\nConnection: close\r\n\r\nMethod Not Allowed",
            )
            .await;
        return;
    }

    if constant_time_eq(req_path.as_bytes(), page_route.as_bytes()) {
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            page.len(),
            page
        );
        let _ = socket.write_all(resp.as_bytes()).await;
        return;
    }

    if req_path.len() > dl_prefix.len()
        && constant_time_eq(
            &req_path.as_bytes()[..dl_prefix.len()],
            dl_prefix.as_bytes(),
        )
    {
        if let Ok(idx) = req_path[dl_prefix.len()..].parse::<usize>() {
            if let Some((path, name)) = files.get(idx) {
                let Ok(mut f) = File::open(path) else {
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 7\r\nConnection: close\r\n\r\nMissing",
                        )
                        .await;
                    return;
                };
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let clean_header_name = sanitize_content_disposition_name(name);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {len}\r\nContent-Disposition: attachment; filename=\"{clean_header_name}\"\r\nConnection: close\r\n\r\n"
                );
                if socket.write_all(header.as_bytes()).await.is_err() {
                    return;
                }
                let mut chunk = vec![0u8; 64 * 1024];
                loop {
                    let Ok(read_n) = f.read(&mut chunk) else {
                        break;
                    };
                    if read_n == 0 {
                        break;
                    }
                    if socket.write_all(&chunk[..read_n]).await.is_err() {
                        break;
                    }
                }
                return;
            }
        }
    }

    let _ = socket
        .write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found",
        )
        .await;
}

#[tauri::command]
pub async fn start_qr_send_files(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
    ids: Vec<String>,
) -> Result<QrTransferInfo, String> {
    let entries = {
        let mut guard = state.inner.lock().unwrap();
        if let Some(prev) = guard.qr_stop.take() {
            let _ = prev.send(());
        }
        guard.qr_active = None;
        guard.entries.clone()
    };

    let selected: Vec<(PathBuf, String, u64)> = if ids.is_empty() {
        entries
            .into_iter()
            .filter_map(|e| {
                let pb = PathBuf::from(&e.path);
                if pb.is_file() {
                    let sz = fs::metadata(&pb).map(|m| m.len()).unwrap_or(0);
                    Some((pb, e.name, sz))
                } else {
                    None
                }
            })
            .collect()
    } else {
        ids.iter()
            .filter_map(|id| entries.iter().find(|e| &e.id == id))
            .filter_map(|e| {
                let pb = PathBuf::from(&e.path);
                if pb.is_file() {
                    let sz = fs::metadata(&pb).map(|m| m.len()).unwrap_or(0);
                    Some((pb, e.name.clone(), sz))
                } else {
                    None
                }
            })
            .collect()
    };

    if selected.is_empty() {
        return Err("Stage or pick at least one file to send".into());
    }

    let listener = TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let ip = detect_lan_ip();
    #[cfg(windows)]
    let token = crate::windows::generate_random_hex(16);
    #[cfg(not(windows))]
    let token = "localtoken".to_string();

    let page_route = format!("/send/{token}");
    let dl_prefix = format!("/send/{token}/dl/");
    let url = format!("http://{ip}:{port}{page_route}");
    let qr_data_url = render_qr_data_url(&url)?;

    let page_files: Vec<(String, u64, String)> = selected
        .iter()
        .enumerate()
        .map(|(i, (_, name, sz))| (name.clone(), *sz, format!("{dl_prefix}{i}")))
        .collect();
    let html_page = render_send_page(&page_files);

    let file_name = if selected.len() == 1 {
        selected[0].1.clone()
    } else {
        format!("Send {} files to phone", selected.len())
    };

    let info = QrTransferInfo {
        file_name,
        url,
        qr_data_url,
        expires_in_seconds: 300,
        mode: "send".into(),
        save_dir: None,
        received_count: 0,
        last_received: None,
        firewall_blocked: firewall_blocked_now(),
    };

    let (stop_tx, mut stop_rx) = oneshot::channel();
    let seq = {
        let mut guard = state.inner.lock().unwrap();
        guard.qr_seq += 1;
        let seq = guard.qr_seq;
        guard.qr_stop = Some(stop_tx);
        guard.qr_active = Some((seq, info.clone()));
        seq
    };
    let _ = app.emit("qr-transfer-updated", Some(&info));

    let serve_files: Vec<(PathBuf, String)> =
        selected.into_iter().map(|(p, n, _)| (p, n)).collect();
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = &mut deadline => break,
                accepted = listener.accept() => {
                    let Ok((socket, _)) = accepted else { continue; };
                    let page = html_page.clone();
                    let p_route = page_route.clone();
                    let d_prefix = dl_prefix.clone();
                    let f_list = serve_files.clone();
                    tauri::async_runtime::spawn(async move {
                        handle_qr_send_connection(socket, &page, &p_route, &d_prefix, &f_list).await;
                    });
                }
            }
        }
        let st = app_for_task.state::<ShelfState>();
        let mut guard = st.inner.lock().unwrap();
        if guard.qr_active.as_ref().is_some_and(|(s, _)| *s == seq) {
            guard.qr_active = None;
            guard.qr_stop = None;
            drop(guard);
            let _ = app_for_task.emit("qr-transfer-updated", Option::<QrTransferInfo>::None);
        }
    });

    Ok(info)
}

#[tauri::command]
pub async fn start_qr_receive(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
) -> Result<QrTransferInfo, String> {
    {
        let mut guard = state.inner.lock().unwrap();
        if let Some(prev) = guard.qr_stop.take() {
            let _ = prev.send(());
        }
        guard.qr_active = None;
    }

    let dest_dir = received_files_dir()?;
    let listener = TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let ip = detect_lan_ip();
    #[cfg(windows)]
    let token = crate::windows::generate_random_hex(16);
    #[cfg(not(windows))]
    let token = "localtoken".to_string();

    let route = format!("/up/{token}");
    let upload_route = format!("/up/{token}/file");
    let url = format!("http://{ip}:{port}{route}");
    let qr_data_url = render_qr_data_url(&url)?;
    let html_page = render_receive_page(&upload_route);

    let info = QrTransferInfo {
        file_name: "Receive from phone".into(),
        url,
        qr_data_url,
        expires_in_seconds: 300,
        mode: "receive".into(),
        save_dir: Some(dest_dir.to_string_lossy().to_string()),
        received_count: 0,
        last_received: None,
        firewall_blocked: firewall_blocked_now(),
    };

    let (stop_tx, mut stop_rx) = oneshot::channel();
    let seq = {
        let mut guard = state.inner.lock().unwrap();
        guard.qr_seq += 1;
        let seq = guard.qr_seq;
        guard.qr_stop = Some(stop_tx);
        guard.qr_active = Some((seq, info.clone()));
        seq
    };
    let _ = app.emit("qr-transfer-updated", Some(&info));

    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = &mut deadline => break,
                accepted = listener.accept() => {
                    let Ok((socket, _)) = accepted else { continue; };
                    let page = html_page.clone();
                    let page_route = route.clone();
                    let up_route = upload_route.clone();
                    let dir_for_conn = dest_dir.clone();
                    let app_conn = app_for_task.clone();
                    tauri::async_runtime::spawn(async move {
                        let Some((final_path, final_name)) = handle_qr_receive_connection(
                            socket,
                            &page,
                            &page_route,
                            &up_route,
                            &dir_for_conn,
                        )
                        .await
                        else {
                            return;
                        };

                        let app_ingest = app_conn.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Ok((items, _)) = app_ingest
                                .state::<ShelfState>()
                                .add_paths(std::slice::from_ref(&final_path))
                            {
                                let _ = app_ingest.emit("shelf-updated", &items);
                            }
                        })
                        .await;

                        let updated_info = {
                            let st = app_conn.state::<ShelfState>();
                            let mut guard = st.inner.lock().unwrap();
                            if let Some((cur_seq, cur_info)) = guard.qr_active.as_mut() {
                                if *cur_seq == seq {
                                    cur_info.received_count += 1;
                                    cur_info.last_received = Some(final_name.clone());
                                    Some(cur_info.clone())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        };
                        if let Some(info_now) = updated_info {
                            let _ = app_conn.emit("qr-transfer-updated", Some(&info_now));
                        }
                        app_conn.state::<ActivityManager>().push_activity(
                            "Files",
                            "Received from phone",
                            final_name,
                            Priority::Normal,
                            3500,
                            Some("files"),
                            false,
                            false,
                        );
                    });
                }
            }
        }
        let st = app_for_task.state::<ShelfState>();
        let mut guard = st.inner.lock().unwrap();
        if guard.qr_active.as_ref().is_some_and(|(s, _)| *s == seq) {
            guard.qr_active = None;
            guard.qr_stop = None;
            drop(guard);
            let _ = app_for_task.emit("qr-transfer-updated", Option::<QrTransferInfo>::None);
        }
    });

    Ok(info)
}

#[tauri::command]
pub fn get_qr_transfer(state: tauri::State<'_, ShelfState>) -> Option<QrTransferInfo> {
    let blocked = firewall_blocked_now();
    let mut guard = state.inner.lock().unwrap();
    if let Some((_, info)) = guard.qr_active.as_mut() {
        info.firewall_blocked = blocked;
        Some(info.clone())
    } else {
        None
    }
}

#[tauri::command]
pub async fn allow_lan_firewall(
    app: AppHandle,
    state: tauri::State<'_, ShelfState>,
) -> Result<Option<QrTransferInfo>, String> {
    #[cfg(windows)]
    tokio::task::spawn_blocking(crate::windows::allow_exe_in_windows_firewall)
        .await
        .map_err(|e| e.to_string())??;

    let blocked = firewall_blocked_now();
    let updated = {
        let mut guard = state.inner.lock().unwrap();
        if let Some((_, info)) = guard.qr_active.as_mut() {
            info.firewall_blocked = blocked;
            Some(info.clone())
        } else {
            None
        }
    };
    let _ = app.emit("qr-transfer-updated", updated.as_ref());
    Ok(updated)
}

#[tauri::command]
pub fn stop_qr_transfer(app: AppHandle, state: tauri::State<'_, ShelfState>) {
    let mut guard = state.inner.lock().unwrap();
    if let Some(tx) = guard.qr_stop.take() {
        let _ = tx.send(());
    }
    guard.qr_active = None;
    drop(guard);
    let _ = app.emit("qr-transfer-updated", Option::<QrTransferInfo>::None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use tempfile::TempDir;

    #[test]
    fn rename_rejects_traversal_and_renames_cleanly() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("alpha.txt");
        fs::write(&file, b"hello").unwrap();

        assert!(rename_path_safe(&file, "../escape.txt").is_err());
        assert!(rename_path_safe(&file, "sub\\bad.txt").is_err());
        assert!(rename_path_safe(&file, "").is_err());

        let renamed = rename_path_safe(&file, "beta.txt").unwrap();
        assert!(renamed.exists());
        assert!(!file.exists());
    }

    #[test]
    fn zip_roundtrip_and_slip_guard() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("note.txt");
        fs::write(&src, b"peek zip payload").unwrap();
        let zip_path = dir.path().join("bundle.zip");
        create_zip_from_paths(&[src], &zip_path).unwrap();

        let extract_dir = dir.path().join("out");
        extract_zip_safe(&zip_path, &extract_dir).unwrap();
        assert_eq!(
            fs::read(extract_dir.join("note.txt")).unwrap(),
            b"peek zip payload"
        );

        // Build malicious zip entry with traversal path
        let bad_zip = dir.path().join("bad.zip");
        {
            let mut zw = ZipWriter::new(File::create(&bad_zip).unwrap());
            zw.start_file("../evil.txt", SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"bad").unwrap();
            zw.finish().unwrap();
        }
        assert!(extract_zip_safe(&bad_zip, &dir.path().join("safe_out")).is_err());
    }

    #[test]
    fn image_conversion_preserves_original_and_shelf_detects_missing() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("sample.png");
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(16, 16, Rgba([200, 100, 50, 255]));
        img.save(&src).unwrap();

        let converted = convert_image_sibling(&src, "jpeg", 85).unwrap();
        assert!(src.exists());
        assert!(converted.exists());
        assert_eq!(converted.extension().unwrap(), "jpg");

        let shelf = ShelfState::load(Some(dir.path().to_path_buf()));
        shelf.add_paths(&[src.clone()]).unwrap();
        assert!(shelf.build_items()[0].exists);
        fs::remove_file(&src).unwrap();
        let after = shelf.build_items();
        assert_eq!(after.len(), 1);
        assert!(!after[0].exists);
    }

    #[test]
    fn sanitizes_content_disposition_filenames() {
        assert_eq!(
            sanitize_content_disposition_name("report\"final;\r\nX-Test: 1\\foo.pdf"),
            "report_final___X-Test: 1_foo.pdf"
        );
        assert_eq!(sanitize_content_disposition_name(""), "file");
    }

    #[test]
    fn sanitizes_upload_filenames_and_avoids_collisions() {
        assert_eq!(sanitize_upload_filename("../../evil.exe"), "evil.exe");
        assert_eq!(
            sanitize_upload_filename("sub\\bad:name?.png"),
            "badname.png"
        );
        assert_eq!(sanitize_upload_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_upload_filename("nul"), "_nul");
        assert_eq!(sanitize_upload_filename("my%20photo.jpg"), "my photo.jpg");
        assert_eq!(sanitize_upload_filename("..%2F..%2F"), "phone-file");
        assert_eq!(sanitize_upload_filename(""), "phone-file");

        let dir = TempDir::new().unwrap();
        let first = dir.path().join("photo.jpg");
        fs::write(&first, b"1").unwrap();
        let next = unique_dest_path(dir.path(), "photo.jpg");
        assert_eq!(next.file_name().unwrap().to_str().unwrap(), "photo (1).jpg");
        fs::write(&next, b"2").unwrap();
        let third = unique_dest_path(dir.path(), "photo.jpg");
        assert_eq!(
            third.file_name().unwrap().to_str().unwrap(),
            "photo (2).jpg"
        );
    }

    #[test]
    fn qr_receive_http_roundtrip() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().to_path_buf();
        let route = "/up/tok123".to_string();
        let upload_route = "/up/tok123/file".to_string();
        let page = render_receive_page(&upload_route);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            let srv_page = page.clone();
            let srv_route = route.clone();
            let srv_up = upload_route.clone();
            let srv_dest = dest.clone();
            let server = tokio::spawn(async move {
                let mut uploaded = None;
                for _ in 0..3 {
                    let (sock, _) = listener.accept().await.unwrap();
                    if let Some(res) = handle_qr_receive_connection(
                        sock,
                        &srv_page,
                        &srv_route,
                        &srv_up,
                        &srv_dest,
                    )
                    .await
                    {
                        uploaded = Some(res);
                    }
                }
                uploaded
            });

            // 1) GET upload HTML
            let mut s1 = tokio::net::TcpStream::connect(addr).await.unwrap();
            s1.write_all(format!("GET {route} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut resp1 = String::new();
            s1.read_to_string(&mut resp1).await.unwrap();
            assert!(resp1.starts_with("HTTP/1.1 200 OK"));
            assert!(resp1.contains("Choose files or photos"));

            // 2) POST upload payload split across header + body
            let payload = b"hello from phone";
            let mut s2 = tokio::net::TcpStream::connect(addr).await.unwrap();
            let req2 = format!(
                "POST {upload_route}?name=my%20note.txt HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
                payload.len()
            );
            s2.write_all(req2.as_bytes()).await.unwrap();
            s2.write_all(payload).await.unwrap();
            let mut resp2 = String::new();
            s2.read_to_string(&mut resp2).await.unwrap();
            assert!(resp2.starts_with("HTTP/1.1 200 OK"));
            assert!(resp2.contains("{\"ok\":true}"));

            // 3) Bad token -> 404
            let mut s3 = tokio::net::TcpStream::connect(addr).await.unwrap();
            s3.write_all(b"GET /up/wrong HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
            let mut resp3 = String::new();
            s3.read_to_string(&mut resp3).await.unwrap();
            assert!(resp3.starts_with("HTTP/1.1 404 Not Found"));

            let (saved_path, saved_name) = server.await.unwrap().expect("upload saved");
            assert_eq!(saved_name, "my note.txt");
            assert_eq!(fs::read(saved_path).unwrap(), b"hello from phone");
        });
    }

    #[test]
    fn qr_send_batch_http_roundtrip() {
        let dir = TempDir::new().unwrap();
        let file_a = dir.path().join("photo.png");
        fs::write(&file_a, b"png-data-here").unwrap();

        let route = "/send/tokabc".to_string();
        let dl_prefix = "/send/tokabc/dl/".to_string();
        let page = render_send_page(&[("photo.png".into(), 13, format!("{dl_prefix}0"))]);
        let files = vec![(file_a, "photo.png".to_string())];

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            let srv_page = page.clone();
            let srv_route = route.clone();
            let srv_prefix = dl_prefix.clone();
            let srv_files = files.clone();
            let server = tokio::spawn(async move {
                for _ in 0..3 {
                    let (sock, _) = listener.accept().await.unwrap();
                    handle_qr_send_connection(sock, &srv_page, &srv_route, &srv_prefix, &srv_files)
                        .await;
                }
            });

            // 1) GET listing HTML
            let mut s1 = tokio::net::TcpStream::connect(addr).await.unwrap();
            s1.write_all(format!("GET {route} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut resp1 = String::new();
            s1.read_to_string(&mut resp1).await.unwrap();
            assert!(resp1.starts_with("HTTP/1.1 200 OK"));
            assert!(resp1.contains("photo.png"));

            // 2) GET file 0
            let mut s2 = tokio::net::TcpStream::connect(addr).await.unwrap();
            s2.write_all(
                format!("GET {dl_prefix}0 HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
            let mut resp2 = Vec::new();
            s2.read_to_end(&mut resp2).await.unwrap();
            let resp2_str = String::from_utf8_lossy(&resp2);
            assert!(resp2_str.starts_with("HTTP/1.1 200 OK"));
            assert!(resp2_str.contains("filename=\"photo.png\""));
            assert!(resp2.ends_with(b"png-data-here"));

            // 3) Invalid index -> 404
            let mut s3 = tokio::net::TcpStream::connect(addr).await.unwrap();
            s3.write_all(
                format!("GET {dl_prefix}9 HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
            let mut resp3 = String::new();
            s3.read_to_string(&mut resp3).await.unwrap();
            assert!(resp3.starts_with("HTTP/1.1 404 Not Found"));

            server.await.unwrap();
        });
    }
}
