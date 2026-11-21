use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: String,
    pub pub_date: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStateSnapshot {
    pub status: String,
    pub current_version: String,
    pub available: Option<UpdateInfo>,
    pub progress_percent: Option<u8>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    pub dismissed: bool,
}

pub struct UpdaterState {
    snapshot: Mutex<UpdateStateSnapshot>,
    pending: tokio::sync::Mutex<Option<Update>>,
}

impl UpdaterState {
    pub fn new(current_version: String) -> Self {
        Self {
            snapshot: Mutex::new(UpdateStateSnapshot {
                status: "idle".into(),
                current_version,
                available: None,
                progress_percent: None,
                downloaded_bytes: 0,
                total_bytes: None,
                error: None,
                dismissed: false,
            }),
            pending: tokio::sync::Mutex::new(None),
        }
    }

    pub fn snapshot(&self) -> UpdateStateSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    fn emit_state(&self, app: &AppHandle) -> UpdateStateSnapshot {
        let snap = self.snapshot();
        let _ = app.emit("app-update-state", &snap);
        snap
    }
}

pub fn compute_progress_percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total?;
    if total == 0 {
        return None;
    }
    let pct = ((downloaded as f64 / total as f64) * 100.0).round();
    Some(pct.clamp(0.0, 100.0) as u8)
}

pub fn friendly_update_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("404")
        || lower.contains("release json")
        || lower.contains("valid release")
        || lower.contains("not found")
    {
        return "No published release feed found yet".into();
    }
    if lower.contains("timed out") || lower.contains("dns") || lower.contains("connect") {
        return "Could not reach GitHub update feed".into();
    }
    let clean = raw.lines().next().unwrap_or(raw).trim();
    if clean.is_empty() {
        "Update check failed".into()
    } else if clean.chars().count() > 140 {
        let prefix: String = clean.chars().take(137).collect();
        format!("{prefix}...")
    } else {
        clean.to_string()
    }
}

pub fn summarize_release_notes(body: Option<&str>) -> String {
    let Some(raw) = body else {
        return String::new();
    };
    let lines: Vec<String> = raw
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim().to_string())
        .filter(|l| !l.is_empty())
        .take(2)
        .collect();
    let joined = lines.join(" — ");
    if joined.chars().count() > 120 {
        let prefix: String = joined.chars().take(117).collect();
        format!("{prefix}...")
    } else {
        joined
    }
}

async fn run_check(
    app: &AppHandle,
    state: &UpdaterState,
    manual: bool,
) -> Result<UpdateStateSnapshot, String> {
    {
        let mut guard = state.snapshot.lock().unwrap();
        if matches!(guard.status.as_str(), "downloading" | "installing") {
            return Ok(guard.clone());
        }
        guard.status = "checking".into();
        guard.error = None;
    }
    state.emit_state(app);

    let checker = app.updater().map_err(|e| e.to_string())?;
    match checker.check().await {
        Ok(Some(update)) => {
            let info = UpdateInfo {
                version: update.version.clone(),
                current_version: update.current_version.clone(),
                notes: summarize_release_notes(update.body.as_deref()),
                pub_date: update.date.map(|d| d.to_string()),
            };
            *state.pending.lock().await = Some(update);
            {
                let mut guard = state.snapshot.lock().unwrap();
                guard.status = "available".into();
                guard.available = Some(info);
                guard.progress_percent = None;
                guard.downloaded_bytes = 0;
                guard.total_bytes = None;
                guard.error = None;
                guard.dismissed = false;
            }
            Ok(state.emit_state(app))
        }
        Ok(None) => {
            *state.pending.lock().await = None;
            {
                let mut guard = state.snapshot.lock().unwrap();
                guard.status = if manual {
                    "upToDate".into()
                } else {
                    "idle".into()
                };
                guard.available = None;
                guard.progress_percent = None;
                guard.downloaded_bytes = 0;
                guard.total_bytes = None;
                guard.error = None;
                guard.dismissed = false;
            }
            Ok(state.emit_state(app))
        }
        Err(err) => {
            let msg = friendly_update_error(&err.to_string());
            {
                let mut guard = state.snapshot.lock().unwrap();
                if manual {
                    guard.status = "error".into();
                    guard.error = Some(msg);
                } else {
                    guard.status = "idle".into();
                    guard.error = None;
                }
            }
            Ok(state.emit_state(app))
        }
    }
}

pub fn spawn_startup_check(app: AppHandle, state: Arc<UpdaterState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        let _ = run_check(&app, &state, false).await;
    });
}

#[tauri::command]
pub fn get_update_state(state: State<'_, Arc<UpdaterState>>) -> UpdateStateSnapshot {
    state.snapshot()
}

#[tauri::command]
pub async fn check_for_updates(
    app: AppHandle,
    state: State<'_, Arc<UpdaterState>>,
    manual: Option<bool>,
) -> Result<UpdateStateSnapshot, String> {
    run_check(&app, state.inner(), manual.unwrap_or(true)).await
}

#[tauri::command]
pub async fn install_available_update(
    app: AppHandle,
    state: State<'_, Arc<UpdaterState>>,
) -> Result<(), String> {
    let update = {
        let existing = state.pending.lock().await.take();
        if let Some(u) = existing {
            u
        } else {
            let checker = app.updater().map_err(|e| e.to_string())?;
            checker
                .check()
                .await
                .map_err(|e| friendly_update_error(&e.to_string()))?
                .ok_or_else(|| "No update available".to_string())?
        }
    };

    {
        let mut guard = state.snapshot.lock().unwrap();
        guard.status = "downloading".into();
        guard.progress_percent = Some(0);
        guard.downloaded_bytes = 0;
        guard.total_bytes = None;
        guard.error = None;
        guard.dismissed = false;
    }
    state.emit_state(&app);

    let downloaded = Arc::new(AtomicU64::new(0));
    let last_emitted_pct = Arc::new(AtomicU8::new(0));
    let state_dl = state.inner().clone();
    let app_dl = app.clone();
    let downloaded_cb = downloaded.clone();
    let last_pct_cb = last_emitted_pct.clone();

    let state_done = state.inner().clone();
    let app_done = app.clone();

    let install_res = update
        .download_and_install(
            move |chunk_len, total_len| {
                let cur =
                    downloaded_cb.fetch_add(chunk_len as u64, Ordering::SeqCst) + chunk_len as u64;
                let pct = compute_progress_percent(cur, total_len);
                let prev = last_pct_cb.load(Ordering::Relaxed);
                let should_emit = match pct {
                    Some(p) => p == 100 || p.saturating_sub(prev) >= 2,
                    None => true,
                };
                if should_emit {
                    if let Some(p) = pct {
                        last_pct_cb.store(p, Ordering::Relaxed);
                    }
                    {
                        let mut guard = state_dl.snapshot.lock().unwrap();
                        guard.downloaded_bytes = cur;
                        guard.total_bytes = total_len;
                        guard.progress_percent = pct;
                    }
                    state_dl.emit_state(&app_dl);
                }
            },
            move || {
                {
                    let mut guard = state_done.snapshot.lock().unwrap();
                    guard.status = "installing".into();
                    guard.progress_percent = Some(100);
                }
                state_done.emit_state(&app_done);
            },
        )
        .await;

    match install_res {
        Ok(()) => {
            app.restart();
        }
        Err(err) => {
            let msg = friendly_update_error(&err.to_string());
            {
                let mut guard = state.snapshot.lock().unwrap();
                guard.status = "error".into();
                guard.error = Some(msg.clone());
            }
            state.emit_state(&app);
            Err(msg)
        }
    }
}

#[tauri::command]
pub fn dismiss_update_prompt(
    app: AppHandle,
    state: State<'_, Arc<UpdaterState>>,
) -> UpdateStateSnapshot {
    {
        let mut guard = state.snapshot.lock().unwrap();
        guard.dismissed = true;
    }
    state.emit_state(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_bounded_progress_percentages() {
        assert_eq!(compute_progress_percent(0, None), None);
        assert_eq!(compute_progress_percent(100, Some(0)), None);
        assert_eq!(compute_progress_percent(500, Some(1000)), Some(50));
        assert_eq!(compute_progress_percent(1200, Some(1000)), Some(100));
    }

    #[test]
    fn maps_release_errors_and_notes_cleanly() {
        assert_eq!(
            friendly_update_error("Could not fetch a valid release JSON from the remote: 404"),
            "No published release feed found yet"
        );
        assert_eq!(
            friendly_update_error("error sending request: timed out"),
            "Could not reach GitHub update feed"
        );
        assert_eq!(
            summarize_release_notes(Some("## Peek v0.2.0\n\n- Improved cards\n- Bug fixes")),
            "Peek v0.2.0 — - Improved cards"
        );
        assert_eq!(summarize_release_notes(None), "");
    }
}
