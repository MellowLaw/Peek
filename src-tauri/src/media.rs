use base64::{engine::general_purpose::STANDARD, Engine};
use image::{codecs::jpeg::JpegEncoder, ImageReader};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    future::IntoFuture,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, WebviewWindow};
use tokio::sync::{watch, Notify};
use windows::{
    Foundation::TypedEventHandler,
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as SessionManager,
        GlobalSystemMediaTransportControlsSessionMediaProperties as MediaProperties,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    },
    Storage::Streams::DataReader,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(4);
const ARTWORK_RETRY_INTERVAL: Duration = Duration::from_millis(700);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_ARTWORK_SIZE: usize = 8 * 1024 * 1024;
const MAX_ARTWORK_RETRY_ATTEMPTS: u8 = 6;
const MAX_CACHED_COVERS: usize = 120;
const TICKS_PER_SECOND: f64 = 10_000_000.0;
const WINRT_EPOCH_OFFSET: f64 = 11_644_473_600.0;

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum Snapshot {
    Empty,
    Unavailable,
    Ready { track: Track },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionSummary {
    pub source_id: String,
    pub name: String,
    pub is_current: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Track {
    session_id: u64,
    source_id: String,
    title: String,
    artist: String,
    album: String,
    artwork: Option<String>,
    playing: bool,
    can_play_pause: bool,
    can_previous: bool,
    can_next: bool,
    can_seek: bool,
    position_seconds: f64,
    duration_seconds: f64,
    rate: f64,
    sampled_at_ms: f64,
    sessions: Vec<MediaSessionSummary>,
}

#[derive(Clone)]
struct ActiveSession {
    id: u64,
    session: Session,
    source_id: String,
}

struct ArtworkCache {
    key: Option<(String, String, String)>,
    url: Option<String>,
    native_loaded: bool,
    attempts: u8,
    next_retry: Option<Instant>,
    fallback_started: bool,
    pending_fallback: Arc<Mutex<Option<((String, String, String), String)>>>,
}

impl Default for ArtworkCache {
    fn default() -> Self {
        Self {
            key: None,
            url: None,
            native_loaded: false,
            attempts: 0,
            next_retry: None,
            fallback_started: false,
            pending_fallback: Arc::new(Mutex::new(None)),
        }
    }
}

struct SessionWatch {
    active: ActiveSession,
    media_token: Option<i64>,
    playback_token: Option<i64>,
    timeline_token: Option<i64>,
    artwork: ArtworkCache,
}

impl SessionWatch {
    fn new(active: ActiveSession, wake: &Arc<Notify>) -> Self {
        let media_wake = wake.clone();
        let playback_wake = wake.clone();
        let timeline_wake = wake.clone();
        Self {
            media_token: active
                .session
                .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    media_wake.notify_one();
                    Ok(())
                }))
                .ok(),
            playback_token: active
                .session
                .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                    playback_wake.notify_one();
                    Ok(())
                }))
                .ok(),
            timeline_token: active
                .session
                .TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    timeline_wake.notify_one();
                    Ok(())
                }))
                .ok(),
            active,
            artwork: ArtworkCache::default(),
        }
    }
}

impl Drop for SessionWatch {
    fn drop(&mut self) {
        let session = &self.active.session;
        if let Some(token) = self.media_token {
            let _ = session.RemoveMediaPropertiesChanged(token);
        }
        if let Some(token) = self.playback_token {
            let _ = session.RemovePlaybackInfoChanged(token);
        }
        if let Some(token) = self.timeline_token {
            let _ = session.RemoveTimelinePropertiesChanged(token);
        }
    }
}

struct ManagerWatch {
    manager: SessionManager,
    current_token: Option<i64>,
    sessions_token: Option<i64>,
}

impl ManagerWatch {
    fn new(manager: SessionManager, wake: &Arc<Notify>) -> Self {
        let current_wake = wake.clone();
        let sessions_wake = wake.clone();
        Self {
            current_token: manager
                .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                    current_wake.notify_one();
                    Ok(())
                }))
                .ok(),
            sessions_token: manager
                .SessionsChanged(&TypedEventHandler::new(move |_, _| {
                    sessions_wake.notify_one();
                    Ok(())
                }))
                .ok(),
            manager,
        }
    }
}

impl Drop for ManagerWatch {
    fn drop(&mut self) {
        if let Some(token) = self.current_token {
            let _ = self.manager.RemoveCurrentSessionChanged(token);
        }
        if let Some(token) = self.sessions_token {
            let _ = self.manager.RemoveSessionsChanged(token);
        }
    }
}

#[derive(Clone)]
pub struct MediaState {
    enabled: watch::Sender<bool>,
    current: Arc<Mutex<Option<ActiveSession>>>,
    preferred_source: Arc<Mutex<String>>,
    wake: Arc<Notify>,
}

pub fn start(window: WebviewWindow) -> MediaState {
    let (enabled, mut activity) = watch::channel(false);
    let current = Arc::new(Mutex::new(None));
    let preferred_source = Arc::new(Mutex::new(String::new()));
    let wake = Arc::new(Notify::new());
    let worker_current = current.clone();
    let worker_preferred = preferred_source.clone();
    let worker_wake = wake.clone();
    let cache_dir = window
        .app_handle()
        .path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("artwork_cache"));
    tauri::async_runtime::spawn(async move {
        let mut next_id = 0;
        while activity.wait_for(|enabled| *enabled).await.is_ok() {
            tokio::select! {
                biased;
                _ = activity.wait_for(|enabled| !*enabled) => {}
                _ = monitor(
                    &window,
                    &worker_current,
                    &worker_preferred,
                    &worker_wake,
                    &cache_dir,
                    &mut next_id,
                ) => {}
            }
            *worker_current.lock().unwrap() = None;
        }
    });
    MediaState {
        enabled,
        current,
        preferred_source,
        wake,
    }
}

#[tauri::command]
pub fn watch_media(state: tauri::State<'_, MediaState>, enabled: bool) {
    state.enabled.send_replace(enabled);
}

#[tauri::command]
pub fn select_media_session(state: tauri::State<'_, MediaState>, source_id: String) {
    *state.preferred_source.lock().unwrap() = source_id;
    state.wake.notify_one();
}

fn friendly_source_name(source_id: &str) -> String {
    if let Ok(info) = windows::ApplicationModel::AppInfo::GetFromAppUserModelId(
        &windows::core::HSTRING::from(source_id),
    ) {
        if let Ok(display) = info.DisplayInfo() {
            if let Ok(name) = display.DisplayName() {
                let s = name.to_string_lossy();
                if !s.trim().is_empty() {
                    return s;
                }
            }
        }
    }
    let tail = source_id
        .rsplit(['\\', '/', '!'])
        .next()
        .unwrap_or(source_id)
        .trim_end_matches(".exe");
    match tail.to_lowercase().as_str() {
        "chrome" => "Google Chrome".into(),
        "msedge" => "Microsoft Edge".into(),
        "firefox" => "Firefox".into(),
        "spotify" => "Spotify".into(),
        "vlc" => "VLC".into(),
        "" => "Media player".into(),
        _ => tail.to_string(),
    }
}

fn pick_session_and_list(
    manager: &SessionManager,
    preferred_source: &str,
) -> (Option<(Session, String)>, Vec<MediaSessionSummary>) {
    let current_session = manager.GetCurrentSession().ok();
    let current_source = current_session
        .as_ref()
        .and_then(|s| s.SourceAppUserModelId().ok())
        .map(|s| s.to_string_lossy());

    let mut summaries = Vec::new();
    let mut preferred_match: Option<(Session, String)> = None;

    if let Ok(sessions) = manager.GetSessions() {
        for session in sessions {
            let sid = session
                .SourceAppUserModelId()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            if sid.is_empty() {
                continue;
            }
            if !preferred_source.is_empty() && sid == preferred_source && preferred_match.is_none()
            {
                preferred_match = Some((session, sid.clone()));
            }
            if !summaries
                .iter()
                .any(|s: &MediaSessionSummary| s.source_id == sid)
            {
                summaries.push(MediaSessionSummary {
                    is_current: current_source.as_deref() == Some(sid.as_str()),
                    name: friendly_source_name(&sid),
                    source_id: sid,
                });
            }
        }
    }

    let chosen = preferred_match.or_else(|| {
        current_session.map(|s| {
            let sid = s
                .SourceAppUserModelId()
                .map(|v| v.to_string_lossy())
                .unwrap_or_default();
            (s, sid)
        })
    });
    (chosen, summaries)
}

async fn monitor(
    window: &WebviewWindow,
    current: &Mutex<Option<ActiveSession>>,
    preferred_source: &Mutex<String>,
    wake: &Arc<Notify>,
    cache_dir: &Option<PathBuf>,
    next_id: &mut u64,
) {
    let manager = loop {
        if let Ok(request) = SessionManager::RequestAsync() {
            if let Ok(Ok(manager)) =
                tokio::time::timeout(REQUEST_TIMEOUT, request.into_future()).await
            {
                break ManagerWatch::new(manager, wake);
            }
        }
        let _ = window.emit("media-update", Snapshot::Unavailable);
        tokio::time::sleep(REFRESH_INTERVAL).await;
    };
    let mut watched: Option<SessionWatch> = None;

    loop {
        let preferred = preferred_source.lock().unwrap().clone();
        let (chosen, sessions) = pick_session_and_list(&manager.manager, &preferred);
        let chosen_session = chosen.as_ref().map(|(s, _)| s);
        if watched.as_ref().map(|watch| &watch.active.session) != chosen_session {
            watched = chosen.map(|(session, source_id)| {
                *next_id += 1;
                SessionWatch::new(
                    ActiveSession {
                        id: *next_id,
                        session,
                        source_id,
                    },
                    wake,
                )
            });
            *current.lock().unwrap() = watched.as_ref().map(|watch| watch.active.clone());
        }
        let snapshot = match &mut watched {
            Some(watch) => {
                let track = read_track(watch, sessions, cache_dir, wake).await;
                Snapshot::Ready { track }
            }
            None => Snapshot::Empty,
        };
        let _ = window.emit("media-update", snapshot);
        let wait_dur = match &watched {
            Some(w)
                if w.artwork.url.is_none()
                    && !w.artwork.native_loaded
                    && w.artwork.attempts < MAX_ARTWORK_RETRY_ATTEMPTS =>
            {
                ARTWORK_RETRY_INTERVAL
            }
            _ => REFRESH_INTERVAL,
        };
        let _ = tokio::time::timeout(wait_dur, wake.notified()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn read_track(
    watch: &mut SessionWatch,
    sessions: Vec<MediaSessionSummary>,
    cache_dir: &Option<PathBuf>,
    wake: &Arc<Notify>,
) -> Track {
    let session = &watch.active.session;
    let properties = if let Ok(request) = session.TryGetMediaPropertiesAsync() {
        tokio::time::timeout(REQUEST_TIMEOUT, request.into_future())
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let (title, artist, album) = properties
        .as_ref()
        .map(|properties| {
            (
                properties.Title().unwrap_or_default().to_string_lossy(),
                properties.Artist().unwrap_or_default().to_string_lossy(),
                properties
                    .AlbumTitle()
                    .unwrap_or_default()
                    .to_string_lossy(),
            )
        })
        .unwrap_or_default();
    let key = (title.clone(), artist.clone(), album.clone());
    if watch.artwork.key.as_ref() != Some(&key) {
        let cached = cache_dir
            .as_deref()
            .and_then(|dir| load_cached_artwork(dir, &title, &artist, &album));
        *watch.artwork.pending_fallback.lock().unwrap() = None;
        watch.artwork.key = Some(key.clone());
        watch.artwork.url = cached;
        watch.artwork.native_loaded = false;
        watch.artwork.attempts = 0;
        watch.artwork.next_retry = Some(Instant::now());
        watch.artwork.fallback_started = false;
    }

    if watch.artwork.url.is_none() {
        let pending = watch.artwork.pending_fallback.lock().unwrap().take();
        if let Some((fb_key, fb_url)) = pending {
            if fb_key == key {
                watch.artwork.url = Some(fb_url);
            }
        }
    }

    let retry_due = watch
        .artwork
        .next_retry
        .is_none_or(|deadline| Instant::now() >= deadline);
    if !watch.artwork.native_loaded
        && watch.artwork.attempts < MAX_ARTWORK_RETRY_ATTEMPTS
        && retry_due
    {
        watch.artwork.attempts += 1;
        watch.artwork.next_retry = Some(Instant::now() + ARTWORK_RETRY_INTERVAL);
        if let Some(properties) = &properties {
            if let Some((data_url, jpg_opt)) =
                tokio::time::timeout(Duration::from_millis(1500), read_artwork(properties))
                    .await
                    .ok()
                    .flatten()
            {
                if let (Some(dir), Some(jpg)) = (cache_dir.as_deref(), jpg_opt.as_ref()) {
                    save_cached_artwork(dir, &title, &artist, &album, jpg);
                }
                watch.artwork.url = Some(data_url);
                watch.artwork.native_loaded = true;
            }
        }
    }

    if watch.artwork.url.is_none()
        && watch.artwork.attempts >= 2
        && !watch.artwork.fallback_started
        && !title.trim().is_empty()
    {
        watch.artwork.fallback_started = true;
        let slot = watch.artwork.pending_fallback.clone();
        let wake_ref = wake.clone();
        let dir_clone = cache_dir.clone();
        let fb_key = key;
        let t = title.clone();
        let a = artist.clone();
        let al = album.clone();
        tauri::async_runtime::spawn(async move {
            let fetched = tokio::task::spawn_blocking(move || {
                fetch_itunes_artwork(&t, &a, &al, dir_clone.as_deref())
            })
            .await
            .ok()
            .flatten();
            if let Some(data_url) = fetched {
                *slot.lock().unwrap() = Some((fb_key, data_url));
                wake_ref.notify_one();
            }
        });
    }
    let info = session.GetPlaybackInfo().ok();
    let playing =
        info.as_ref().and_then(|info| info.PlaybackStatus().ok()) == Some(PlaybackStatus::Playing);
    let controls = info.as_ref().and_then(|info| info.Controls().ok());
    let rate = info
        .as_ref()
        .and_then(|info| info.PlaybackRate().ok())
        .and_then(|rate| rate.Value().ok())
        .filter(|rate| rate.is_finite() && *rate >= 0.0)
        .unwrap_or(1.0);
    let sampled_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let (position_seconds, duration_seconds) = session
        .GetTimelineProperties()
        .ok()
        .map(|timeline| {
            let start = timeline.StartTime().unwrap_or_default().Duration as f64 / TICKS_PER_SECOND;
            let end = timeline.EndTime().unwrap_or_default().Duration as f64 / TICKS_PER_SECOND;
            let position =
                timeline.Position().unwrap_or_default().Duration as f64 / TICKS_PER_SECOND;
            let updated_at = timeline.LastUpdatedTime().unwrap_or_default().UniversalTime as f64
                / TICKS_PER_SECOND
                - WINRT_EPOCH_OFFSET;
            let elapsed = if playing && updated_at > 0.0 {
                (sampled_at - updated_at).max(0.0) * rate
            } else {
                0.0
            };
            let duration = (end - start).max(0.0);
            ((position - start + elapsed).clamp(0.0, duration), duration)
        })
        .unwrap_or_default();

    Track {
        session_id: watch.active.id,
        source_id: watch.active.source_id.clone(),
        title,
        artist,
        album,
        artwork: watch.artwork.url.clone(),
        playing,
        can_play_pause: controls.as_ref().is_some_and(|controls| {
            controls.IsPlayPauseToggleEnabled().unwrap_or(false)
                || if playing {
                    controls.IsPauseEnabled().unwrap_or(false)
                } else {
                    controls.IsPlayEnabled().unwrap_or(false)
                }
        }),
        can_previous: controls
            .as_ref()
            .is_some_and(|controls| controls.IsPreviousEnabled().unwrap_or(false)),
        can_next: controls
            .as_ref()
            .is_some_and(|controls| controls.IsNextEnabled().unwrap_or(false)),
        can_seek: controls
            .as_ref()
            .is_some_and(|controls| controls.IsPlaybackPositionEnabled().unwrap_or(false)),
        position_seconds,
        duration_seconds,
        rate,
        sampled_at_ms: sampled_at * 1000.0,
        sessions,
    }
}

fn fnv1a_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in input.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn artwork_cache_filename(title: &str, artist: &str, album: &str) -> Option<String> {
    let a = artist.trim().to_lowercase();
    let al = album.trim().to_lowercase();
    let t = title.trim().to_lowercase();
    if t.is_empty() && a.is_empty() && al.is_empty() {
        return None;
    }
    let second = if !al.is_empty() { al } else { t };
    Some(format!("{}.jpg", fnv1a_hex(&format!("{a}\n{second}"))))
}

fn load_cached_artwork(dir: &Path, title: &str, artist: &str, album: &str) -> Option<String> {
    let name = artwork_cache_filename(title, artist, album)?;
    let path = dir.join(name);
    let bytes = fs::read(&path).ok()?;
    if bytes.len() < 4 || &bytes[..3] != b"\xFF\xD8\xFF" {
        return None;
    }
    Some(format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)))
}

fn save_cached_artwork(dir: &Path, title: &str, artist: &str, album: &str, jpeg_bytes: &[u8]) {
    let Some(name) = artwork_cache_filename(title, artist, album) else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let target = dir.join(name);
    let tmp = target.with_extension("tmp");
    if fs::write(&tmp, jpeg_bytes).is_ok() {
        let _ = fs::rename(&tmp, &target);
    }
    let _ = fs::remove_file(&tmp);

    if let Ok(entries) = fs::read_dir(dir) {
        let mut files: Vec<(PathBuf, SystemTime)> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) != Some("jpg") {
                    return None;
                }
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((p, mtime))
            })
            .collect();
        if files.len() > MAX_CACHED_COVERS {
            files.sort_by_key(|(_, t)| *t);
            let remove_count = files.len() - MAX_CACHED_COVERS;
            for (p, _) in files.into_iter().take(remove_count) {
                let _ = fs::remove_file(p);
            }
        }
    }
}

fn detect_magic_mime(raw: &[u8]) -> Option<&'static str> {
    if raw.len() >= 8 && raw.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if raw.len() >= 3 && raw.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if raw.len() >= 12 && raw.starts_with(b"RIFF") && &raw[8..12] == b"WEBP" {
        Some("image/webp")
    } else if raw.len() >= 6 && (raw.starts_with(b"GIF87a") || raw.starts_with(b"GIF89a")) {
        Some("image/gif")
    } else {
        None
    }
}

fn normalize_artwork_jpeg(raw: &[u8]) -> Option<Vec<u8>> {
    let img = ImageReader::new(std::io::Cursor::new(raw))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let rgb = img.thumbnail(512, 512).to_rgb8();
    let mut out = Vec::new();
    let enc = JpegEncoder::new_with_quality(&mut out, 86);
    if rgb.write_with_encoder(enc).is_ok() && !out.is_empty() {
        Some(out)
    } else {
        None
    }
}

fn normalize_artwork_bytes(raw: &[u8]) -> Option<(String, Option<Vec<u8>>)> {
    if raw.is_empty() || raw.len() > MAX_ARTWORK_SIZE {
        return None;
    }
    if let Some(jpg) = normalize_artwork_jpeg(raw) {
        let data_url = format!("data:image/jpeg;base64,{}", STANDARD.encode(&jpg));
        return Some((data_url, Some(jpg)));
    }
    if raw.len() <= 512 * 1024 {
        if let Some(mime) = detect_magic_mime(raw) {
            return Some((format!("data:{mime};base64,{}", STANDARD.encode(raw)), None));
        }
    }
    None
}

fn norm_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn pick_itunes_artwork_url(body: &[u8], title: &str, artist: &str) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let results = v.get("results")?.as_array()?;
    let title_words = norm_words(title);
    let artist_words = norm_words(artist);

    let mut best_url: Option<String> = None;
    let mut best_score: i32 = i32::MIN;
    for item in results {
        let url = item
            .get("artworkUrl100")
            .or_else(|| item.get("artworkUrl60"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if url.is_empty() {
            continue;
        }
        let item_title = norm_words(item.get("trackName").and_then(Value::as_str).unwrap_or(""));
        let item_artist = norm_words(item.get("artistName").and_then(Value::as_str).unwrap_or(""));

        let t_hits = title_words
            .iter()
            .filter(|w| item_title.contains(*w))
            .count() as i32;
        let a_hits = artist_words
            .iter()
            .filter(|w| item_artist.contains(*w))
            .count() as i32;
        if !title_words.is_empty() && t_hits == 0 {
            continue;
        }
        if !artist_words.is_empty() && a_hits == 0 {
            continue;
        }
        let score = t_hits * 3 + a_hits * 3;
        if score > best_score {
            best_score = score;
            best_url = Some(url.to_string());
        }
    }

    best_url.map(|u| {
        if u.contains("100x100bb") {
            u.replace("100x100bb", "600x600bb")
        } else if u.contains("60x60bb") {
            u.replace("60x60bb", "600x600bb")
        } else {
            u.replace("100x100", "600x600")
        }
    })
}

fn parse_https_mzstatic_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path_part) = rest.split_once('/')?;
    if host.is_empty() || host.contains('@') || host.contains(':') {
        return None;
    }
    let lower = host.to_ascii_lowercase();
    if lower != "mzstatic.com" && !lower.ends_with(".mzstatic.com") {
        return None;
    }
    Some((host.to_string(), format!("/{path_part}")))
}

fn fetch_itunes_artwork(
    title: &str,
    artist: &str,
    album: &str,
    cache_dir: Option<&Path>,
) -> Option<String> {
    let term = if artist.trim().is_empty() {
        title.trim().to_string()
    } else {
        format!("{} {}", artist.trim(), title.trim())
    };
    if term.is_empty() {
        return None;
    }
    let path = format!(
        "/search?media=music&entity=song&limit=5&term={}",
        crate::lyrical::url_encode(&term)
    );
    let (status, _, body) = crate::lyrical::winhttp_get_bytes(
        "itunes.apple.com",
        &path,
        "Accept: application/json\r\n",
        512 * 1024,
    )
    .ok()?;
    if status != 200 {
        return None;
    }
    let artwork_url = pick_itunes_artwork_url(&body, title, artist)?;
    let (host, img_path) = parse_https_mzstatic_url(&artwork_url)?;
    let (img_status, _, img_bytes) = crate::lyrical::winhttp_get_bytes(
        &host,
        &img_path,
        "Accept: image/*,*/*\r\n",
        4 * 1024 * 1024,
    )
    .ok()?;
    if img_status != 200 {
        return None;
    }
    let (data_url, jpg_opt) = normalize_artwork_bytes(&img_bytes)?;
    if let (Some(dir), Some(jpg)) = (cache_dir, jpg_opt.as_ref()) {
        save_cached_artwork(dir, title, artist, album, jpg);
    }
    Some(data_url)
}

async fn read_artwork(properties: &MediaProperties) -> Option<(String, Option<Vec<u8>>)> {
    const CHUNK: u32 = 65_536;
    let (reader, expected) = {
        let open = properties.Thumbnail().ok()?.OpenReadAsync().ok()?;
        let stream = open.await.ok()?;
        let _ = stream.Seek(0);
        let size = stream.Size().ok().unwrap_or(0);
        if size > MAX_ARTWORK_SIZE as u64 {
            return None;
        }
        (DataReader::CreateDataReader(&stream).ok()?, size)
    };
    let mut raw = Vec::with_capacity((expected as usize).min(262_144));
    loop {
        let remaining = if expected > 0 {
            expected.saturating_sub(raw.len() as u64).min(CHUNK as u64) as u32
        } else {
            CHUNK
        };
        if remaining == 0 {
            break;
        }
        let loaded = reader.LoadAsync(remaining).ok()?.await.ok()?;
        if loaded == 0 {
            break;
        }
        if raw.len() + loaded as usize > MAX_ARTWORK_SIZE {
            return None;
        }
        let mut chunk = vec![0u8; loaded as usize];
        reader.ReadBytes(&mut chunk).ok()?;
        raw.extend_from_slice(&chunk);
        if expected > 0 && raw.len() as u64 >= expected {
            break;
        }
    }
    if raw.is_empty() {
        return None;
    }
    tokio::task::spawn_blocking(move || normalize_artwork_bytes(&raw))
        .await
        .ok()
        .flatten()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaAction {
    PlayPause,
    Previous,
    Next,
}

#[tauri::command]
pub async fn control_media(
    state: tauri::State<'_, MediaState>,
    session_id: u64,
    action: MediaAction,
) -> Result<(), String> {
    let active = state
        .current
        .lock()
        .unwrap()
        .clone()
        .filter(|active| active.id == session_id)
        .ok_or("The current player changed")?;
    if !*state.enabled.borrow() {
        return Err("Media controls are paused".into());
    }
    let session = active.session;
    let info = session
        .GetPlaybackInfo()
        .map_err(|error| error.to_string())?;
    let controls = info.Controls().map_err(|error| error.to_string())?;
    let playing = info.PlaybackStatus().ok() == Some(PlaybackStatus::Playing);
    let operation = match action {
        MediaAction::Previous if controls.IsPreviousEnabled().unwrap_or(false) => {
            session.TrySkipPreviousAsync()
        }
        MediaAction::Next if controls.IsNextEnabled().unwrap_or(false) => {
            session.TrySkipNextAsync()
        }
        MediaAction::PlayPause if controls.IsPlayPauseToggleEnabled().unwrap_or(false) => {
            session.TryTogglePlayPauseAsync()
        }
        MediaAction::PlayPause if playing && controls.IsPauseEnabled().unwrap_or(false) => {
            session.TryPauseAsync()
        }
        MediaAction::PlayPause if !playing && controls.IsPlayEnabled().unwrap_or(false) => {
            session.TryPlayAsync()
        }
        _ => return Err("This player does not support the action".into()),
    }
    .map_err(|error| error.to_string())?;
    let accepted = tokio::time::timeout(REQUEST_TIMEOUT, operation.into_future())
        .await
        .map_err(|_| "The player did not respond")?
        .map_err(|error| error.to_string())?;
    state.wake.notify_one();
    if accepted {
        Ok(())
    } else {
        Err("The player declined the action".into())
    }
}

#[tauri::command]
pub async fn seek_media(
    state: tauri::State<'_, MediaState>,
    session_id: u64,
    position_seconds: f64,
) -> Result<(), String> {
    if !position_seconds.is_finite() || position_seconds < 0.0 {
        return Err("Invalid seek position".into());
    }
    let active = state
        .current
        .lock()
        .unwrap()
        .clone()
        .filter(|active| active.id == session_id)
        .ok_or("The current player changed")?;
    let session = active.session;
    let info = session.GetPlaybackInfo().map_err(|e| e.to_string())?;
    let controls = info.Controls().map_err(|e| e.to_string())?;
    if !controls.IsPlaybackPositionEnabled().unwrap_or(false) {
        return Err("This player does not support seeking".into());
    }
    let start_ticks = session
        .GetTimelineProperties()
        .ok()
        .and_then(|t| t.StartTime().ok())
        .map(|s| s.Duration)
        .unwrap_or(0);
    let target_ticks = start_ticks + (position_seconds * TICKS_PER_SECOND).round() as i64;
    let op = session
        .TryChangePlaybackPositionAsync(target_ticks)
        .map_err(|e| e.to_string())?;
    let accepted = tokio::time::timeout(REQUEST_TIMEOUT, op.into_future())
        .await
        .map_err(|_| "Seek timed out")?
        .map_err(|e| e.to_string())?;
    state.wake.notify_one();
    if accepted {
        Ok(())
    } else {
        Err("The player declined to seek".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_png_and_bmp_artwork_to_jpeg_data_url() {
        for fmt in [image::ImageFormat::Png, image::ImageFormat::Bmp] {
            let mut img = image::RgbImage::new(4, 4);
            for p in img.pixels_mut() {
                *p = image::Rgb([40, 120, 220]);
            }
            let mut encoded = Vec::new();
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut std::io::Cursor::new(&mut encoded), fmt)
                .unwrap();
            let (url, jpg) = normalize_artwork_bytes(&encoded).expect("normalized image");
            assert!(url.starts_with("data:image/jpeg;base64,"));
            assert!(jpg.is_some());
        }
        assert!(normalize_artwork_bytes(b"not an image").is_none());
        assert!(normalize_artwork_bytes(b"").is_none());
    }

    #[test]
    fn picks_matching_itunes_artwork_and_upgrades_resolution() {
        let json = br#"{"results":[
            {"trackName":"Other Song","artistName":"Someone Else","artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/v4/wrong/100x100bb.jpg"},
            {"trackName":"Totoong tayo","artistName":"Jin DC","artworkUrl100":"https://is2-ssl.mzstatic.com/image/thumb/Music/v4/right/100x100bb.jpg"}
        ]}"#;
        let url = pick_itunes_artwork_url(json, "Totoong tayo", "Jin DC").unwrap();
        assert_eq!(
            url,
            "https://is2-ssl.mzstatic.com/image/thumb/Music/v4/right/600x600bb.jpg"
        );
        assert_eq!(
            parse_https_mzstatic_url(&url),
            Some((
                "is2-ssl.mzstatic.com".to_string(),
                "/image/thumb/Music/v4/right/600x600bb.jpg".to_string()
            ))
        );
        assert!(parse_https_mzstatic_url("http://is1-ssl.mzstatic.com/a.jpg").is_none());
        assert!(parse_https_mzstatic_url("https://evil.example.com/a.jpg").is_none());
    }
}
