use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

const MAX_HISTORY: usize = 150;
const HISTORY_FILE: &str = "notifications.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OngoingKind {
    Recording,
    Timer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OngoingBadge {
    pub kind: OngoingKind,
    pub label: String,
    pub detail: String,
    pub paused: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    pub id: String,
    pub source: String,
    pub title: String,
    pub body: String,
    pub priority: Priority,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub action_panel: Option<String>,
    pub sensitive: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityHubState {
    pub active_banner: Option<ActivityItem>,
    pub ongoing: Vec<OngoingBadge>,
    pub notifications: Vec<ActivityItem>,
    pub unread_count: usize,
}

#[derive(Clone, Debug)]
pub struct PrivacyPolicy {
    pub do_not_disturb: bool,
    pub hide_sensitive: bool,
    pub keep_history: bool,
    pub muted_sources: Vec<String>,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            do_not_disturb: false,
            hide_sensitive: false,
            keep_history: true,
            muted_sources: Vec::new(),
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct ActivityCore {
    active_banner: Option<ActivityItem>,
    ongoing: Vec<OngoingBadge>,
    notifications: Vec<ActivityItem>,
    policy: PrivacyPolicy,
    next_seq: u64,
}

impl ActivityCore {
    pub fn new(history: Vec<ActivityItem>, policy: PrivacyPolicy) -> Self {
        let mut notifications = history;
        notifications.truncate(MAX_HISTORY);
        Self {
            active_banner: None,
            ongoing: Vec::new(),
            notifications,
            policy,
            next_seq: 1,
        }
    }

    pub fn set_policy(&mut self, policy: PrivacyPolicy) {
        if !policy.keep_history {
            self.notifications.clear();
        }
        self.policy = policy;
    }

    pub fn push(
        &mut self,
        source: String,
        title: String,
        body: String,
        priority: Priority,
        ttl_ms: u64,
        action_panel: Option<String>,
        sensitive: bool,
        record_in_history: bool,
        at_ms: u64,
    ) -> Option<ActivityItem> {
        let source_lower = source.trim().to_lowercase();
        let is_muted = self
            .policy
            .muted_sources
            .iter()
            .any(|s| s.trim().eq_ignore_ascii_case(&source_lower));
        if is_muted && priority < Priority::Critical {
            return None;
        }

        let seq = self.next_seq;
        self.next_seq += 1;
        let stored_body = if self.policy.hide_sensitive && sensitive {
            "Content hidden".to_string()
        } else {
            body
        };
        let item = ActivityItem {
            id: format!("act-{}-{}", at_ms, seq),
            source,
            title,
            body: stored_body,
            priority,
            created_at_ms: at_ms,
            expires_at_ms: if ttl_ms > 0 {
                at_ms.saturating_add(ttl_ms)
            } else {
                0
            },
            action_panel,
            sensitive,
        };

        if record_in_history && self.policy.keep_history {
            self.notifications.insert(0, item.clone());
            self.notifications.truncate(MAX_HISTORY);
        }

        let dnd_blocks = self.policy.do_not_disturb && priority < Priority::Critical;
        if !dnd_blocks {
            let expired = self
                .active_banner
                .as_ref()
                .is_some_and(|b| b.expires_at_ms > 0 && b.expires_at_ms <= at_ms);
            if expired {
                self.active_banner = None;
            }
            let can_replace = self
                .active_banner
                .as_ref()
                .is_none_or(|b| priority >= b.priority);
            if can_replace {
                self.active_banner = Some(item.clone());
            }
        }

        Some(item)
    }

    pub fn expire(&mut self, at_ms: u64) -> bool {
        if let Some(banner) = &self.active_banner {
            if banner.expires_at_ms > 0 && banner.expires_at_ms <= at_ms {
                self.active_banner = None;
                return true;
            }
        }
        false
    }

    pub fn dismiss_banner(&mut self, id: &str) -> bool {
        if self.active_banner.as_ref().is_some_and(|b| b.id == id) {
            self.active_banner = None;
            return true;
        }
        false
    }

    pub fn dismiss_notification(&mut self, id: &str) -> bool {
        let before = self.notifications.len();
        self.notifications.retain(|n| n.id != id);
        if self.active_banner.as_ref().is_some_and(|b| b.id == id) {
            self.active_banner = None;
        }
        self.notifications.len() != before
    }

    pub fn clear_notifications(&mut self) {
        self.notifications.clear();
    }

    pub fn set_ongoing(&mut self, kind: OngoingKind, label: String, detail: String, paused: bool) {
        if let Some(existing) = self.ongoing.iter_mut().find(|o| o.kind == kind) {
            existing.label = label;
            existing.detail = detail;
            existing.paused = paused;
        } else {
            self.ongoing.push(OngoingBadge {
                kind,
                label,
                detail,
                paused,
            });
        }
    }

    pub fn clear_ongoing(&mut self, kind: OngoingKind) -> bool {
        let before = self.ongoing.len();
        self.ongoing.retain(|o| o.kind != kind);
        self.ongoing.len() != before
    }

    pub fn snapshot(&self, at_ms: u64) -> ActivityHubState {
        let mask = |mut item: ActivityItem| {
            if self.policy.hide_sensitive && item.sensitive {
                item.body = "Content hidden".to_string();
            }
            item
        };
        let active_banner = self
            .active_banner
            .as_ref()
            .filter(|b| b.expires_at_ms == 0 || b.expires_at_ms > at_ms)
            .cloned()
            .map(mask);
        let notifications: Vec<ActivityItem> =
            self.notifications.iter().cloned().map(mask).collect();
        let unread_count = notifications.len();
        ActivityHubState {
            active_banner,
            ongoing: self.ongoing.clone(),
            notifications,
            unread_count,
        }
    }

    pub fn next_expiry_ms(&self) -> Option<u64> {
        self.active_banner
            .as_ref()
            .and_then(|b| (b.expires_at_ms > 0).then_some(b.expires_at_ms))
    }

    pub fn raw_notifications(&self) -> &[ActivityItem] {
        &self.notifications
    }
}

#[derive(Clone)]
pub struct ActivityManager {
    core: Arc<Mutex<ActivityCore>>,
    app: AppHandle,
    config_dir: Option<PathBuf>,
    wake: Arc<Notify>,
}

impl ActivityManager {
    pub fn start(app: AppHandle, config_dir: Option<PathBuf>, policy: PrivacyPolicy) -> Self {
        let loaded = config_dir
            .as_ref()
            .filter(|_| policy.keep_history)
            .and_then(|dir| std::fs::read(dir.join(HISTORY_FILE)).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<ActivityItem>>(&bytes).ok())
            .unwrap_or_default();
        let core = Arc::new(Mutex::new(ActivityCore::new(loaded, policy)));
        let wake = Arc::new(Notify::new());
        let manager = Self {
            core: core.clone(),
            app: app.clone(),
            config_dir,
            wake: wake.clone(),
        };

        let loop_mgr = manager.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let next_expiry = loop_mgr.core.lock().unwrap().next_expiry_ms();
                match next_expiry {
                    Some(exp) => {
                        let now = now_ms();
                        if exp <= now {
                            if loop_mgr.core.lock().unwrap().expire(now) {
                                loop_mgr.emit_state();
                            }
                            continue;
                        }
                        let wait = std::time::Duration::from_millis(exp - now);
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {
                                if loop_mgr.core.lock().unwrap().expire(now_ms()) {
                                    loop_mgr.emit_state();
                                }
                            }
                            _ = loop_mgr.wake.notified() => {}
                        }
                    }
                    None => {
                        loop_mgr.wake.notified().await;
                    }
                }
            }
        });

        manager
    }

    pub fn update_policy(&self, policy: PrivacyPolicy) {
        let should_clear_disk = !policy.keep_history;
        {
            let mut guard = self.core.lock().unwrap();
            guard.set_policy(policy);
        }
        if should_clear_disk {
            self.persist_history(&[]);
        }
        self.emit_state();
    }

    pub fn push_activity(
        &self,
        source: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
        priority: Priority,
        ttl_ms: u64,
        action_panel: Option<&str>,
        sensitive: bool,
        record_in_history: bool,
    ) {
        let snapshot_to_save = {
            let mut guard = self.core.lock().unwrap();
            let pushed = guard.push(
                source.into(),
                title.into(),
                body.into(),
                priority,
                ttl_ms,
                action_panel.map(str::to_string),
                sensitive,
                record_in_history,
                now_ms(),
            );
            if pushed.is_none() {
                return;
            }
            if record_in_history && guard.policy.keep_history {
                Some(guard.raw_notifications().to_vec())
            } else {
                None
            }
        };
        if let Some(items) = snapshot_to_save {
            self.persist_history(&items);
        }
        self.wake.notify_one();
        self.emit_state();
    }

    pub fn set_ongoing(
        &self,
        kind: OngoingKind,
        label: impl Into<String>,
        detail: impl Into<String>,
        paused: bool,
    ) {
        self.core
            .lock()
            .unwrap()
            .set_ongoing(kind, label.into(), detail.into(), paused);
        self.emit_state();
    }

    pub fn clear_ongoing(&self, kind: OngoingKind) {
        if self.core.lock().unwrap().clear_ongoing(kind) {
            self.emit_state();
        }
    }

    pub fn snapshot(&self) -> ActivityHubState {
        self.core.lock().unwrap().snapshot(now_ms())
    }

    pub fn dismiss_banner(&self, id: &str) {
        if self.core.lock().unwrap().dismiss_banner(id) {
            self.wake.notify_one();
            self.emit_state();
        }
    }

    pub fn dismiss_notification(&self, id: &str) {
        let items = {
            let mut guard = self.core.lock().unwrap();
            if !guard.dismiss_notification(id) {
                return;
            }
            guard.raw_notifications().to_vec()
        };
        self.persist_history(&items);
        self.emit_state();
    }

    pub fn clear_notifications(&self) {
        {
            self.core.lock().unwrap().clear_notifications();
        }
        self.persist_history(&[]);
        self.emit_state();
    }

    fn emit_state(&self) {
        let state = self.snapshot();
        let _ = self.app.emit("activity-state", state);
    }

    fn persist_history(&self, items: &[ActivityItem]) {
        if let Some(dir) = &self.config_dir {
            let _ = crate::preferences::write_json_atomic(dir, HISTORY_FILE, items);
        }
    }
}

#[tauri::command]
pub fn get_activity_hub(state: tauri::State<'_, ActivityManager>) -> ActivityHubState {
    state.snapshot()
}

#[tauri::command]
pub fn dismiss_activity(state: tauri::State<'_, ActivityManager>, id: String) {
    state.dismiss_banner(&id);
}

#[tauri::command]
pub fn dismiss_notification(state: tauri::State<'_, ActivityManager>, id: String) {
    state.dismiss_notification(&id);
}

#[tauri::command]
pub fn clear_notifications(state: tauri::State<'_, ActivityManager>) {
    state.clear_notifications();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_preempts_low_and_low_does_not_clobber_high() {
        let mut core = ActivityCore::new(vec![], PrivacyPolicy::default());
        core.push(
            "Music".into(),
            "Song A".into(),
            "Artist".into(),
            Priority::Low,
            4000,
            Some("music".into()),
            false,
            false,
            1000,
        );
        assert_eq!(core.snapshot(1000).active_banner.unwrap().title, "Song A");

        core.push(
            "Capture".into(),
            "Recording".into(),
            "".into(),
            Priority::Critical,
            4000,
            Some("capture".into()),
            false,
            false,
            1100,
        );
        assert_eq!(
            core.snapshot(1100).active_banner.unwrap().title,
            "Recording"
        );

        // Lower priority cannot displace active Critical banner before expiry
        core.push(
            "Music".into(),
            "Song B".into(),
            "".into(),
            Priority::High,
            4000,
            None,
            false,
            false,
            1200,
        );
        assert_eq!(
            core.snapshot(1200).active_banner.unwrap().title,
            "Recording"
        );

        // Expiry clears active banner
        assert!(core.expire(5100));
        assert!(core.snapshot(5100).active_banner.is_none());
    }

    #[test]
    fn dnd_suppresses_non_critical_banners_and_sensitive_is_redacted() {
        let policy = PrivacyPolicy {
            do_not_disturb: true,
            hide_sensitive: true,
            keep_history: true,
            muted_sources: vec!["NoisyApp".into()],
        };
        let mut core = ActivityCore::new(vec![], policy);

        // Muted non-critical source is dropped
        assert!(core
            .push(
                "noisyapp".into(),
                "Spam".into(),
                "Hidden".into(),
                Priority::Normal,
                4000,
                None,
                false,
                true,
                1000,
            )
            .is_none());

        // Normal event is recorded in history (redacted) but suppressed from popup banner by DND
        core.push(
            "Mail".into(),
            "New message".into(),
            "Secret OTP 1234".into(),
            Priority::Normal,
            4000,
            None,
            true,
            true,
            1050,
        );
        let snap = core.snapshot(1050);
        assert!(snap.active_banner.is_none());
        assert_eq!(snap.notifications.len(), 1);
        assert_eq!(snap.notifications[0].body, "Content hidden");

        // Critical still shows banner even under DND
        core.push(
            "Capture".into(),
            "Recording".into(),
            "".into(),
            Priority::Critical,
            4000,
            None,
            false,
            false,
            1100,
        );
        assert_eq!(
            core.snapshot(1100).active_banner.unwrap().title,
            "Recording"
        );
    }
}
