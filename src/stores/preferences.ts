import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { MonitorInfo, PanelId, Preferences, ShortcutStates } from "../types/peek";

export const DEFAULT_PANELS: PanelId[] = [
  "music",
  "ai",
  "files",
  "capture",
  "camera",
  "controls",
  "clipboard",
  "timer",
  "notes",
  "launcher",
  "notifications",
  "settings",
];

export const DEFAULT_PREFERENCES: Preferences = {
  hoverPreview: true,
  hoverDelayMs: 80,
  alwaysVisible: true,
  autoCloseMode: "leaveAndClick",
  reducedMotion: false,
  accent: "neutral",
  sizePreset: "standard",
  monitorName: "",
  enabledPanels: DEFAULT_PANELS,
  toggleShortcut: "Ctrl+Alt+Space",
  captureShortcut: "Ctrl+Alt+S",
  lyricsEnabled: true,
  idleLyricsEnabled: true,
  autostart: false,
  clipboardHistoryEnabled: false,
  notificationsEnabled: false,
  doNotDisturb: false,
  hideSensitiveNotifications: false,
  notificationHistoryEnabled: true,
  mutedSources: [],
  localApiEnabled: false,
  localApiPort: 48123,
  localApiToken: "",
  pinnedRepos: [],
  pinnedAiProviders: [],
  aiBudgets: {
    claude5hTokens: 25_000_000,
    claudeWeeklyTokens: 150_000_000,
    gemini5hTokens: 100_000_000,
    geminiWeeklyTokens: 500_000_000,
    codex5hTokens: 25_000_000,
    codexWeeklyTokens: 150_000_000,
  },
  cameraPanelAdded: true,
};

const DEFAULT_SHORTCUTS: ShortcutStates = {
  toggle: false,
  capture: false,
  toggleError: "",
  captureError: "",
};

type PreferencesState = {
  preferences: Preferences;
  ready: boolean;
  saving: boolean;
  error: string;
  monitors: MonitorInfo[];
  shortcuts: ShortcutStates;
  load: () => Promise<void>;
  save: (change: Partial<Preferences>) => Promise<boolean>;
  refreshMonitors: () => Promise<void>;
  refreshShortcuts: () => Promise<void>;
};

export const usePreferences = create<PreferencesState>((set, get) => ({
  preferences: DEFAULT_PREFERENCES,
  ready: false,
  saving: false,
  error: "",
  monitors: [],
  shortcuts: DEFAULT_SHORTCUTS,
  load: async () => {
    try {
      const [preferences, monitors, shortcuts] = await Promise.all([
        invoke<Preferences>("get_preferences"),
        invoke<MonitorInfo[]>("list_monitors").catch(() => [] as MonitorInfo[]),
        invoke<ShortcutStates>("shortcut_states").catch(() => DEFAULT_SHORTCUTS),
      ]);
      set({ preferences, monitors, shortcuts, ready: true, error: "" });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  save: async (change) => {
    if (!get().ready || get().saving) return false;
    const next = { ...get().preferences, ...change };
    set({ saving: true, error: "" });
    try {
      const saved = await invoke<Preferences>("save_preferences", { preferences: next });
      const shortcuts = await invoke<ShortcutStates>("shortcut_states").catch(() => get().shortcuts);
      set({ preferences: saved, shortcuts, saving: false });
      return true;
    } catch (error) {
      set({ error: String(error), saving: false });
      return false;
    }
  },
  refreshMonitors: async () => {
    try {
      const monitors = await invoke<MonitorInfo[]>("list_monitors");
      set({ monitors });
    } catch {
      // Ignore transient display enumeration failures
    }
  },
  refreshShortcuts: async () => {
    try {
      const shortcuts = await invoke<ShortcutStates>("shortcut_states");
      set({ shortcuts });
    } catch {
      // Keep prior status
    }
  },
}));
