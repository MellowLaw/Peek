import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowDown,
  ArrowUp,
  Bell,
  Bot,
  Camera,
  Check,
  Clipboard,
  Copy,
  Files,
  Music2,
  RefreshCw,
  Rocket,
  ScanLine,
  Settings2,
  SlidersHorizontal,
  Sparkles,
  StickyNote,
  Timer,
} from "lucide-react";
import { SmoothSlider } from "../../components/SmoothSlider";
import { DEFAULT_PANELS, usePreferences } from "../../stores/preferences";
import { useUpdater } from "../../stores/updater";
import type { Accent, AutostartStatus, AutoCloseMode, PanelId, SizePreset } from "../../types/peek";

type LocalApiStatus = {
  enabled: boolean;
  listening: boolean;
  port: number;
  error: string;
};

const PANEL_META: Record<PanelId, { label: string; icon: typeof Music2 }> = {
  music: { label: "Music", icon: Music2 },
  ai: { label: "AI", icon: Bot },
  files: { label: "Files", icon: Files },
  capture: { label: "Capture", icon: ScanLine },
  camera: { label: "Cam", icon: Camera },
  controls: { label: "Controls", icon: SlidersHorizontal },
  clipboard: { label: "Clipboard", icon: Clipboard },
  timer: { label: "Timer", icon: Timer },
  notes: { label: "Notes", icon: StickyNote },
  launcher: { label: "Launcher", icon: Rocket },
  notifications: { label: "Alerts", icon: Bell },
  settings: { label: "Settings", icon: Settings2 },
};

const ACCENT_SWATCHES: { id: Accent; label: string; color: string }[] = [
  { id: "neutral", label: "Neutral", color: "#e8e8e8" },
  { id: "blue", label: "Blue", color: "#b8ceff" },
  { id: "green", label: "Green", color: "#b2e2c5" },
  { id: "amber", label: "Amber", color: "#f4cf8a" },
  { id: "rose", label: "Rose", color: "#f2b8c6" },
];

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

function Keycaps({ combo }: { combo: string }) {
  const parts = combo
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length === 0) return <kbd className="set-kbd is-muted">Unset</kbd>;
  return (
    <span className="set-keycaps" aria-hidden="true">
      {parts.map((part, i) => (
        <kbd key={`${part}-${i}`} className="set-kbd">
          {part}
        </kbd>
      ))}
    </span>
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
  const [autostartStatus, setAutostartStatus] = useState<AutostartStatus | null>(null);
  const [draftHoverDelay, setDraftHoverDelay] = useState(preferences.hoverDelayMs);
  const draggingDelayRef = useRef(false);
  const [notice, setNotice] = useState("");

  useEffect(() => {
    if (!draggingDelayRef.current) {
      setDraftHoverDelay(preferences.hoverDelayMs);
    }
  }, [preferences.hoverDelayMs]);

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

  const refreshAutostart = useCallback(() => {
    void invoke<AutostartStatus>("get_autostart_status")
      .then(setAutostartStatus)
      .catch(() => {});
  }, []);

  useEffect(() => {
    refreshAutostart();
  }, [preferences.autostart, ready, refreshAutostart]);

  const flash = (msg: string) => {
    setNotice(msg);
    setTimeout(() => setNotice(""), 2200);
  };

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
    const port = Number.isFinite(parsed)
      ? Math.min(65535, Math.max(1024, Math.round(parsed)))
      : 48123;
    setApiPortText(String(port));
    const ok = await save({ localApiPort: port });
    if (ok) flash("API port updated");
  }

  const autostartBadge = (() => {
    if (!preferences.autostart) {
      return { text: "Off", cls: "is-muted" };
    }
    if (!autostartStatus) {
      return { text: "Checking", cls: "is-muted" };
    }
    if (autostartStatus.disabledByWindows) {
      return { text: "Disabled in Task Mgr", cls: "is-warn" };
    }
    if (autostartStatus.registered && autostartStatus.pathMatches && autostartStatus.targetExists) {
      return { text: "Active", cls: "is-ok" };
    }
    return { text: "Path mismatch", cls: "is-warn" };
  })();

  const updSnap = updater.snapshot;
  const updBusy =
    updSnap.status === "checking" ||
    updSnap.status === "downloading" ||
    updSnap.status === "installing";

  return (
    <section aria-label="Settings" className="settings-bento">
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
          <div className="settings-bento-row">
            <div className="bento-tile set-card">
              <div className="set-card-head">
                <span>Behavior</span>
              </div>
              <div className="set-field-col">
                <span className="set-sub-label">Notch visibility</span>
                <div className="drag-mode-pill is-full" role="group" aria-label="Top notch visibility">
                  <button
                    type="button"
                    className={"drag-mode-opt" + (preferences.alwaysVisible ? " active" : "")}
                    disabled={!ready || saving}
                    onClick={() => void save({ alwaysVisible: true })}
                  >
                    Always
                  </button>
                  <button
                    type="button"
                    className={"drag-mode-opt" + (!preferences.alwaysVisible ? " active" : "")}
                    disabled={!ready || saving}
                    onClick={() => void save({ alwaysVisible: false })}
                  >
                    On hover
                  </button>
                </div>
              </div>

              <div className="set-field-col">
                <span className="set-sub-label">Auto-close</span>
                <div className="drag-mode-pill is-full" role="group" aria-label="Auto-close mode">
                  {(
                    [
                      ["off", "Off"],
                      ["clickOutside", "Click"],
                      ["leaveAndClick", "Leave"],
                    ] as [AutoCloseMode, string][]
                  ).map(([modeVal, label]) => (
                    <button
                      key={modeVal}
                      type="button"
                      className={
                        "drag-mode-opt" + (preferences.autoCloseMode === modeVal ? " active" : "")
                      }
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
                <div className="set-slider-row">
                  <div className="set-slider-head">
                    <span className="set-sub-label">Delay</span>
                    <span className="set-delay-pill">{draftHoverDelay} ms</span>
                  </div>
                  <SmoothSlider
                    ariaLabel="Hover delay milliseconds"
                    min={0}
                    max={800}
                    step={20}
                    value={draftHoverDelay}
                    disabled={!ready}
                    onDragStart={() => {
                      draggingDelayRef.current = true;
                    }}
                    onDragEnd={() => {
                      draggingDelayRef.current = false;
                    }}
                    onChange={(val) => setDraftHoverDelay(val)}
                    onCommit={(val) => {
                      draggingDelayRef.current = false;
                      if (val !== preferences.hoverDelayMs) {
                        void save({ hoverDelayMs: val });
                      }
                    }}
                  />
                </div>
              )}
            </div>

            <div className="bento-tile set-card">
              <div className="set-card-head">
                <span>Look & Display</span>
              </div>

              <div className="set-field-col">
                <span className="set-sub-label">Accent</span>
                <div className="set-swatches" role="group" aria-label="Accent color">
                  {ACCENT_SWATCHES.map((s) => (
                    <button
                      key={s.id}
                      type="button"
                      className={"set-swatch-btn" + (preferences.accent === s.id ? " active" : "")}
                      style={{ "--swatch": s.color } as React.CSSProperties}
                      title={s.label}
                      aria-label={s.label}
                      aria-pressed={preferences.accent === s.id}
                      disabled={!ready || saving}
                      onClick={() => void save({ accent: s.id })}
                    />
                  ))}
                </div>
              </div>

              <div className="set-field-col">
                <span className="set-sub-label">Panel size</span>
                <div className="drag-mode-pill is-full" role="group" aria-label="Panel size">
                  {(
                    [
                      ["compact", "Compact"],
                      ["standard", "Std"],
                      ["large", "Large"],
                    ] as [SizePreset, string][]
                  ).map(([val, label]) => (
                    <button
                      key={val}
                      type="button"
                      className={"drag-mode-opt" + (preferences.sizePreset === val ? " active" : "")}
                      disabled={!ready || saving}
                      onClick={() => void save({ sizePreset: val })}
                    >
                      {label}
                    </button>
                  ))}
                </div>
              </div>

              <SettingSwitch
                label="Reduce motion"
                checked={preferences.reducedMotion}
                disabled={!ready || saving}
                onChange={(checked) => void save({ reducedMotion: checked })}
              />

              {monitors.length > 1 && (
                <label className="set-field-col">
                  <span className="set-sub-label">Display</span>
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
            </div>
          </div>

          <div className="settings-bento-row">
            <div className="bento-tile set-card">
              <div className="set-card-head">
                <span>Startup & Lyrics</span>
                <span className={"set-pill " + autostartBadge.cls}>{autostartBadge.text}</span>
              </div>
              <SettingSwitch
                label="Launch at sign-in"
                checked={preferences.autostart}
                disabled={!ready || saving}
                onChange={(checked) => void save({ autostart: checked })}
              />
              <SettingSwitch
                label="Sync lyrics (Lyrical)"
                checked={preferences.lyricsEnabled}
                disabled={!ready || saving}
                onChange={(checked) => void save({ lyricsEnabled: checked })}
              />
              {preferences.lyricsEnabled && (
                <SettingSwitch
                  label="Collapsed notch lyrics"
                  checked={preferences.idleLyricsEnabled}
                  disabled={!ready || saving}
                  onChange={(checked) => void save({ idleLyricsEnabled: checked })}
                />
              )}
            </div>

            <div className="bento-tile set-card">
              <div className="set-card-head">
                <span className="set-ver-title">
                  <img src="/favicon.svg" alt="" width={16} height={16} className="set-logo-img" />
                  <span>Peek v{updSnap.currentVersion}</span>
                </span>
                <span
                  className={
                    "set-pill " +
                    (updSnap.available
                      ? "is-ok"
                      : updSnap.status === "error"
                        ? "is-warn"
                        : "is-muted")
                  }
                >
                  {updSnap.status === "checking"
                    ? "Checking…"
                    : updSnap.status === "downloading"
                      ? `${updSnap.progressPercent ?? 0}%`
                      : updSnap.status === "installing"
                        ? "Installing"
                        : updSnap.status === "upToDate"
                          ? "Up to date"
                          : updSnap.available
                            ? `v${updSnap.available.version}`
                            : "Auto"}
                </span>
              </div>
              <span className="set-sub-label">
                {updSnap.status === "error" && updSnap.error
                  ? updSnap.error
                  : updSnap.available
                    ? `Update v${updSnap.available.version} ready to install`
                    : "Checks GitHub Releases on launch"}
              </span>
              {updSnap.status === "downloading" && (
                <div className="update-progress-track">
                  <div
                    className="update-progress-fill"
                    style={{ width: `${updSnap.progressPercent ?? 0}%` }}
                  />
                </div>
              )}
              <div className="set-card-actions">
                <button
                  type="button"
                  className="btn sm"
                  disabled={updBusy}
                  onClick={() => void updater.check(true)}
                >
                  <RefreshCw size={11} />
                  <span>Check</span>
                </button>
                {updSnap.available && updSnap.status !== "downloading" && updSnap.status !== "installing" && (
                  <>
                    <button
                      type="button"
                      className="btn sm"
                      onClick={() => void updater.openPopup()}
                    >
                      <Sparkles size={11} />
                      <span>Notes</span>
                    </button>
                    <button
                      type="button"
                      className="btn primary sm"
                      onClick={() => void updater.install()}
                    >
                      Update
                    </button>
                  </>
                )}
              </div>
            </div>
          </div>

          <div className="bento-tile set-card">
            <div className="set-card-head">
              <span>Visible Tools & Order</span>
              <span className="set-sub-label">
                {preferences.enabledPanels.length} active
              </span>
            </div>
            <div className="set-tools-grid">
              {orderedPanels.map((id) => {
                const enabled = preferences.enabledPanels.includes(id);
                const idx = preferences.enabledPanels.indexOf(id);
                const meta = PANEL_META[id];
                const Icon = meta.icon;
                return (
                  <div key={id} className={"set-tool-chip" + (enabled ? " is-on" : "")}>
                    <button
                      type="button"
                      className="set-tool-toggle"
                      disabled={id === "settings" || !ready || saving}
                      onClick={() => togglePanelEnabled(id)}
                    >
                      <span className={"check-box-btn sm" + (enabled ? " is-checked" : "")}>
                        {enabled && <Check size={9} strokeWidth={2.7} />}
                      </span>
                      <Icon size={12} />
                      <span>{meta.label}</span>
                    </button>
                    {enabled && (
                      <div className="set-tool-order">
                        <button
                          type="button"
                          className="set-order-btn"
                          aria-label={`Move ${meta.label} up`}
                          disabled={idx <= 0 || saving}
                          onClick={() => movePanel(id, -1)}
                        >
                          <ArrowUp size={10} />
                        </button>
                        <button
                          type="button"
                          className="set-order-btn"
                          aria-label={`Move ${meta.label} down`}
                          disabled={idx === preferences.enabledPanels.length - 1 || saving}
                          onClick={() => movePanel(id, 1)}
                        >
                          <ArrowDown size={10} />
                        </button>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      )}

      {tab === "shortcuts" && (
        <div className="settings-stack">
          <div className="settings-bento-row">
            <div className="bento-tile set-card">
              <div className="set-card-head">
                <span>Toggle Peek</span>
                <span className={"set-pill " + (shortcuts.toggle ? "is-ok" : "is-warn")}>
                  {shortcuts.toggle ? "Active" : "Conflict"}
                </span>
              </div>
              <Keycaps combo={toggleKey} />
              <input
                type="text"
                aria-label="Toggle Peek shortcut"
                value={toggleKey}
                onChange={(e) => setToggleKey(e.target.value)}
              />
              {!shortcuts.toggle && shortcuts.toggleError && (
                <div className="set-sub-label">{shortcuts.toggleError}</div>
              )}
            </div>

            <div className="bento-tile set-card">
              <div className="set-card-head">
                <span>Capture Bar (Region)</span>
                <span className={"set-pill " + (shortcuts.capture ? "is-ok" : "is-warn")}>
                  {shortcuts.capture ? "Active" : "Conflict"}
                </span>
              </div>
              <Keycaps combo={capKey} />
              <input
                type="text"
                aria-label="Region screenshot shortcut"
                value={capKey}
                onChange={(e) => setCapKey(e.target.value)}
              />
              {!shortcuts.capture && shortcuts.captureError && (
                <div className="set-sub-label">{shortcuts.captureError}</div>
              )}
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
          <div className="settings-bento-row">
            <div className="bento-tile set-card">
              <SettingSwitch
                label="Clipboard history"
                checked={preferences.clipboardHistoryEnabled}
                disabled={!ready || saving}
                onChange={(checked) => void save({ clipboardHistoryEnabled: checked })}
              />
              <SettingSwitch
                label="Windows toast capture"
                checked={preferences.notificationsEnabled}
                disabled={!ready || saving}
                onChange={(checked) => void save({ notificationsEnabled: checked })}
              />
            </div>

            <div className="bento-tile set-card">
              <SettingSwitch
                label="Do Not Disturb"
                checked={preferences.doNotDisturb}
                disabled={!ready || saving}
                onChange={(checked) => void save({ doNotDisturb: checked })}
              />
              <SettingSwitch
                label="Hide sensitive alerts"
                checked={preferences.hideSensitiveNotifications}
                disabled={!ready || saving}
                onChange={(checked) => void save({ hideSensitiveNotifications: checked })}
              />
            </div>
          </div>

          <div className="bento-tile set-card">
            <div className="set-card-head">
              <span>Local Event API (127.0.0.1)</span>
              <span
                className={
                  "set-pill " +
                  (apiStatus?.listening ? "is-ok" : apiStatus?.error ? "is-warn" : "is-muted")
                }
              >
                {!apiStatus?.enabled
                  ? "Off"
                  : apiStatus.error
                    ? "Error"
                    : apiStatus.listening
                      ? `:${apiStatus.port}`
                      : "Starting"}
              </span>
            </div>
            <SettingSwitch
              label="Enable loopback server"
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
