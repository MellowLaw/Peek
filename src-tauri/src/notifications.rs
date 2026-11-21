use serde::Serialize;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{watch, Notify};

use crate::activity::{ActivityManager, Priority};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum WindowsNotificationStatus {
    Disabled,
    Allowed,
    Denied { reason: String },
    Unspecified { reason: String },
    Unavailable { reason: String },
}

pub struct NotificationsBridgeState {
    enabled: watch::Sender<bool>,
    status: Arc<Mutex<WindowsNotificationStatus>>,
}

pub fn start(app: AppHandle, initial_enabled: bool) -> NotificationsBridgeState {
    let (enabled, mut rx) = watch::channel(initial_enabled);
    let status = Arc::new(Mutex::new(if initial_enabled {
        WindowsNotificationStatus::Unspecified {
            reason: "Checking Windows access…".into(),
        }
    } else {
        WindowsNotificationStatus::Disabled
    }));

    let status_clone = status.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if !*rx.borrow() {
                *status_clone.lock().unwrap() = WindowsNotificationStatus::Disabled;
                let _ = app.emit(
                    "windows-notifications-status",
                    WindowsNotificationStatus::Disabled,
                );
                if rx.changed().await.is_err() {
                    break;
                }
                continue;
            }

            #[cfg(windows)]
            run_listener_session(&app, &status_clone, &mut rx).await;

            #[cfg(not(windows))]
            {
                let st = WindowsNotificationStatus::Unavailable {
                    reason: "Windows only".into(),
                };
                *status_clone.lock().unwrap() = st.clone();
                let _ = app.emit("windows-notifications-status", st);
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
    });

    NotificationsBridgeState { enabled, status }
}

#[cfg(windows)]
async fn request_listener_and_token(
    wake: Arc<Notify>,
) -> Result<
    (
        windows::UI::Notifications::Management::UserNotificationListener,
        i64,
    ),
    WindowsNotificationStatus,
> {
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::Management::{
        UserNotificationListener, UserNotificationListenerAccessStatus,
    };

    tokio::task::spawn_blocking(move || {
        tauri::async_runtime::block_on(async move {
            let listener = UserNotificationListener::Current().map_err(|_| {
                WindowsNotificationStatus::Unavailable {
                    reason: "Windows notification listener requires packaged app identity or OS support".into(),
                }
            })?;
            let access = listener
                .RequestAccessAsync()
                .map_err(|e| WindowsNotificationStatus::Unavailable {
                    reason: format!("Notification access request failed: {e}"),
                })?
                .await
                .map_err(|e| WindowsNotificationStatus::Unavailable {
                    reason: format!("Notification access request failed: {e}"),
                })?;

            match access {
                UserNotificationListenerAccessStatus::Allowed => {
                    let token = listener
                        .NotificationChanged(&TypedEventHandler::new(move |_, _| {
                            wake.notify_one();
                            Ok(())
                        }))
                        .map_err(|e| WindowsNotificationStatus::Unavailable {
                            reason: format!("Could not subscribe to notifications: {e}"),
                        })?;
                    Ok((listener, token))
                }
                UserNotificationListenerAccessStatus::Denied => {
                    Err(WindowsNotificationStatus::Denied {
                        reason: "Blocked in Windows Settings -> Privacy -> Notifications".into(),
                    })
                }
                _ => Err(WindowsNotificationStatus::Unspecified {
                    reason: "Windows did not grant notification listener access for this unpackaged build".into(),
                }),
            }
        })
    })
    .await
    .map_err(|e| WindowsNotificationStatus::Unavailable {
        reason: e.to_string(),
    })?
}

#[cfg(windows)]
struct ToastSnapshot {
    id: u32,
    app_name: String,
    title: String,
    body: String,
}

#[cfg(windows)]
async fn fetch_toasts(
    listener: windows::UI::Notifications::Management::UserNotificationListener,
) -> Vec<ToastSnapshot> {
    use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};

    tokio::task::spawn_blocking(move || {
        tauri::async_runtime::block_on(async move {
            let Ok(op) = listener.GetNotificationsAsync(NotificationKinds::Toast) else {
                return Vec::new();
            };
            let Ok(list) = op.await else {
                return Vec::new();
            };
            let Ok(binding_name) = KnownNotificationBindings::ToastGeneric() else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for item in list {
                let Ok(id) = item.Id() else { continue };
                let app_name = item
                    .AppInfo()
                    .ok()
                    .and_then(|ai| ai.DisplayInfo().ok())
                    .and_then(|di| di.DisplayName().ok())
                    .map(|s| s.to_string_lossy())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "Windows".into());
                let mut lines = Vec::new();
                if let Ok(notif) = item.Notification() {
                    if let Ok(vis) = notif.Visual() {
                        if let Ok(binding) = vis.GetBinding(&binding_name) {
                            if let Ok(elems) = binding.GetTextElements() {
                                for el in elems {
                                    if let Ok(t) = el.Text() {
                                        let s = t.to_string_lossy();
                                        if !s.trim().is_empty() {
                                            lines.push(s);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                let title = lines.first().cloned().unwrap_or_else(|| app_name.clone());
                let body = if lines.len() > 1 {
                    lines[1..].join(" ")
                } else {
                    String::new()
                };
                out.push(ToastSnapshot {
                    id,
                    app_name,
                    title,
                    body,
                });
            }
            out
        })
    })
    .await
    .unwrap_or_default()
}

#[cfg(windows)]
async fn run_listener_session(
    app: &AppHandle,
    status: &Arc<Mutex<WindowsNotificationStatus>>,
    rx: &mut watch::Receiver<bool>,
) {
    let wake = Arc::new(Notify::new());
    match request_listener_and_token(wake.clone()).await {
        Ok((listener, token)) => {
            *status.lock().unwrap() = WindowsNotificationStatus::Allowed;
            let _ = app.emit(
                "windows-notifications-status",
                WindowsNotificationStatus::Allowed,
            );
            let mut seen: HashSet<u32> = HashSet::new();
            let mut seeded = false;

            loop {
                if !*rx.borrow() {
                    break;
                }
                let toasts = fetch_toasts(listener.clone()).await;
                let mut current_ids = HashSet::new();
                for t in toasts {
                    current_ids.insert(t.id);
                    if seeded && !seen.contains(&t.id) {
                        app.state::<ActivityManager>().push_activity(
                            t.app_name,
                            t.title,
                            t.body,
                            Priority::Normal,
                            4500,
                            Some("notifications"),
                            true,
                            true,
                        );
                    }
                }
                seen = current_ids;
                seeded = true;

                tokio::select! {
                    _ = rx.changed() => break,
                    _ = wake.notified() => {},
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {},
                }
            }
            let _ = listener.RemoveNotificationChanged(token);
        }
        Err(err_status) => {
            *status.lock().unwrap() = err_status.clone();
            let _ = app.emit("windows-notifications-status", err_status);
            let _ = rx.changed().await;
        }
    }
}

pub fn set_enabled(app: &AppHandle, enabled: bool) {
    app.state::<NotificationsBridgeState>()
        .enabled
        .send_replace(enabled);
}

#[tauri::command]
pub fn get_windows_notification_status(
    state: State<'_, NotificationsBridgeState>,
) -> WindowsNotificationStatus {
    state.status.lock().unwrap().clone()
}
