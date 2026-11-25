export type Mode =
  | "hidden"
  | "idle"
  | "lyrics"
  | "compactBadge"
  | "preview"
  | "activity"
  | "dragTarget"
  | "expanded";

export type PanelId =
  | "music"
  | "ai"
  | "files"
  | "capture"
  | "camera"
  | "controls"
  | "clipboard"
  | "timer"
  | "notes"
  | "launcher"
  | "notifications"
  | "settings";

export type Accent = "neutral" | "blue" | "green" | "amber" | "rose";
export type SizePreset = "compact" | "standard" | "large";
export type AutoCloseMode = "off" | "clickOutside" | "leaveAndClick";
export type AiProviderId = "claude" | "gemini" | "codex";

export type ProviderBudgets = {
  claude5hTokens: number;
  claudeWeeklyTokens: number;
  gemini5hTokens: number;
  geminiWeeklyTokens: number;
  codex5hTokens: number;
  codexWeeklyTokens: number;
};

export type Preferences = {
  hoverPreview: boolean;
  hoverDelayMs: number;
  alwaysVisible: boolean;
  hideInFullscreen: boolean;
  autoCloseMode: AutoCloseMode;
  reducedMotion: boolean;
  accent: Accent;
  sizePreset: SizePreset;
  monitorName: string;
  enabledPanels: PanelId[];
  toggleShortcut: string;
  captureShortcut: string;
  lyricsEnabled: boolean;
  idleLyricsEnabled: boolean;
  autostart: boolean;
  clipboardHistoryEnabled: boolean;
  notificationsEnabled: boolean;
  doNotDisturb: boolean;
  hideSensitiveNotifications: boolean;
  notificationHistoryEnabled: boolean;
  mutedSources: string[];
  localApiEnabled: boolean;
  localApiPort: number;
  localApiToken: string;
  pinnedRepos: string[];
  pinnedAiProviders: AiProviderId[];
  aiBudgets: ProviderBudgets;
  cameraPanelAdded: boolean;
  skippedUpdateVersion: string;
};

export type AutostartStatus = {
  enabledInPrefs: boolean;
  registered: boolean;
  pathMatches: boolean;
  targetExists: boolean;
  disabledByWindows: boolean;
  registeredPath: string;
};

export type MonitorInfo = {
  name: string;
  deviceName: string;
  isPrimary: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
  workX: number;
  workY: number;
  workWidth: number;
  workHeight: number;
  scale: number;
};

export type ShortcutStates = {
  toggle: boolean;
  capture: boolean;
  toggleError: string;
  captureError: string;
};

export type Priority = "low" | "normal" | "high" | "critical";
export type OngoingKind = "recording" | "timer";

export type OngoingBadge = {
  kind: OngoingKind;
  label: string;
  detail: string;
  paused: boolean;
};

export type ActivityItem = {
  id: string;
  source: string;
  title: string;
  body: string;
  priority: Priority;
  createdAtMs: number;
  expiresAtMs: number;
  actionPanel: PanelId | null;
  sensitive: boolean;
};

export type ActivityHubState = {
  activeBanner: ActivityItem | null;
  ongoing: OngoingBadge[];
  notifications: ActivityItem[];
  unreadCount: number;
};
