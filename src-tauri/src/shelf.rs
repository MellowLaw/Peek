use base64::{engine::general_purpose::STANDARD, Engine};
use image::{codecs::jpeg::JpegEncoder, ImageFormat, ImageReader};
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Read, Write},
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
}

struct ShelfInner {
    entries: Vec<ShelfEntry>,
    thumbs: HashMap<(String, u64), Option<String>>,
    next_id: u64,
    qr_stop: Option<oneshot::Sender<()>>,
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
fn make_shell_thumbnail(path: &Path) -> Option<String> {
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

    fn extract_on_sta(path: &Path) -> Option<String> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if hr == RPC_E_CHANGED_MODE {
                let owned = path.to_path_buf();
                return std::thread::spawn(move || extract_on_sta(&owned))
                    .join()
                    .ok()
                    .flatten();
            }
            let need_uninit = hr.is_ok();
            let res = (|| -> Option<String> {
                let hpath = HSTRING::from(path.as_os_str());
                let factory: IShellItemImageFactory =
                    SHCreateItemFromParsingName(&hpath, None).ok()?;
                let size = SIZE { cx: 256, cy: 256 };
                let hbm = factory
                    .GetImage(size, SIIGBF_BIGGERSIZEOK)
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

    extract_on_sta(path)
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

#[tauri::command]
pub async fn start_qr_transfer(
    state: tauri::State<'_, ShelfState>,
    id: String,
) -> Result<QrTransferInfo, String> {
    let entry = {
        let mut guard = state.inner.lock().unwrap();
        if let Some(prev) = guard.qr_stop.take() {
            let _ = prev.send(());
        }
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

    let code = QrCode::new(url.as_bytes()).map_err(|e| e.to_string())?;
    let svg_xml = code
        .render::<svg::Color>()
        .min_dimensions(180, 180)
        .dark_color(svg::Color("#080808"))
        .light_color(svg::Color("#ffffff"))
        .build();
    let qr_data_url = format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg_xml.as_bytes())
    );

    let (stop_tx, mut stop_rx) = oneshot::channel();
    state.inner.lock().unwrap().qr_stop = Some(stop_tx);

    let expected_route = route;
    let file_name = entry.name.clone();
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
    });

    Ok(QrTransferInfo {
        file_name: entry.name,
        url,
        qr_data_url,
        expires_in_seconds: 300,
    })
}

#[tauri::command]
pub fn stop_qr_transfer(state: tauri::State<'_, ShelfState>) {
    if let Some(tx) = state.inner.lock().unwrap().qr_stop.take() {
        let _ = tx.send(());
    }
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
}
