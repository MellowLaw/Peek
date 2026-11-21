import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDown, ArrowUp, Copy, RefreshCw } from "lucide-react";
import { DEFAULT_PANELS, usePreferences } from "../../stores/preferences";
import { useUpdater } from "../../stores/updater";
import type { Accent, AutoCloseMode, PanelId, SizePreset } from "../../types/peek";

type LocalApiStatus = {
  enabled: boolean;
  listening: boolean;
  port: number;
  error: string;
};

const PANEL_LABELS: Record<PanelId, string> = {
  music: "Music",
  ai: "AI",
  files: "Files",
  capture: "Capture",
  controls: "Controls",
  clipboard: "Clipboard",
  timer: "Timer",
  notes: "Notes",
  launcher: "Launcher",
  notifications: "Alerts",
  settings: "Settings",
};

export function SettingSwitch({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <button
      type="button"
      className="setting-row switch-row"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span>{label}</span>
      <span className="switch-track" aria-hidden="true">
        <span className="switch-thumb" />
      </span>
    </button>
  );
}

export function SettingsPanel() {
  const { preferences, ready, saving, error, monitors, shortcuts, load, save } = usePreferences();
  const updater = useUpdater();
  const [tab, setTab] = useState<"island" | "shortcuts" | "privacy">("island");
  const [toggleKey, setToggleKey] = useState(preferences.toggleShortcut);
  const [capKey, setCapKey] = useState(preferences.captureShortcut);
  const [apiPortText, setApiPortText] = useState(String(preferences.localApiPort));
  const [apiStatus, setApiStatus] = useState<LocalApiStatus | null>(null);
  const [notice, setNotice] = useState("");

  useEffect(() => {
    setToggleKey(preferences.toggleShortcut);
    setCapKey(preferences.captureShortcut);
    setApiPortText(String(preferences.localApiPort));
  }, [preferences.toggleShortcut, preferences.captureShortcut, preferences.localApiPort]);

  useEffect(() => {
    void invoke<LocalApiStatus>("get_local_api_status")
      .then(setApiStatus)
      .catch(() => {});
  }, [preferences.localApiEnabled, preferences.localApiPort, preferences.localApiToken]);

  const flash = (msg: string) => {
    setNotice(msg);
    setTimeout(() => setNotice(""), 2200);
  };

  // Build ordered list containing all panels (enabled first in saved order, then disabled)
  const orderedPanels: PanelId[] = [
    ...preferences.enabledPanels,
    ...DEFAULT_PANELS.filter((id) => !preferences.enabledPanels.includes(id)),
  ];

  function togglePanelEnabled(id: PanelId) {
    if (id === "settings") return;
    const isEnabled = preferences.enabledPanels.includes(id);
    const next = isEnabled
      ? preferences.enabledPanels.filter((p) => p !== id)
      : orderedPanels.filter((p) => p === id || preferences.enabledPanels.includes(p));
    void save({ enabledPanels: next });
  }

  function movePanel(id: PanelId, dir: -1 | 1) {
    const enabled = [...preferences.enabledPanels];
    const idx = enabled.indexOf(id);
    const swap = idx + dir;
    if (idx === -1 || swap < 0 || swap >= enabled.length) return;
    [enabled[idx], enabled[swap]] = [enabled[swap], enabled[idx]];
    void save({ enabledPanels: enabled });
  }

  async function applyShortcuts() {
    const ok = await save({
      toggleShortcut: toggleKey,
      captureShortcut: capKey,
    });
    if (ok) flash("Shortcuts updated");
  }

  async function applyApiPort() {
    const parsed = Number(apiPortText);
    const port = Number.isFinite(parsed) ? Math.min(65535, Math.max(1024, Math.round(parsed))) : 48123;
    setApiPortText(String(port));
    const ok = await save({ localApiPort: port });
    if (ok) flash("API port updated");
  }

  return (
    <section aria-label="Settings" className="settings">
      <div className="subtabs" role="tablist">
        {(
          [
            ["island", "Notch"],
            ["shortcuts", "Shortcuts"],
            ["privacy", "Privacy & API"],
          ] as const
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className={"subtab " + (tab === id ? "active" : "")}
            onClick={() => setTab(id)}
          >
            {label}
          </button>
        ))}
      </div>

      {tab === "island" && (
        <div className="settings-stack">
          <div className="setting-row">
            <span>Top notch visibility</span>
            <div className="drag-mode-pill" role="group" aria-label="Top notch visibility">
              <button
                type="button"
                className={"drag-mode-opt" + (preferences.alwaysVisible ? " active" : "")}
                disabled={!ready || saving}
                onClick={() => void save({ alwaysVisible: true })}
              >
                Always visible
              </button>
              <button
                type="button"
                className={"drag-mode-opt" + (!preferences.alwaysVisible ? " active" : "")}
                disabled={!ready || saving}
                onClick={() => void save({ alwaysVisible: false })}
              >
                Show on hover
              </button>
            </div>
          </div>

          <div className="setting-row">
            <span>Auto-close</span>
            <div className="drag-mode-pill" role="group" aria-label="Auto-close mode">
              {(
                [
                  ["off", "Off"],
                  ["clickOutside", "Click outside"],
                  ["leaveAndClick", "Leave & click"],
                ] as [AutoCloseMode, string][]
              ).map(([modeVal, label]) => (
                <button
                  key={modeVal}
                  type="button"
                  className={"drag-mode-opt" + (preferences.autoCloseMode === modeVal ? " active" : "")}
                  disabled={!ready || saving}
                  onClick={() => void save({ autoCloseMode: modeVal })}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>

          <SettingSwitch
            label="Hover toolbar"
            checked={preferences.hoverPreview}
            disabled={!ready || saving}
            onChange={(checked) => void save({ hoverPreview: checked })}
          />
          {preferences.hoverPreview && (
            <div className="setting-row">
              <span>Hover delay ({preferences.hoverDelayMs} ms)</span>
              <input
                type="range"
                aria-label="Hover delay milliseconds"
                min={0}
                max={800}
                step={20}
                value={preferences.hoverDelayMs}
                disabled={!ready || saving}
                onChange={(e) => void save({ hoverDelayMs: Number(e.target.value) })}
              />
            </div>
          )}
          <SettingSwitch
            label="Reduce motion"
            checked={preferences.reducedMotion}
            disabled={!ready || saving}
            onChange={(checked) => void save({ reducedMotion: checked })}
          />
          <SettingSwitch
            label="Sync lyrics with Lyrical"
            checked={preferences.lyricsEnabled}
            disabled={!ready || saving}
            onChange={(checked) => void save({ lyricsEnabled: checked })}
          />
          <SettingSwitch
            label="Launch Peek at Windows sign-in"
            checked={preferences.autostart}
            disabled={!ready || saving}
            onChange={(checked) => void save({ autostart: checked })}
          />

          <label className="setting-row">
            <span>Accent</span>
            <select
              value={preferences.accent}
              disabled={!ready || saving}
              onChange={(event) => void save({ accent: event.target.value as Accent })}
            >
              <option value="neutral">Neutral</option>
              <option value="blue">Blue</option>
              <option value="green">Green</option>
              <option value="amber">Amber</option>
              <option value="rose">Rose</option>
            </select>
          </label>

          <label className="setting-row">
            <span>Panel size</span>
            <select
              value={preferences.sizePreset}
              disabled={!ready || saving}
              onChange={(event) => void save({ sizePreset: event.target.value as SizePreset })}
            >
              <option value="compact">Compact</option>
              <option value="standard">Standard</option>
              <option value="large">Large</option>
            </select>
          </label>

          {monitors.length > 1 && (
            <label className="setting-row">
              <span>Display</span>
              <select
                value={preferences.monitorName}
                disabled={!ready || saving}
                onChange={(event) => void save({ monitorName: event.target.value })}
              >
                <option value="">Primary display</option>
                {monitors.map((m) => (
                  <option key={m.deviceName || m.name} value={m.deviceName}>
                    {m.name}
                  </option>
                ))}
              </select>
            </label>
          )}

          <div className="ui-section settings-panels-box">
            <span className="status-note">Visible tools & order</span>
            {orderedPanels.map((id) => {
              const enabled = preferences.enabledPanels.includes(id);
              const idx = preferences.enabledPanels.indexOf(id);
              return (
                <div key={id} className="panel-order-row">
                  <label className="panel-check-label">
                    <input
                      type="checkbox"
                      checked={enabled}
                      disabled={id === "settings" || !ready || saving}
                      onChange={() => togglePanelEnabled(id)}
                    />
                    <span>{PANEL_LABELS[id]}</span>
                  </label>
                  {enabled && (
                    <div className="panel-order-btns">
                      <button
                        type="button"
                        className="btn sm"
                        aria-label={`Move ${PANEL_LABELS[id]} left`}
                        disabled={idx <= 0 || saving}
                        onClick={() => movePanel(id, -1)}
                      >
                        <ArrowUp size={11} />
                      </button>
                      <button
                        type="button"
                        className="btn sm"
                        aria-label={`Move ${PANEL_LABELS[id]} right`}
                        disabled={idx === preferences.enabledPanels.length - 1 || saving}
                        onClick={() => movePanel(id, 1)}
                      >
                        <ArrowDown size={11} />
                      </button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>

          <div className="ui-section settings-stack">
            <div className="setting-row">
              <span>Updates (Peek v{updater.snapshot.currentVersion})</span>
              <span className="status-note">
                {updater.snapshot.status === "checking"
                  ? "Checking…"
                  : updater.snapshot.status === "downloading"
                    ? `Downloading ${updater.snapshot.progressPercent ?? 0}%`
                    : updater.snapshot.status === "installing"
                      ? "Installing…"
                      : updater.snapshot.status === "upToDate"
                        ? "Up to date"
                        : updater.snapshot.status === "available" && updater.snapshot.available
                          ? `v${updater.snapshot.available.version} available`
                          : updater.snapshot.status === "error" && updater.snapshot.error
                            ? updater.snapshot.error
                            : "Automatic checks on launch"}
              </span>
            </div>
            {updater.snapshot.status === "downloading" && (
              <div className="update-progress-track">
                <div
                  className="update-progress-fill"
                  style={{ width: `${updater.snapshot.progressPercent ?? 0}%` }}
                />
              </div>
            )}
            <div className="tools-row">
              <button
                type="button"
                className="btn sm"
                disabled={
                  updater.snapshot.status === "checking" ||
                  updater.snapshot.status === "downloading" ||
                  updater.snapshot.status === "installing"
                }
                onClick={() => void updater.check(true)}
              >
                <RefreshCw size={11} /> Check for updates
              </button>
              {updater.snapshot.available &&
                updater.snapshot.status !== "downloading" &&
                updater.snapshot.status !== "installing" && (
                  <button
                    type="button"
                    className="btn primary sm"
                    onClick={() => void updater.install()}
                  >
                    Update to v{updater.snapshot.available.version}
                  </button>
                )}
            </div>
          </div>
        </div>
      )}

      {tab === "shortcuts" && (
        <div className="settings-stack">
          <div className="ui-section shortcut-edit-card">
            <label className="shortcut-field">
              <span>Toggle Peek</span>
              <input
                type="text"
                value={toggleKey}
                onChange={(e) => setToggleKey(e.target.value)}
              />
            </label>
            <div className="status-note">
              {shortcuts.toggle
                ? "Registered"
                : shortcuts.toggleError || "Shortcut unavailable (tray remains active)"}
            </div>
          </div>

          <div className="ui-section shortcut-edit-card">
            <label className="shortcut-field">
              <span>Region screenshot</span>
              <input
                type="text"
                value={capKey}
                onChange={(e) => setCapKey(e.target.value)}
              />
            </label>
            <div className="status-note">
              {shortcuts.capture
                ? "Registered"
                : shortcuts.captureError || "Unassigned or unavailable"}
            </div>
          </div>

          <button
            type="button"
            className="btn primary"
            disabled={!ready || saving}
            onClick={() => void applyShortcuts()}
          >
            Save shortcuts
          </button>
        </div>
      )}

      {tab === "privacy" && (
        <div className="settings-stack">
          <SettingSwitch
            label="Clipboard history (opt-in)"
            checked={preferences.clipboardHistoryEnabled}
            disabled={!ready || saving}
            onChange={(checked) => void save({ clipboardHistoryEnabled: checked })}
          />
          <SettingSwitch
            label="Windows toast notifications (opt-in)"
            checked={preferences.notificationsEnabled}
            disabled={!ready || saving}
            onChange={(checked) => void save({ notificationsEnabled: checked })}
          />
          <SettingSwitch
            label="Do Not Disturb"
            checked={preferences.doNotDisturb}
            disabled={!ready || saving}
            onChange={(checked) => void save({ doNotDisturb: checked })}
          />
          <SettingSwitch
            label="Hide sensitive notification content"
            checked={preferences.hideSensitiveNotifications}
            disabled={!ready || saving}
            onChange={(checked) => void save({ hideSensitiveNotifications: checked })}
          />

          <div className="ui-section settings-stack">
            <SettingSwitch
              label="Local Event API (127.0.0.1 only)"
              checked={preferences.localApiEnabled}
              disabled={!ready || saving}
              onChange={(checked) => void save({ localApiEnabled: checked })}
            />
            <div className="setting-row">
              <span>Port</span>
              <div style={{ display: "flex", gap: 6 }}>
                <input
                  type="text"
                  inputMode="numeric"
                  aria-label="Local API port"
                  value={apiPortText}
                  disabled={!ready || saving}
                  onChange={(e) => setApiPortText(e.target.value.replace(/\D+/g, ""))}
                  style={{ width: 84, textAlign: "right" }}
                />
                <button
                  type="button"
                  className="btn sm"
                  disabled={!ready || saving}
                  onClick={() => void applyApiPort()}
                >
                  Apply
                </button>
              </div>
            </div>
            {apiStatus && (
              <div className="status-note">
                {!apiStatus.enabled
                  ? "Disabled (no local port bound)"
                  : apiStatus.error
                    ? apiStatus.error
                    : apiStatus.listening
                      ? `Listening on http://127.0.0.1:${apiStatus.port}/v1/events`
                      : "Starting…"}
              </div>
            )}
            {preferences.localApiEnabled && preferences.localApiToken && (
              <div className="tools-row">
                <code className="qr-url" style={{ flex: 1 }}>
                  Bearer {preferences.localApiToken}
                </code>
                <button
                  type="button"
                  className="btn sm"
                  aria-label="Copy API token"
                  onClick={() =>
                    void navigator.clipboard
                      .writeText(preferences.localApiToken)
                      .then(() => flash("Token copied"))
                  }
                >
                  <Copy size={11} />
                </button>
                <button
                  type="button"
                  className="btn sm"
                  aria-label="Rotate API token"
                  title="Generate new Bearer token"
                  onClick={() => void save({ localApiToken: "" })}
                >
                  <RefreshCw size={11} />
                </button>
              </div>
            )}
          </div>
        </div>
      )}

      {saving && <div role="status">Saving…</div>}
      {notice && (
        <div className="status-note" role="status">
          {notice}
        </div>
      )}
      {error && (
        <div role="alert" className="error">
          Settings could not be {ready ? "saved" : "loaded"}.
          {!ready && <button onClick={() => void load()}>Retry</button>}
        </div>
      )}
    </section>
  );
}
