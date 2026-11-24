use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicIsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::activity::{ActivityManager, OngoingKind, Priority};
use crate::preferences::write_json_atomic;

const CLIPBOARD_FILE: &str = "clipboard.json";
const CLIPBOARD_IMAGES_DIR: &str = "clipboard_images";
const NOTES_FILE: &str = "notes.json";
const MAX_CLIPBOARD_ITEMS: usize = 100;
const MAX_CLIPBOARD_IMAGES: usize = 30;
const MAX_CLIPBOARD_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardKind {
    #[default]
    Text,
    Image,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardItem {
    pub id: String,
    #[serde(default)]
    pub kind: ClipboardKind,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_thumbnail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub pinned: bool,
    pub copied_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    #[default]
    Note,
    Checklist,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistItem {
    pub id: String,
    pub text: String,
    pub done: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteItem {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub kind: NoteKind,
    #[serde(default)]
    pub items: Vec<ChecklistItem>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetItem {
    pub id: String,
    pub title: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NotesStoreData {
    pub notes: Vec<NoteItem>,
    pub snippets: Vec<SnippetItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderItem {
    pub id: String,
    pub title: String,
    pub due_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerSnapshot {
    pub running: bool,
    pub paused: bool,
    pub label: String,
    pub total_seconds: u64,
    pub remaining_seconds: u64,
    pub reminders: Vec<ReminderItem>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub name: String,
    pub path: String,
}

struct TimerInner {
    running: bool,
    paused: bool,
    label: String,
    total_seconds: u64,
    remaining_seconds: u64,
    reminders: Vec<ReminderItem>,
    next_seq: u64,
}

impl Default for TimerInner {
    fn default() -> Self {
        Self {
            running: false,
            paused: false,
            label: "Focus".into(),
            total_seconds: 25 * 60,
            remaining_seconds: 25 * 60,
            reminders: Vec::new(),
            next_seq: 1,
        }
    }
}

pub struct ToolsState {
    config_dir: Option<PathBuf>,
    clipboard: Arc<Mutex<Vec<ClipboardItem>>>,
    clipboard_hwnd: Arc<AtomicIsize>,
    clipboard_enabled: Mutex<bool>,
    notes: Mutex<NotesStoreData>,
    timer: Arc<Mutex<TimerInner>>,
    timer_wake: Arc<Notify>,
    app_icons: Mutex<HashMap<String, Option<String>>>,
    app_handle: AppHandle,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn format_mm_ss(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn hash_rgba_image(width: u32, height: u32, rgba: &[u8]) -> String {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0xaf63bd4c8601b7df;
    for &b in rgba {
        h1 ^= b as u64;
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 = h2.wrapping_mul(0x100000001b3);
        h2 ^= b as u64;
    }
    format!("{width}x{height}-{h1:016x}-{h2:016x}")
}

fn encode_clipboard_image(width: u32, height: u32, rgba: Vec<u8>) -> Option<(Vec<u8>, String)> {
    let img = RgbaImage::from_raw(width, height, rgba)?;
    let dyn_img = image::DynamicImage::ImageRgba8(img);

    let mut full_png = Vec::new();
    dyn_img
        .write_to(&mut Cursor::new(&mut full_png), ImageFormat::Png)
        .ok()?;

    let thumb = dyn_img.thumbnail(420, 240);
    let mut thumb_png = Vec::new();
    thumb
        .write_to(&mut Cursor::new(&mut thumb_png), ImageFormat::Png)
        .ok()?;

    let thumb_url = format!("data:image/png;base64,{}", STANDARD.encode(&thumb_png));
    Some((full_png, thumb_url))
}

fn prune_clipboard_items(items: &mut Vec<ClipboardItem>) {
    items.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.copied_at_ms.cmp(&a.copied_at_ms))
    });

    let mut unpinned_images = 0usize;
    items.retain(|item| {
        if item.pinned || item.kind != ClipboardKind::Image {
            return true;
        }
        unpinned_images += 1;
        unpinned_images <= MAX_CLIPBOARD_IMAGES
    });

    if items.len() > MAX_CLIPBOARD_ITEMS {
        let mut kept = 0usize;
        items.retain(|item| {
            if item.pinned {
                kept += 1;
                true
            } else if kept < MAX_CLIPBOARD_ITEMS {
                kept += 1;
                true
            } else {
                false
            }
        });
    }
}

fn cleanup_orphaned_clipboard_images(config_dir: &Path, items: &[ClipboardItem]) {
    let dir = config_dir.join(CLIPBOARD_IMAGES_DIR);
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let live_names: HashSet<String> = items
        .iter()
        .filter_map(|i| i.image_path.as_deref())
        .filter_map(|p| Path::new(p).file_name())
        .filter_map(|n| n.to_str())
        .map(|s| s.to_ascii_lowercase())
        .collect();

    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if !live_names.contains(&name) {
                let _ = fs::remove_file(p);
            }
        }
    }
}

fn upsert_clipboard_text(items: &mut Vec<ClipboardItem>, clean: &str, ts: u64) {
    let was_pinned = items
        .iter()
        .find(|i| i.kind == ClipboardKind::Text && i.text == clean)
        .map(|i| i.pinned)
        .unwrap_or(false);
    items.retain(|i| !(i.kind == ClipboardKind::Text && i.text == clean));
    let id = format!("cb-{ts}-{}", items.len());
    items.insert(
        0,
        ClipboardItem {
            id,
            kind: ClipboardKind::Text,
            text: clean.to_string(),
            image_path: None,
            image_thumbnail: None,
            width: None,
            height: None,
            content_hash: None,
            pinned: was_pinned,
            copied_at_ms: ts,
        },
    );
    prune_clipboard_items(items);
}

fn prepare_new_image_file(
    config_dir: &Path,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    hash: &str,
    ts: u64,
) -> Option<(String, String, String)> {
    let (full_png, thumb_url) = encode_clipboard_image(width, height, rgba)?;
    let dir = config_dir.join(CLIPBOARD_IMAGES_DIR);
    fs::create_dir_all(&dir).ok()?;
    let short_hash = hash.rsplit('-').next().unwrap_or("img");
    let id = format!("cb-{ts}-{short_hash}");
    let img_path = dir.join(format!("{id}.png"));
    fs::write(&img_path, &full_png).ok()?;
    Some((id, img_path.to_string_lossy().to_string(), thumb_url))
}

fn upsert_clipboard_image(
    items_arc: &Mutex<Vec<ClipboardItem>>,
    config_dir: Option<&Path>,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    ts: u64,
) -> bool {
    let Some(dir) = config_dir else {
        return false;
    };
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if width == 0 || height == 0 || rgba.len() != expected {
        return false;
    }

    let hash = hash_rgba_image(width, height, &rgba);
    {
        let mut guard = items_arc.lock().unwrap();
        if let Some(existing_idx) = guard.iter().position(|i| {
            i.kind == ClipboardKind::Image
                && i.content_hash.as_deref() == Some(hash.as_str())
                && i.image_path
                    .as_deref()
                    .is_some_and(|p| Path::new(p).is_file())
        }) {
            let mut item = guard.remove(existing_idx);
            item.copied_at_ms = ts;
            guard.insert(0, item);
            prune_clipboard_items(&mut guard);
            return true;
        }
    }

    let Some((id, image_path, thumb_url)) =
        prepare_new_image_file(dir, width, height, rgba, &hash, ts)
    else {
        return false;
    };

    let mut guard = items_arc.lock().unwrap();
    let was_pinned = guard
        .iter()
        .find(|i| {
            i.kind == ClipboardKind::Image && i.content_hash.as_deref() == Some(hash.as_str())
        })
        .map(|i| i.pinned)
        .unwrap_or(false);
    guard.retain(|i| {
        !(i.kind == ClipboardKind::Image && i.content_hash.as_deref() == Some(hash.as_str()))
    });
    guard.insert(
        0,
        ClipboardItem {
            id,
            kind: ClipboardKind::Image,
            text: String::new(),
            image_path: Some(image_path),
            image_thumbnail: Some(thumb_url),
            width: Some(width),
            height: Some(height),
            content_hash: Some(hash),
            pinned: was_pinned,
            copied_at_ms: ts,
        },
    );
    prune_clipboard_items(&mut guard);
    true
}

impl ToolsState {
    pub fn start(
        app: AppHandle,
        config_dir: Option<PathBuf>,
        clipboard_enabled: bool,
    ) -> Arc<Self> {
        let mut clipboard_items: Vec<ClipboardItem> = config_dir
            .as_ref()
            .filter(|_| clipboard_enabled)
            .and_then(|d| fs::read(d.join(CLIPBOARD_FILE)).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        clipboard_items.retain(|item| {
            item.kind == ClipboardKind::Text
                || item
                    .image_path
                    .as_deref()
                    .is_some_and(|p| Path::new(p).is_file())
        });

        let notes_data: NotesStoreData = config_dir
            .as_ref()
            .and_then(|d| fs::read(d.join(NOTES_FILE)).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();

        let state = Arc::new(Self {
            config_dir,
            clipboard: Arc::new(Mutex::new(clipboard_items)),
            clipboard_hwnd: Arc::new(AtomicIsize::new(0)),
            clipboard_enabled: Mutex::new(false),
            notes: Mutex::new(notes_data),
            timer: Arc::new(Mutex::new(TimerInner::default())),
            timer_wake: Arc::new(Notify::new()),
            app_icons: Mutex::new(HashMap::new()),
            app_handle: app.clone(),
        });

        state.set_clipboard_enabled(clipboard_enabled);

        let timer_state = state.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let has_work = {
                    let g = timer_state.timer.lock().unwrap();
                    (g.running && !g.paused) || !g.reminders.is_empty()
                };
                if !has_work {
                    timer_state.timer_wake.notified().await;
                    continue;
                }
                tokio::select! {
                    _ = timer_state.timer_wake.notified() => continue,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }

                let now = now_ms();
                let mut completed_timer: Option<String> = None;
                let mut due_reminders: Vec<ReminderItem> = Vec::new();
                let mut ongoing_update: Option<(String, String, bool)> = None;

                {
                    let mut g = timer_state.timer.lock().unwrap();
                    if g.running && !g.paused {
                        if g.remaining_seconds > 0 {
                            g.remaining_seconds -= 1;
                        }
                        if g.remaining_seconds == 0 {
                            g.running = false;
                            g.paused = false;
                            completed_timer = Some(g.label.clone());
                        } else {
                            ongoing_update =
                                Some((g.label.clone(), format_mm_ss(g.remaining_seconds), false));
                        }
                    }

                    let mut keep = Vec::new();
                    for r in g.reminders.drain(..) {
                        if r.due_ms <= now {
                            due_reminders.push(r);
                        } else {
                            keep.push(r);
                        }
                    }
                    g.reminders = keep;
                }

                let act = timer_state.app_handle.state::<ActivityManager>();
                if let Some(label) = completed_timer {
                    act.clear_ongoing(OngoingKind::Timer);
                    act.push_activity(
                        "Timer",
                        "Timer complete",
                        label,
                        Priority::High,
                        6000,
                        Some("timer"),
                        false,
                        true,
                    );
                } else if let Some((label, detail, paused)) = ongoing_update {
                    act.set_ongoing(OngoingKind::Timer, label, detail, paused);
                }

                for r in due_reminders {
                    act.push_activity(
                        "Reminder",
                        "Reminder",
                        r.title,
                        Priority::High,
                        6000,
                        Some("timer"),
                        false,
                        true,
                    );
                }

                let _ = timer_state
                    .app_handle
                    .emit("tools-timer", timer_state.timer_snapshot());
            }
        });

        state
    }

    pub fn set_clipboard_enabled(&self, enabled: bool) {
        let mut cur = self.clipboard_enabled.lock().unwrap();
        if *cur == enabled {
            return;
        }
        *cur = enabled;
        drop(cur);

        if enabled {
            #[cfg(windows)]
            self.start_clipboard_thread();
        } else {
            self.stop_clipboard_thread();
        }
    }

    fn stop_clipboard_thread(&self) {
        #[cfg(windows)]
        {
            let raw = self.clipboard_hwnd.swap(0, Ordering::SeqCst);
            if raw != 0 {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(windows::Win32::Foundation::HWND(raw as _)),
                        windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                }
            }
        }
    }

    #[cfg(windows)]
    fn start_clipboard_thread(&self) {
        if self.clipboard_hwnd.load(Ordering::SeqCst) != 0 {
            return;
        }
        let app = self.app_handle.clone();
        let items_arc = self.clipboard.clone();
        let hwnd_arc = self.clipboard_hwnd.clone();
        let config_dir = self.config_dir.clone();

        let (clip_tx, clip_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let worker_app = app.clone();
        let worker_items = items_arc.clone();
        let worker_dir = config_dir.clone();
        std::thread::spawn(move || {
            while clip_rx.recv().is_ok() {
                std::thread::sleep(Duration::from_millis(25));
                while clip_rx.try_recv().is_ok() {}

                let ts = now_ms();
                let mut changed = false;

                if let Some(raw_text) = crate::windows::read_clipboard_text(MAX_CLIPBOARD_BYTES) {
                    let clean = raw_text.trim();
                    if !clean.is_empty() && clean.len() <= MAX_CLIPBOARD_BYTES {
                        let mut guard = worker_items.lock().unwrap();
                        upsert_clipboard_text(&mut guard, clean, ts);
                        changed = true;
                    }
                } else if let Some((w, h, rgba)) = crate::windows::read_clipboard_image_rgba() {
                    changed = upsert_clipboard_image(
                        &worker_items,
                        worker_dir.as_deref(),
                        w,
                        h,
                        rgba,
                        ts,
                    );
                }

                if changed {
                    let snapshot = worker_items.lock().unwrap().clone();
                    if let Some(dir) = &worker_dir {
                        let _ = write_json_atomic(dir, CLIPBOARD_FILE, &snapshot);
                        cleanup_orphaned_clipboard_images(dir, &snapshot);
                    }
                    let _ = worker_app.emit("clipboard-updated", &snapshot);
                }
            }
        });

        std::thread::spawn(move || {
            use windows::core::w;
            use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
            use windows::Win32::System::DataExchange::{
                AddClipboardFormatListener, RemoveClipboardFormatListener,
            };
            use windows::Win32::System::LibraryLoader::GetModuleHandleW;
            use windows::Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                GetWindowLongPtrW, RegisterClassW, SetWindowLongPtrW, TranslateMessage,
                GWLP_USERDATA, HWND_MESSAGE, MSG, WM_CLIPBOARDUPDATE, WNDCLASSW,
            };

            struct HookCtx {
                tx: std::sync::mpsc::SyncSender<()>,
            }

            unsafe extern "system" fn wnd_proc(
                hwnd: HWND,
                msg: u32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                if msg == WM_CLIPBOARDUPDATE {
                    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const HookCtx;
                    if !ptr.is_null() {
                        let _ = (*ptr).tx.try_send(());
                    }
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            unsafe {
                let hinst = GetModuleHandleW(None).ok().map(|m| m.into());
                let cls_name = w!("PeekClipboardWatcher");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: hinst.unwrap_or_default(),
                    lpszClassName: cls_name,
                    ..Default::default()
                };
                let _ = RegisterClassW(&wc);
                let Ok(hwnd) = CreateWindowExW(
                    Default::default(),
                    cls_name,
                    w!(""),
                    Default::default(),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    hinst,
                    None,
                ) else {
                    return;
                };

                let ctx = Box::new(HookCtx { tx: clip_tx });
                let ctx_ptr = Box::into_raw(ctx);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);
                if AddClipboardFormatListener(hwnd).is_err() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(ctx_ptr));
                    let _ = DestroyWindow(hwnd);
                    return;
                }
                hwnd_arc.store(hwnd.0 as isize, Ordering::SeqCst);

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                hwnd_arc.store(0, Ordering::SeqCst);
                let _ = RemoveClipboardFormatListener(hwnd);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ctx_ptr));
                let _ = DestroyWindow(hwnd);
            }
        });
    }

    fn save_clipboard(&self, items: &[ClipboardItem]) {
        if let Some(dir) = &self.config_dir {
            let _ = write_json_atomic(dir, CLIPBOARD_FILE, items);
            cleanup_orphaned_clipboard_images(dir, items);
        }
    }

    fn save_notes(&self, data: &NotesStoreData) -> Result<(), String> {
        if let Some(dir) = &self.config_dir {
            write_json_atomic(dir, NOTES_FILE, data)?;
        }
        Ok(())
    }

    pub fn create_empty_note(&self) -> Result<NoteItem, String> {
        let ts = now_ms();
        let (created, data) = {
            let mut g = self.notes.lock().unwrap();
            let id = format!("note-{ts}-{}", g.notes.len() + 1);
            let item = NoteItem {
                id,
                title: "Quick Note".to_string(),
                body: String::new(),
                kind: NoteKind::Note,
                items: Vec::new(),
                updated_at_ms: ts,
            };
            g.notes.insert(0, item.clone());
            (item, g.clone())
        };
        self.save_notes(&data)?;
        let _ = self.app_handle.emit("notes-updated", &data);
        Ok(created)
    }

    pub fn timer_snapshot(&self) -> TimerSnapshot {
        let g = self.timer.lock().unwrap();
        TimerSnapshot {
            running: g.running,
            paused: g.paused,
            label: g.label.clone(),
            total_seconds: g.total_seconds,
            remaining_seconds: g.remaining_seconds,
            reminders: g.reminders.clone(),
        }
    }
}

#[tauri::command]
pub fn get_clipboard_history(state: State<'_, Arc<ToolsState>>) -> Vec<ClipboardItem> {
    state.clipboard.lock().unwrap().clone()
}

#[tauri::command]
pub async fn copy_clipboard_item(
    state: State<'_, Arc<ToolsState>>,
    id: String,
) -> Result<(), String> {
    let item = state
        .clipboard
        .lock()
        .unwrap()
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or_else(|| "Clipboard item not found".to_string())?;

    match item.kind {
        ClipboardKind::Text => {
            #[cfg(windows)]
            return crate::windows::copy_text_to_clipboard(&item.text);
            #[cfg(not(windows))]
            Ok(())
        }
        ClipboardKind::Image => {
            let path = item
                .image_path
                .ok_or_else(|| "Clipboard image path missing".to_string())?;
            tokio::task::spawn_blocking(move || {
                let img = image::open(&path)
                    .map_err(|e| format!("Could not read saved clipboard image: {e}"))?
                    .into_rgba8();
                #[cfg(windows)]
                crate::windows::copy_rgba_to_clipboard(img.width(), img.height(), img.as_raw())?;
                Ok::<(), String>(())
            })
            .await
            .map_err(|e| e.to_string())?
        }
    }
}

#[tauri::command]
pub fn toggle_pin_clipboard_item(
    state: State<'_, Arc<ToolsState>>,
    id: String,
) -> Vec<ClipboardItem> {
    let items = {
        let mut g = state.clipboard.lock().unwrap();
        if let Some(item) = g.iter_mut().find(|i| i.id == id) {
            item.pinned = !item.pinned;
        }
        g.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.copied_at_ms.cmp(&a.copied_at_ms))
        });
        g.clone()
    };
    state.save_clipboard(&items);
    items
}

#[tauri::command]
pub fn delete_clipboard_item(state: State<'_, Arc<ToolsState>>, id: String) -> Vec<ClipboardItem> {
    let items = {
        let mut g = state.clipboard.lock().unwrap();
        g.retain(|i| i.id != id);
        g.clone()
    };
    state.save_clipboard(&items);
    items
}

#[tauri::command]
pub fn clear_clipboard_history(state: State<'_, Arc<ToolsState>>) -> Vec<ClipboardItem> {
    let items = {
        let mut g = state.clipboard.lock().unwrap();
        g.retain(|i| i.pinned);
        g.clone()
    };
    state.save_clipboard(&items);
    items
}

#[tauri::command]
pub fn get_notes_and_snippets(state: State<'_, Arc<ToolsState>>) -> NotesStoreData {
    state.notes.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_note(
    state: State<'_, Arc<ToolsState>>,
    id: Option<String>,
    title: String,
    body: String,
    kind: Option<NoteKind>,
    items: Option<Vec<ChecklistItem>>,
) -> Result<NotesStoreData, String> {
    let clean_title = if title.trim().is_empty() {
        "Note".to_string()
    } else {
        title.trim().to_string()
    };
    let clean_items = items.map(|list| {
        list.into_iter()
            .filter(|i| !i.text.trim().is_empty())
            .enumerate()
            .map(|(idx, i)| ChecklistItem {
                id: if i.id.trim().is_empty() {
                    format!("chk-{idx}")
                } else {
                    i.id
                },
                text: i.text.trim().to_string(),
                done: i.done,
            })
            .collect::<Vec<_>>()
    });
    let ts = now_ms();
    let data = {
        let mut g = state.notes.lock().unwrap();
        if let Some(existing_id) = id.as_deref() {
            if let Some(n) = g.notes.iter_mut().find(|n| n.id == existing_id) {
                n.title = clean_title;
                n.body = body;
                if let Some(k) = kind {
                    n.kind = k;
                }
                if let Some(next_items) = clean_items {
                    n.items = next_items;
                }
                n.updated_at_ms = ts;
            }
        } else {
            let id = format!("note-{ts}-{}", g.notes.len() + 1);
            g.notes.insert(
                0,
                NoteItem {
                    id,
                    title: clean_title,
                    body,
                    kind: kind.unwrap_or(NoteKind::Note),
                    items: clean_items.unwrap_or_default(),
                    updated_at_ms: ts,
                },
            );
        }
        g.clone()
    };
    state.save_notes(&data)?;
    let _ = state.app_handle.emit("notes-updated", &data);
    Ok(data)
}

#[tauri::command]
pub fn delete_note(
    state: State<'_, Arc<ToolsState>>,
    id: String,
) -> Result<NotesStoreData, String> {
    let data = {
        let mut g = state.notes.lock().unwrap();
        g.notes.retain(|n| n.id != id);
        g.clone()
    };
    state.save_notes(&data)?;
    let _ = state.app_handle.emit("notes-updated", &data);
    Ok(data)
}

#[tauri::command]
pub fn save_snippet(
    state: State<'_, Arc<ToolsState>>,
    id: Option<String>,
    title: String,
    content: String,
) -> Result<NotesStoreData, String> {
    if content.trim().is_empty() {
        return Err("Snippet content is required".into());
    }
    let clean_title = if title.trim().is_empty() {
        "Snippet".to_string()
    } else {
        title.trim().to_string()
    };
    let ts = now_ms();
    let data = {
        let mut g = state.notes.lock().unwrap();
        if let Some(existing_id) = id.as_deref() {
            if let Some(s) = g.snippets.iter_mut().find(|s| s.id == existing_id) {
                s.title = clean_title;
                s.content = content;
            }
        } else {
            let id = format!("snip-{ts}-{}", g.snippets.len() + 1);
            g.snippets.insert(
                0,
                SnippetItem {
                    id,
                    title: clean_title,
                    content,
                },
            );
        }
        g.clone()
    };
    state.save_notes(&data)?;
    Ok(data)
}

#[tauri::command]
pub fn delete_snippet(
    state: State<'_, Arc<ToolsState>>,
    id: String,
) -> Result<NotesStoreData, String> {
    let data = {
        let mut g = state.notes.lock().unwrap();
        g.snippets.retain(|s| s.id != id);
        g.clone()
    };
    state.save_notes(&data)?;
    Ok(data)
}

#[tauri::command]
pub fn copy_tool_text(text: String) -> Result<(), String> {
    #[cfg(windows)]
    return crate::windows::copy_text_to_clipboard(&text);
    #[cfg(not(windows))]
    {
        let _ = text;
        Ok(())
    }
}

#[tauri::command]
pub fn get_timer_state(state: State<'_, Arc<ToolsState>>) -> TimerSnapshot {
    state.timer_snapshot()
}

#[tauri::command]
pub fn start_timer(
    app: AppHandle,
    state: State<'_, Arc<ToolsState>>,
    label: String,
    duration_seconds: u64,
) -> TimerSnapshot {
    let secs = duration_seconds.clamp(10, 24 * 3600);
    let clean_label = if label.trim().is_empty() {
        "Focus".to_string()
    } else {
        label.trim().to_string()
    };
    {
        let mut g = state.timer.lock().unwrap();
        g.running = true;
        g.paused = false;
        g.label = clean_label.clone();
        g.total_seconds = secs;
        g.remaining_seconds = secs;
    }
    app.state::<ActivityManager>().set_ongoing(
        OngoingKind::Timer,
        clean_label,
        format_mm_ss(secs),
        false,
    );
    state.timer_wake.notify_one();
    let snap = state.timer_snapshot();
    let _ = app.emit("tools-timer", snap.clone());
    snap
}

#[tauri::command]
pub fn pause_timer(app: AppHandle, state: State<'_, Arc<ToolsState>>) -> TimerSnapshot {
    let (label, detail, active) = {
        let mut g = state.timer.lock().unwrap();
        if g.running {
            g.paused = true;
        }
        (
            g.label.clone(),
            format_mm_ss(g.remaining_seconds),
            g.running,
        )
    };
    if active {
        app.state::<ActivityManager>()
            .set_ongoing(OngoingKind::Timer, label, detail, true);
    }
    state.timer_wake.notify_one();
    let snap = state.timer_snapshot();
    let _ = app.emit("tools-timer", snap.clone());
    snap
}

#[tauri::command]
pub fn resume_timer(app: AppHandle, state: State<'_, Arc<ToolsState>>) -> TimerSnapshot {
    let (label, detail, active) = {
        let mut g = state.timer.lock().unwrap();
        if g.running {
            g.paused = false;
        }
        (
            g.label.clone(),
            format_mm_ss(g.remaining_seconds),
            g.running,
        )
    };
    if active {
        app.state::<ActivityManager>()
            .set_ongoing(OngoingKind::Timer, label, detail, false);
    }
    state.timer_wake.notify_one();
    let snap = state.timer_snapshot();
    let _ = app.emit("tools-timer", snap.clone());
    snap
}

#[tauri::command]
pub fn restart_timer(app: AppHandle, state: State<'_, Arc<ToolsState>>) -> TimerSnapshot {
    let (label, detail) = {
        let mut g = state.timer.lock().unwrap();
        let total = if g.total_seconds >= 10 {
            g.total_seconds
        } else {
            25 * 60
        };
        g.total_seconds = total;
        g.remaining_seconds = total;
        g.running = true;
        g.paused = false;
        (g.label.clone(), format_mm_ss(total))
    };
    app.state::<ActivityManager>()
        .set_ongoing(OngoingKind::Timer, label, detail, false);
    state.timer_wake.notify_one();
    let snap = state.timer_snapshot();
    let _ = app.emit("tools-timer", snap.clone());
    snap
}

#[tauri::command]
pub fn reset_timer(app: AppHandle, state: State<'_, Arc<ToolsState>>) -> TimerSnapshot {
    {
        let mut g = state.timer.lock().unwrap();
        g.running = false;
        g.paused = false;
        g.remaining_seconds = g.total_seconds;
    }
    app.state::<ActivityManager>()
        .clear_ongoing(OngoingKind::Timer);
    state.timer_wake.notify_one();
    let snap = state.timer_snapshot();
    let _ = app.emit("tools-timer", snap.clone());
    snap
}

#[tauri::command]
pub fn add_reminder(
    state: State<'_, Arc<ToolsState>>,
    title: String,
    minutes: u64,
) -> Result<TimerSnapshot, String> {
    let clean = title.trim();
    if clean.is_empty() {
        return Err("Enter a reminder title".into());
    }
    let mins = minutes.clamp(1, 24 * 60);
    let due_ms = now_ms().saturating_add(mins * 60_000);
    {
        let mut g = state.timer.lock().unwrap();
        let seq = g.next_seq;
        g.next_seq += 1;
        g.reminders.push(ReminderItem {
            id: format!("rem-{seq}"),
            title: clean.to_string(),
            due_ms,
        });
    }
    state.timer_wake.notify_one();
    Ok(state.timer_snapshot())
}

#[tauri::command]
pub fn remove_reminder(state: State<'_, Arc<ToolsState>>, id: String) -> TimerSnapshot {
    {
        let mut g = state.timer.lock().unwrap();
        g.reminders.retain(|r| r.id != id);
    }
    state.timer_wake.notify_one();
    state.timer_snapshot()
}

fn start_menu_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(pd) = std::env::var("ProgramData") {
        roots.push(PathBuf::from(pd).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Ok(ad) = std::env::var("APPDATA") {
        roots.push(PathBuf::from(ad).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    roots
}

#[tauri::command]
pub async fn scan_installed_apps() -> Vec<InstalledApp> {
    tokio::task::spawn_blocking(|| {
        let mut seen = HashSet::new();
        let mut apps = Vec::new();
        let mut stack = start_menu_roots();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
                {
                    let name = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let lower = name.to_lowercase();
                    if name.is_empty()
                        || lower.starts_with("uninstall")
                        || lower.contains("readme")
                        || lower.contains("help")
                    {
                        continue;
                    }
                    if seen.insert(lower) {
                        apps.push(InstalledApp {
                            name,
                            path: p.to_string_lossy().to_string(),
                        });
                    }
                }
            }
        }
        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        apps
    })
    .await
    .unwrap_or_default()
}

fn is_allowed_shortcut_path(candidate: &Path) -> Result<PathBuf, String> {
    if candidate
        .extension()
        .and_then(|e| e.to_str())
        .is_none_or(|e| !e.eq_ignore_ascii_case("lnk"))
    {
        return Err("Only Start Menu application shortcuts are supported".into());
    }
    let canon = fs::canonicalize(candidate).map_err(|_| "Shortcut does not exist".to_string())?;
    let allowed = start_menu_roots()
        .into_iter()
        .filter_map(|r| fs::canonicalize(r).ok())
        .any(|root| canon.starts_with(&root));
    if !allowed {
        return Err("Shortcut path is outside Start Menu roots".into());
    }
    Ok(canon)
}

#[tauri::command]
pub async fn get_app_icons(
    state: State<'_, Arc<ToolsState>>,
    paths: Vec<String>,
) -> Result<HashMap<String, String>, String> {
    const MAX_ICON_BATCH: usize = 32;
    let mut result = HashMap::new();
    let mut missing: Vec<(String, PathBuf)> = Vec::new();

    {
        let cache = state.app_icons.lock().unwrap();
        for raw in paths.into_iter().take(MAX_ICON_BATCH) {
            if let Some(cached) = cache.get(&raw) {
                if let Some(url) = cached {
                    result.insert(raw, url.clone());
                }
                continue;
            }
            if let Ok(canon) = is_allowed_shortcut_path(Path::new(&raw)) {
                missing.push((raw, canon));
            }
        }
    }

    if missing.is_empty() {
        return Ok(result);
    }

    #[cfg(windows)]
    let extracted = tokio::task::spawn_blocking(move || {
        missing
            .into_iter()
            .map(|(raw, canon)| {
                let icon = crate::shelf::extract_shell_icon_data_url(&canon, 64, true);
                (raw, icon)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())?;

    #[cfg(not(windows))]
    let extracted: Vec<(String, Option<String>)> = Vec::new();

    let mut cache = state.app_icons.lock().unwrap();
    for (raw, icon) in extracted {
        if let Some(url) = &icon {
            result.insert(raw.clone(), url.clone());
        }
        cache.insert(raw, icon);
    }
    Ok(result)
}

#[tauri::command]
pub fn launch_app(path: String) -> Result<(), String> {
    let canon = is_allowed_shortcut_path(Path::new(&path))?;
    #[cfg(windows)]
    return crate::windows::shell_launch_path(&canon);
    #[cfg(not(windows))]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn formats_countdown_timer_and_enforces_launcher_guards() {
        assert_eq!(format_mm_ss(0), "00:00");
        assert_eq!(format_mm_ss(1500), "25:00");
        assert_eq!(format_mm_ss(65), "01:05");

        let dir = TempDir::new().unwrap();
        let exe_file = dir.path().join("bad.exe");
        fs::write(&exe_file, b"mz").unwrap();
        assert!(is_allowed_shortcut_path(&exe_file).is_err());
        assert!(launch_app(exe_file.to_string_lossy().to_string()).is_err());

        let outside_lnk = dir.path().join("outside.lnk");
        fs::write(&outside_lnk, b"lnk").unwrap();
        assert!(is_allowed_shortcut_path(&outside_lnk).is_err());
        assert!(launch_app(outside_lnk.to_string_lossy().to_string()).is_err());
    }

    #[test]
    fn deserializes_legacy_text_clipboard_items_with_defaults() {
        let json = r#"[{"id":"cb-1","text":"hello world","pinned":true,"copiedAtMs":1234}]"#;
        let items: Vec<ClipboardItem> = serde_json::from_str(json).expect("legacy json parses");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ClipboardKind::Text);
        assert_eq!(items[0].text, "hello world");
        assert!(items[0].pinned);
        assert!(items[0].image_path.is_none());
    }

    #[test]
    fn stores_dedupes_and_cleans_up_copied_clipboard_images() {
        let dir = TempDir::new().unwrap();
        let items = Mutex::new(Vec::<ClipboardItem>::new());
        let rgba = vec![255, 0, 128, 255, 0, 200, 255, 255];

        assert!(upsert_clipboard_image(
            &items,
            Some(dir.path()),
            2,
            1,
            rgba.clone(),
            1000
        ));
        {
            let mut g = items.lock().unwrap();
            assert_eq!(g.len(), 1);
            assert_eq!(g[0].kind, ClipboardKind::Image);
            assert_eq!(g[0].width, Some(2));
            assert_eq!(g[0].height, Some(1));
            assert!(g[0]
                .image_thumbnail
                .as_deref()
                .is_some_and(|u| u.starts_with("data:image/png;base64,")));
            let p = PathBuf::from(g[0].image_path.as_ref().unwrap());
            assert!(p.is_file());
            g[0].pinned = true;
        }

        // Identical copy bumps timestamp without creating a duplicate item or extra file
        assert!(upsert_clipboard_image(
            &items,
            Some(dir.path()),
            2,
            1,
            rgba,
            2000
        ));
        let stored_path = {
            let g = items.lock().unwrap();
            assert_eq!(g.len(), 1);
            assert!(g[0].pinned);
            assert_eq!(g[0].copied_at_ms, 2000);
            PathBuf::from(g[0].image_path.as_ref().unwrap())
        };
        assert_eq!(
            fs::read_dir(dir.path().join(CLIPBOARD_IMAGES_DIR))
                .unwrap()
                .count(),
            1
        );

        // Removing the entry and cleaning up deletes the PNG file
        let remaining: Vec<ClipboardItem> = Vec::new();
        cleanup_orphaned_clipboard_images(dir.path(), &remaining);
        assert!(!stored_path.exists());
    }

    #[test]
    fn checklist_notes_roundtrip_and_legacy_deserialization() {
        let legacy = r#"{"notes":[{"id":"n-1","title":"Old","body":"Plain","updatedAtMs":10}],"snippets":[]}"#;
        let parsed: NotesStoreData = serde_json::from_str(legacy).expect("legacy notes parse");
        assert_eq!(parsed.notes[0].kind, NoteKind::Note);
        assert!(parsed.notes[0].items.is_empty());

        let checklist = NotesStoreData {
            notes: vec![NoteItem {
                id: "n-2".into(),
                title: "Today".into(),
                body: String::new(),
                kind: NoteKind::Checklist,
                items: vec![
                    ChecklistItem {
                        id: "c-1".into(),
                        text: "Check autostart".into(),
                        done: true,
                    },
                    ChecklistItem {
                        id: "c-2".into(),
                        text: "Test v0.3.0".into(),
                        done: false,
                    },
                ],
                updated_at_ms: 20,
            }],
            snippets: vec![],
        };
        let encoded = serde_json::to_string(&checklist).unwrap();
        let round: NotesStoreData = serde_json::from_str(&encoded).unwrap();
        assert_eq!(round.notes[0].kind, NoteKind::Checklist);
        assert_eq!(round.notes[0].items.len(), 2);
        assert!(round.notes[0].items[0].done);
        assert!(!round.notes[0].items[1].done);
    }
}
