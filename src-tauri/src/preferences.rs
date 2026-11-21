use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use tauri::Manager;

pub const ALL_PANELS: [&str; 11] = [
    "music",
    "ai",
    "files",
    "capture",
    "controls",
    "clipboard",
    "timer",
    "notes",
    "launcher",
    "notifications",
    "settings",
];

pub const AI_PROVIDERS: [&str; 3] = ["claude", "gemini", "codex"];

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Accent {
    #[default]
    Neutral,
    Blue,
    Green,
    Amber,
    Rose,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SizePreset {
    Compact,
    #[default]
    Standard,
    Large,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoCloseMode {
    Off,
    ClickOutside,
    #[default]
    LeaveAndClick,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderBudgets {
    pub claude_5h_tokens: u64,
    pub claude_weekly_tokens: u64,
    pub gemini_5h_tokens: u64,
    pub gemini_weekly_tokens: u64,
    pub codex_5h_tokens: u64,
    pub codex_weekly_tokens: u64,
}

impl Default for ProviderBudgets {
    fn default() -> Self {
        Self {
            claude_5h_tokens: 25_000_000,
            claude_weekly_tokens: 150_000_000,
            gemini_5h_tokens: 100_000_000,
            gemini_weekly_tokens: 500_000_000,
            codex_5h_tokens: 25_000_000,
            codex_weekly_tokens: 150_000_000,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub hover_preview: bool,
    pub hover_delay_ms: u32,
    pub always_visible: bool,
    pub auto_close_mode: AutoCloseMode,
    pub reduced_motion: bool,
    pub accent: Accent,
    pub size_preset: SizePreset,
    pub monitor_name: String,
    pub enabled_panels: Vec<String>,
    pub toggle_shortcut: String,
    pub capture_shortcut: String,
    pub lyrics_enabled: bool,
    pub autostart: bool,
    pub clipboard_history_enabled: bool,
    pub notifications_enabled: bool,
    pub do_not_disturb: bool,
    pub hide_sensitive_notifications: bool,
    pub notification_history_enabled: bool,
    pub muted_sources: Vec<String>,
    pub local_api_enabled: bool,
    pub local_api_port: u16,
    pub local_api_token: String,
    pub pinned_repos: Vec<String>,
    pub pinned_ai_providers: Vec<String>,
    pub ai_budgets: ProviderBudgets,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            hover_preview: true,
            hover_delay_ms: 80,
            always_visible: true,
            auto_close_mode: AutoCloseMode::LeaveAndClick,
            reduced_motion: false,
            accent: Accent::Neutral,
            size_preset: SizePreset::Standard,
            monitor_name: String::new(),
            enabled_panels: ALL_PANELS.iter().map(|s| (*s).to_string()).collect(),
            toggle_shortcut: "Ctrl+Alt+Space".into(),
            capture_shortcut: "Ctrl+Alt+S".into(),
            lyrics_enabled: true,
            autostart: false,
            clipboard_history_enabled: false,
            notifications_enabled: false,
            do_not_disturb: false,
            hide_sensitive_notifications: false,
            notification_history_enabled: true,
            muted_sources: Vec::new(),
            local_api_enabled: false,
            local_api_port: 48123,
            local_api_token: String::new(),
            pinned_repos: Vec::new(),
            pinned_ai_providers: Vec::new(),
            ai_budgets: ProviderBudgets::default(),
        }
    }
}

impl Preferences {
    pub fn normalize(mut self) -> Self {
        self.hover_delay_ms = self.hover_delay_ms.clamp(0, 1200);
        if self.local_api_port < 1024 {
            self.local_api_port = 48123;
        }
        let clamp_budget = |v: u64| v.clamp(100_000, 10_000_000_000);
        self.ai_budgets.claude_5h_tokens = clamp_budget(self.ai_budgets.claude_5h_tokens);
        self.ai_budgets.claude_weekly_tokens = clamp_budget(self.ai_budgets.claude_weekly_tokens);
        self.ai_budgets.gemini_5h_tokens = clamp_budget(self.ai_budgets.gemini_5h_tokens);
        self.ai_budgets.gemini_weekly_tokens = clamp_budget(self.ai_budgets.gemini_weekly_tokens);
        self.ai_budgets.codex_5h_tokens = clamp_budget(self.ai_budgets.codex_5h_tokens);
        self.ai_budgets.codex_weekly_tokens = clamp_budget(self.ai_budgets.codex_weekly_tokens);
        let mut panels = Vec::new();
        for candidate in self.enabled_panels {
            if candidate == "tools" {
                for tool in ["clipboard", "timer", "notes", "launcher"] {
                    let t = tool.to_string();
                    if !panels.contains(&t) {
                        panels.push(t);
                    }
                }
            } else if ALL_PANELS.contains(&candidate.as_str()) && !panels.contains(&candidate) {
                panels.push(candidate);
            }
        }
        if panels.is_empty() {
            panels = ALL_PANELS.iter().map(|s| (*s).to_string()).collect();
        } else if !panels.iter().any(|p| p == "settings") {
            panels.push("settings".to_string());
        }
        self.enabled_panels = panels;
        let mut pinned_ai = Vec::new();
        for provider in self.pinned_ai_providers {
            if AI_PROVIDERS.contains(&provider.as_str()) && !pinned_ai.contains(&provider) {
                pinned_ai.push(provider);
            }
        }
        self.pinned_ai_providers = pinned_ai;
        self.toggle_shortcut = self.toggle_shortcut.trim().to_string();
        self.capture_shortcut = self.capture_shortcut.trim().to_string();
        if self.toggle_shortcut.is_empty() {
            self.toggle_shortcut = "Ctrl+Alt+Space".into();
        }
        if self.local_api_enabled && self.local_api_token.trim().is_empty() {
            #[cfg(windows)]
            {
                self.local_api_token = crate::windows::generate_random_hex(16);
            }
            #[cfg(not(windows))]
            {
                self.local_api_token = "peek-local-token".into();
            }
        }
        self
    }

    pub fn privacy_policy(&self) -> crate::activity::PrivacyPolicy {
        crate::activity::PrivacyPolicy {
            do_not_disturb: self.do_not_disturb,
            hide_sensitive: self.hide_sensitive_notifications,
            keep_history: self.notification_history_enabled,
            muted_sources: self.muted_sources.clone(),
        }
    }
}

pub struct PreferencesState(pub Mutex<Preferences>);

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStates {
    pub toggle: bool,
    pub capture: bool,
    pub toggle_error: String,
    pub capture_error: String,
}

pub struct ShortcutsState(pub Mutex<ShortcutStates>);

/// Shared atomic JSON writer used by preferences, activity history, file shelf, and notes.
pub fn write_json_atomic<T: Serialize + ?Sized>(
    directory: &Path,
    file_name: &str,
    value: &T,
) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    temporary
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist(directory.join(file_name))
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn load_from_disk(app: &tauri::AppHandle) -> Result<Preferences, String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("preferences.json");
    match std::fs::read(path) {
        Ok(bytes) => {
            let parsed: Preferences =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            Ok(parsed.normalize())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn get_preferences(
    app: tauri::AppHandle,
    state: tauri::State<'_, PreferencesState>,
) -> Result<Preferences, String> {
    let loaded = load_from_disk(&app)?;
    *state.0.lock().unwrap() = loaded.clone();
    Ok(loaded)
}

#[tauri::command]
pub fn shortcut_states(state: tauri::State<'_, ShortcutsState>) -> ShortcutStates {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub async fn save_preferences(
    app: tauri::AppHandle,
    preferences: Preferences,
) -> Result<Preferences, String> {
    let next = preferences.normalize();
    let previous = app.state::<PreferencesState>().0.lock().unwrap().clone();

    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    write_json_atomic(&directory, "preferences.json", &next)?;

    if next.autostart != previous.autostart {
        #[cfg(windows)]
        crate::windows::set_autostart(next.autostart)?;
    }

    *app.state::<PreferencesState>().0.lock().unwrap() = next.clone();

    if next.toggle_shortcut != previous.toggle_shortcut
        || next.capture_shortcut != previous.capture_shortcut
    {
        let states = crate::register_shortcuts(&app, &next);
        *app.state::<ShortcutsState>().0.lock().unwrap() = states;
    }

    app.state::<crate::activity::ActivityManager>()
        .update_policy(next.privacy_policy());

    if next.notifications_enabled != previous.notifications_enabled {
        crate::notifications::set_enabled(&app, next.notifications_enabled);
    }

    if next.clipboard_history_enabled != previous.clipboard_history_enabled {
        app.state::<std::sync::Arc<crate::tools::ToolsState>>()
            .set_clipboard_enabled(next.clipboard_history_enabled);
    }

    if next.always_visible != previous.always_visible {
        crate::island::sync_visibility(&app, next.always_visible);
    }

    if next.local_api_enabled != previous.local_api_enabled
        || next.local_api_port != previous.local_api_port
        || next.local_api_token != previous.local_api_token
    {
        crate::local_api::sync_config(
            app.clone(),
            next.local_api_enabled,
            next.local_api_port,
            next.local_api_token.clone(),
        )
        .await;
    }

    if next.monitor_name != previous.monitor_name
        || next.size_preset != previous.size_preset
        || next.always_visible != previous.always_visible
    {
        let _ = crate::island::reanchor(&app).await;
    }

    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_preferences_default_new_fields() {
        let preferences: Preferences = serde_json::from_str(
            r#"{"hoverPreview":false,"dictationShortcut":"Ctrl+Alt+V","enabledPanels":["music","voice","settings"]}"#,
        )
        .unwrap();
        let normalized = preferences.normalize();
        assert!(!normalized.hover_preview);
        assert!(normalized.always_visible);
        assert_eq!(
            serde_json::to_value(normalized.auto_close_mode).unwrap(),
            "leaveAndClick"
        );
        assert!(!normalized.reduced_motion);
        assert!(matches!(normalized.accent, Accent::Neutral));
        assert_eq!(normalized.hover_delay_ms, 80);
        assert_eq!(normalized.toggle_shortcut, "Ctrl+Alt+Space");
        assert_eq!(normalized.enabled_panels, vec!["music", "settings"]);

        let legacy_tools: Preferences =
            serde_json::from_str(r#"{"enabledPanels":["music","tools","settings"]}"#).unwrap();
        assert_eq!(
            legacy_tools.normalize().enabled_panels,
            vec![
                "music",
                "clipboard",
                "timer",
                "notes",
                "launcher",
                "settings"
            ]
        );
        let budgets_json = serde_json::to_value(&normalized.ai_budgets).unwrap();
        assert_eq!(budgets_json["claude5hTokens"], 25_000_000);
        assert_eq!(budgets_json["geminiWeeklyTokens"], 500_000_000);
        assert!(normalized.pinned_ai_providers.is_empty());
    }

    #[test]
    fn preferences_round_trip_and_reject_invalid_values() {
        let value = serde_json::json!({"hoverPreview":false,"reducedMotion":true,"accent":"blue","autoCloseMode":"clickOutside"});
        let preferences: Preferences = serde_json::from_value(value).unwrap();
        assert!(!preferences.hover_preview);
        assert!(preferences.reduced_motion);
        assert!(matches!(preferences.accent, Accent::Blue));
        assert!(matches!(
            preferences.auto_close_mode,
            AutoCloseMode::ClickOutside
        ));
        assert!(serde_json::from_str::<Preferences>(r#"{"accent":"unknown"}"#).is_err());
        assert!(serde_json::from_str::<Preferences>(r#"{"hoverPreview":"yes"}"#).is_err());
        assert!(serde_json::from_str::<Preferences>(r#"{"autoCloseMode":"bad"}"#).is_err());
    }

    #[test]
    fn normalize_clamps_values_and_keeps_settings_panel() {
        let prefs = Preferences {
            hover_delay_ms: 5000,
            local_api_port: 80,
            enabled_panels: vec!["music".into(), "unknown".into(), "music".into()],
            pinned_ai_providers: vec![
                "claude".into(),
                "invalid".into(),
                "claude".into(),
                "codex".into(),
            ],
            ai_budgets: ProviderBudgets {
                claude_5h_tokens: 1,
                codex_weekly_tokens: 99_999_999_999,
                ..ProviderBudgets::default()
            },
            ..Preferences::default()
        }
        .normalize();
        assert_eq!(prefs.hover_delay_ms, 1200);
        assert_eq!(prefs.local_api_port, 48123);
        assert_eq!(prefs.enabled_panels, vec!["music", "settings"]);
        assert_eq!(prefs.pinned_ai_providers, vec!["claude", "codex"]);
        assert_eq!(prefs.ai_budgets.claude_5h_tokens, 100_000);
        assert_eq!(prefs.ai_budgets.codex_weekly_tokens, 10_000_000_000);
    }
}
