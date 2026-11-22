import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  Bell,
  Bot,
  Camera,
  Clipboard,
  Files,
  LogOut,
  Music2,
  Rocket,
  ScanLine,
  Settings2,
  SlidersHorizontal,
  StickyNote,
  Timer,
  Video,
  X,
} from "lucide-react";
import { PinnedAiStrip } from "./components/AiNotchIcons";
import {
  SolidPauseIcon,
  SolidPlayIcon,
  SolidRepeatIcon,
  SolidStopIcon,
} from "./components/SolidIcons";
import { AiPanel, computePinnedAiItems, useAiUsage } from "./features/ai/AiPanel";
import { CameraPanel } from "./features/camera/CameraPanel";
import { CapturePanel } from "./features/capture/CapturePanel";
import { ControlsPanel } from "./features/controls/ControlsPanel";
import { FilesPanel } from "./features/files/FilesPanel";
import {
  IdleLyricsBar,
  MusicPanel,
  useAlbumPalette,
  useLyrical,
  useMedia,
} from "./features/music/MusicPanel";
import { NotificationsPanel } from "./features/notifications/NotificationsPanel";
import { SettingsPanel } from "./features/settings/SettingsPanel";
import { ToolsPanel } from "./features/tools/ToolsPanel";
import { useActivity } from "./stores/activity";
import { usePreferences } from "./stores/preferences";
import { useUpdater } from "./stores/updater";
import type { Accent, AutoCloseMode, Mode, OngoingBadge, PanelId, SizePreset } from "./types/peek";

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

const ACCENTS: Record<Accent, string> = {
  neutral: "#e8e8e8",
  blue: "#b8ceff",
  green: "#b2e2c5",
  amber: "#f4cf8a",
  rose: "#f2b8c6",
};

const MAX_EXPANDED_HEIGHT: Record<SizePreset, number> = {
  compact: 368,
  standard: 424,
  large: 480,
};

const EXPAND_SPRING = {
  type: "spring" as const,
  stiffness: 360,
  damping: 25,
  mass: 0.78,
};

const COLLAPSE_SPRING = {
  type: "spring" as const,
  stiffness: 360,
  damping: 30,
  mass: 0.72,
};

type FileDragPayload = {
  kind: "enter" | "over" | "leave" | "drop";
  paths: string[];
};

function logicalDimensions(
  mode: Mode,
  preset: SizePreset,
  expandedHeight: number,
  activePanel?: PanelId,
  idleLyricsWidth?: number,
  pinnedAiCount?: number,
) {
  switch (mode) {
    case "hidden":
      return { width: 120, height: 0, radius: 14 };
    case "idle":
      return pinnedAiCount && pinnedAiCount > 0
        ? { width: 118 + pinnedAiCount * 46, height: 32, radius: 17 }
        : { width: 108, height: 30, radius: 16 };
    case "lyrics":
      return { width: idleLyricsWidth ?? 220, height: 38, radius: 20 };
    case "compactBadge":
      return { width: 236, height: 34, radius: 18 };
    case "preview":
      return { width: 464, height: 42, radius: 22 };
    case "activity":
      return { width: 336, height: 52, radius: 24 };
    case "dragTarget":
      return { width: 480, height: 360, radius: 36 };
    case "expanded": {
      const width =
        activePanel === "music"
          ? preset === "compact"
            ? 536
            : preset === "large"
              ? 592
              : 564
          : preset === "compact"
            ? 480
            : preset === "large"
              ? 556
              : 504;
      const height = Math.min(MAX_EXPANDED_HEIGHT[preset], Math.max(184, expandedHeight));
      return { width, height, radius: 36 };
    }
  }
}

function useClock(): string {
  const formatNow = () =>
    new Date().toLocaleTimeString([], {
      hour: "numeric",
      minute: "2-digit",
    });
  const [clock, setClock] = useState(formatNow);
  useEffect(() => {
    const timer = setInterval(() => {
      setClock(formatNow());
    }, 1000);
    return () => clearInterval(timer);
  }, []);
  return clock;
}

function OngoingPillBadge({
  badge,
  onOpen,
}: {
  badge: OngoingBadge;
  onOpen: (panel: PanelId) => void;
}) {
  if (badge.kind === "timer") {
    return (
      <div className={"ongoing-inline timer" + (badge.paused ? " paused" : "")}>
        <button
          type="button"
          className="ongoing-summary-btn"
          onClick={(e) => {
            e.stopPropagation();
            onOpen("timer");
          }}
          title={badge.label + (badge.detail ? " — " + badge.detail : "")}
        >
          <span
            className={"status-red-dot" + (badge.paused ? " paused" : "")}
            aria-hidden="true"
          />
          <span className="ongoing-detail">{badge.detail || badge.label}</span>
        </button>
        <div className="ongoing-actions" role="group" aria-label="Timer controls">
          <button
            type="button"
            className="ongoing-action-btn"
            aria-label={badge.paused ? "Resume timer" : "Pause timer"}
            title={badge.paused ? "Resume" : "Pause"}
            onClick={(e) => {
              e.stopPropagation();
              void invoke(badge.paused ? "resume_timer" : "pause_timer").catch(console.error);
            }}
          >
            {badge.paused ? <SolidPlayIcon size={11} /> : <SolidPauseIcon size={11} />}
          </button>
          <button
            type="button"
            className="ongoing-action-btn"
            aria-label="Stop timer"
            title="Stop"
            onClick={(e) => {
              e.stopPropagation();
              void invoke("reset_timer").catch(console.error);
            }}
          >
            <SolidStopIcon size={11} />
          </button>
          <button
            type="button"
            className="ongoing-action-btn"
            aria-label="Restart timer"
            title="Repeat"
            onClick={(e) => {
              e.stopPropagation();
              void invoke("restart_timer").catch(console.error);
            }}
          >
            <SolidRepeatIcon size={11} />
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className={"ongoing-inline recording" + (badge.paused ? " paused" : "")}>
      <button
        type="button"
        className="ongoing-summary-btn"
        onClick={(e) => {
          e.stopPropagation();
          onOpen("capture");
        }}
        title={badge.label + (badge.detail ? " — " + badge.detail : "")}
      >
        <span
          className={"status-red-dot recording" + (badge.paused ? " paused" : "")}
          aria-hidden="true"
        />
        <Video size={12} strokeWidth={2.2} />
        <span className="ongoing-detail">{badge.detail || badge.label}</span>
      </button>
      <div className="ongoing-actions" role="group" aria-label="Recording controls">
        <button
          type="button"
          className="ongoing-action-btn stop"
          aria-label="Stop recording"
          title="Stop recording"
          onClick={(e) => {
            e.stopPropagation();
            void invoke("stop_recording").catch(console.error);
          }}
        >
          <SolidStopIcon size={11} />
        </button>
      </div>
    </div>
  );
}

export function App() {
  const [mode, setMode] = useState<Mode>("idle");
  const [hiddenSettled, setHiddenSettled] = useState(false);
  const [expandedHeight, setExpandedHeight] = useState(260);
  const [lyricsWidth, setLyricsWidth] = useState(220);
  const [growingTransition, setGrowingTransition] = useState(true);
  const [error, setError] = useState("");
  const [panel, setPanel] = useState<PanelId>("music");
  const [visible, setVisible] = useState(true);
  const [fileDragOver, setFileDragOver] = useState(false);
  const [draggedFiles, setDraggedFiles] = useState<string[]>([]);
  const clockTime = useClock();

  const { preferences, ready, load } = usePreferences();
  const activity = useActivity();
  const updater = useUpdater();
  const systemReducedMotion = useReducedMotion();
  const reduceMotion = preferences.reducedMotion || systemReducedMotion;

  const hoverTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const leavePreviewTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const autoCloseTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const dragLeaveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const settleTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const openedForDragRef = useRef(false);
  const openedByHoverRef = useRef(false);
  const draggingOutRef = useRef(false);
  const fileDragOverRef = useRef(false);
  fileDragOverRef.current = fileDragOver;
  const autoCloseModeRef = useRef<AutoCloseMode>(preferences.autoCloseMode);
  autoCloseModeRef.current = preferences.autoCloseMode;

  const expandedHeightRef = useRef(expandedHeight);
  expandedHeightRef.current = expandedHeight;
  const lyricsWidthRef = useRef(lyricsWidth);
  const expandedMeasureRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<PanelId>(panel);
  panelRef.current = panel;
  const prevPanelIsMusicRef = useRef(panel === "music");
  const prevPinnedCountRef = useRef(preferences.pinnedAiProviders.length);

  const requestVersion = useRef(0);
  const requestedMode = useRef<Mode>("idle");
  const activeTransitionRef = useRef<{
    token: number;
    mode: Mode;
    targetWidth: number;
    targetHeight: number;
    settled: boolean;
  } | null>(null);
  const shellRef = useRef<HTMLElement>(null);
  const prevSizePresetRef = useRef<SizePreset>(preferences.sizePreset);

  const navPanels = useMemo(() => {
    const enabled = preferences.enabledPanels.filter((id): id is PanelId => id in PANEL_META);
    return enabled.length > 0 ? enabled : (["settings"] as PanelId[]);
  }, [preferences.enabledPanels]);
  const navPanelsRef = useRef<PanelId[]>(navPanels);
  navPanelsRef.current = navPanels;

  useEffect(() => {
    if (!navPanels.includes(panel)) {
      setPanel(navPanels[0]);
    }
  }, [navPanels, panel]);

  const settleActiveTransition = useCallback(() => {
    clearTimeout(settleTimer.current);
    const active = activeTransitionRef.current;
    if (!active || active.settled) return;
    active.settled = true;
    if (active.mode === "hidden") {
      setHiddenSettled(true);
    }
    void invoke("settle_island_transition", {
      mode: active.mode,
      targetWidth: active.targetWidth,
      targetHeight: active.targetHeight,
      token: active.token,
    }).catch(() => {});
  }, []);

  const changeMode = useCallback(
    async (
      next: Mode,
      opts?: {
        focus?: boolean;
        targetHeightOverride?: number;
        targetWidthOverride?: number;
        prevPinnedCountOverride?: number;
        panelOverride?: PanelId;
      },
    ) => {
      clearTimeout(hoverTimer.current);
      clearTimeout(leavePreviewTimer.current);
      clearTimeout(autoCloseTimer.current);
      clearTimeout(settleTimer.current);

      const prevMode = requestedMode.current;
      const prefsSnapshot = usePreferences.getState().preferences;
      const preset = prefsSnapshot.sizePreset;
      const pinnedCount = prefsSnapshot.pinnedAiProviders.length;
      const activePanel = opts?.panelOverride ?? panelRef.current;
      const expH = opts?.targetHeightOverride ?? expandedHeightRef.current;
      const prevDims = logicalDimensions(
        prevMode,
        preset,
        expandedHeightRef.current,
        panelRef.current,
        lyricsWidthRef.current,
        opts?.prevPinnedCountOverride ?? pinnedCount,
      );
      const nextDims = logicalDimensions(
        next,
        preset,
        expH,
        activePanel,
        opts?.targetWidthOverride ?? lyricsWidthRef.current,
        pinnedCount,
      );
      const isGrowing = nextDims.width * nextDims.height >= prevDims.width * prevDims.height;

      requestedMode.current = next;
      const version = ++requestVersion.current;
      const animate =
        !usePreferences.getState().preferences.reducedMotion &&
        !window.matchMedia("(prefers-reduced-motion: reduce)").matches;

      setGrowingTransition(isGrowing);
      setHiddenSettled(false);

      try {
        const token = await invoke<number>("prepare_island_transition", {
          mode: next,
          targetWidth: nextDims.width,
          targetHeight: nextDims.height,
          focus: opts?.focus ?? true,
        });
        if (version !== requestVersion.current) return;
        activeTransitionRef.current = {
          token,
          mode: next,
          targetWidth: nextDims.width,
          targetHeight: nextDims.height,
          settled: false,
        };
        setMode(next);
        setError("");

        const sameSize = prevDims.width === nextDims.width && prevDims.height === nextDims.height;
        const safetyDelay = !animate || sameSize ? 24 : 420;
        settleTimer.current = setTimeout(() => {
          settleActiveTransition();
        }, safetyDelay);
      } catch (failure) {
        if (version === requestVersion.current) {
          setError(String(failure));
        }
      }
    },
    [settleActiveTransition],
  );

  const handleIdleLyricsWidth = useCallback(
    (nextW: number) => {
      const clamped = Math.max(96, Math.min(540, Math.round(nextW)));
      const aligned = clamped % 2 === 0 ? clamped : clamped + 1;
      if (Math.abs(aligned - lyricsWidthRef.current) < 4) return;
      if (requestedMode.current === "lyrics") {
        // Call changeMode before mutating lyricsWidthRef so prevDims reflects the prior width.
        void changeMode("lyrics", { focus: false, targetWidthOverride: aligned });
      }
      lyricsWidthRef.current = aligned;
      setLyricsWidth(aligned);
    },
    [changeMode],
  );

  const openPanel = useCallback(
    (target: PanelId) => {
      openedForDragRef.current = false;
      openedByHoverRef.current = false;
      panelRef.current = target;
      setPanel(target);
      void changeMode("expanded", { focus: true, panelOverride: target });
    },
    [changeMode],
  );

  const openLastPanelOnHover = useCallback(() => {
    const list = navPanelsRef.current;
    const target = list.includes(panelRef.current) ? panelRef.current : list[0];
    openedForDragRef.current = false;
    openedByHoverRef.current = true;
    panelRef.current = target;
    setPanel(target);
    void changeMode("expanded", { focus: false, panelOverride: target });
  }, [changeMode]);

  const watchMediaActive =
    visible &&
    ((preferences.lyricsEnabled && preferences.idleLyricsEnabled) ||
      mode === "preview" ||
      (mode === "expanded" && panel === "music"));
  const media = useMedia(watchMediaActive);
  const mediaPlaying = media.status === "ready" && media.track.playing;
  const lyricsActive =
    visible &&
    preferences.lyricsEnabled &&
    ((mode === "expanded" && panel === "music") ||
      (mediaPlaying && preferences.idleLyricsEnabled) ||
      mode === "lyrics");
  const lyrical = useLyrical(media, lyricsActive, preferences.lyricsEnabled);

  const watchAiActive =
    visible && (preferences.pinnedAiProviders.length > 0 || (mode === "expanded" && panel === "ai"));
  const aiUsage = useAiUsage(watchAiActive);
  const pinnedAiItems = useMemo(
    () =>
      computePinnedAiItems(
        aiUsage.snapshot,
        preferences.pinnedAiProviders,
        preferences.aiBudgets,
      ),
    [aiUsage.snapshot, preferences.pinnedAiProviders, preferences.aiBudgets],
  );

  const updatePromptActive =
    (Boolean(updater.snapshot.available) && !updater.snapshot.dismissed) ||
    updater.snapshot.status === "downloading" ||
    updater.snapshot.status === "installing";

  const collapsedTargetMode: Mode =
    activity.ongoing.length > 0
      ? "compactBadge"
      : activity.activeBanner || updatePromptActive
        ? "activity"
        : preferences.alwaysVisible
          ? mediaPlaying && preferences.lyricsEnabled && preferences.idleLyricsEnabled
            ? "lyrics"
            : "idle"
          : "hidden";
  const collapsedTargetRef = useRef<Mode>(collapsedTargetMode);
  collapsedTargetRef.current = collapsedTargetMode;

  useEffect(() => {
    return () => {
      clearTimeout(hoverTimer.current);
      clearTimeout(leavePreviewTimer.current);
      clearTimeout(autoCloseTimer.current);
      clearTimeout(dragLeaveTimer.current);
      clearTimeout(settleTimer.current);
    };
  }, []);

  useEffect(() => {
    void load();
    const stopAct = activity.init();
    const stopUpd = updater.init();
    return () => {
      stopAct();
      stopUpd();
    };
  }, [load, activity.init, updater.init]);

  // Measure the expanded container's natural content height so short panels (e.g. empty Files) stay compact.
  useEffect(() => {
    if (mode !== "expanded") return;
    const el = expandedMeasureRef.current;
    if (!el) return;
    const maxH = MAX_EXPANDED_HEIGHT[preferences.sizePreset];
    const measure = () => {
      const nextH = Math.min(maxH, Math.max(184, Math.ceil(el.getBoundingClientRect().height)));
      setExpandedHeight((prev) => {
        if (Math.abs(prev - nextH) < 2) return prev;
        if (requestedMode.current === "expanded") {
          void changeMode("expanded", {
            focus: false,
            targetHeightOverride: nextH,
            panelOverride: panelRef.current,
          });
        }
        return nextH;
      });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [mode, panel, preferences.sizePreset, activity.ongoing.length, updatePromptActive, changeMode]);

  // Synchronize collapsed presentation when visibility setting, ongoing badge, or banner changes.
  useEffect(() => {
    if (!ready) return;
    if (mode !== "expanded" && mode !== "preview" && mode !== "dragTarget") {
      if (requestedMode.current !== collapsedTargetMode) {
        void changeMode(collapsedTargetMode);
      }
    }
  }, [ready, collapsedTargetMode, mode, changeMode]);

  // Smoothly animate resized expanded panel when panel size preset changes.
  useEffect(() => {
    if (!ready) return;
    if (prevSizePresetRef.current !== preferences.sizePreset) {
      prevSizePresetRef.current = preferences.sizePreset;
      if (requestedMode.current === "expanded") {
        void changeMode("expanded", { focus: false });
      }
    }
  }, [ready, preferences.sizePreset, changeMode]);

  // Smoothly resize expanded width when switching into or out of the wider Music workspace.
  useEffect(() => {
    const isMusic = panel === "music";
    if (!ready) {
      prevPanelIsMusicRef.current = isMusic;
      return;
    }
    if (prevPanelIsMusicRef.current !== isMusic) {
      prevPanelIsMusicRef.current = isMusic;
      if (requestedMode.current === "expanded") {
        void changeMode("expanded", { focus: false, panelOverride: panel });
      }
    }
  }, [ready, panel, changeMode]);

  // Resize the idle notch smoothly if pinned AI count changes while idle.
  useEffect(() => {
    const count = preferences.pinnedAiProviders.length;
    if (!ready) {
      prevPinnedCountRef.current = count;
      return;
    }
    if (prevPinnedCountRef.current !== count) {
      const prevCount = prevPinnedCountRef.current;
      prevPinnedCountRef.current = count;
      if (requestedMode.current === "idle") {
        void changeMode("idle", { focus: false, prevPinnedCountOverride: prevCount });
      }
    }
  }, [ready, preferences.pinnedAiProviders.length, changeMode]);

  useEffect(() => {
    const subscriptions = [
      listen("open-settings", () => {
        openedForDragRef.current = false;
        openedByHoverRef.current = false;
        panelRef.current = "settings";
        setPanel("settings");
        void changeMode("expanded", { focus: true, panelOverride: "settings" });
      }),
      listen<string>("open-panel", ({ payload }) => {
        if (payload in PANEL_META) {
          openedForDragRef.current = false;
          openedByHoverRef.current = false;
          const target = payload as PanelId;
          panelRef.current = target;
          setPanel(target);
          void changeMode("expanded", { focus: true, panelOverride: target });
        }
      }),
      listen<boolean>("toggle-peek", ({ payload: wasHidden }) => {
        if (wasHidden) setVisible(true);
        openedByHoverRef.current = false;
        const next =
          wasHidden || requestedMode.current !== "expanded"
            ? "expanded"
            : collapsedTargetRef.current;
        void changeMode(next, { focus: next === "expanded" });
      }),
      listen<boolean>("peek-visibility", ({ payload }) => setVisible(payload)),
      listen("top-edge-hover", () => {
        clearTimeout(leavePreviewTimer.current);
        clearTimeout(autoCloseTimer.current);
        if (
          requestedMode.current === "hidden" ||
          requestedMode.current === "idle" ||
          requestedMode.current === "lyrics"
        ) {
          openLastPanelOnHover();
        }
      }),
      listen("peek-blur", () => {
        if (
          requestedMode.current === "expanded" &&
          autoCloseModeRef.current !== "off" &&
          !draggingOutRef.current &&
          !fileDragOverRef.current
        ) {
          openedForDragRef.current = false;
          openedByHoverRef.current = false;
          void changeMode(collapsedTargetRef.current);
        }
      }),
      listen<FileDragPayload>("file-drag", ({ payload }) => {
        if (payload.kind === "enter") {
          clearTimeout(dragLeaveTimer.current);
          clearTimeout(leavePreviewTimer.current);
          clearTimeout(autoCloseTimer.current);
          setFileDragOver(true);
          if (payload.paths.length > 0) {
            setDraggedFiles(payload.paths);
          }
          if (requestedMode.current !== "expanded") {
            openedForDragRef.current = true;
          }
          openedByHoverRef.current = false;
          panelRef.current = "files";
          setPanel("files");
          void changeMode("expanded", { focus: false, panelOverride: "files" });
        } else if (payload.kind === "over") {
          clearTimeout(dragLeaveTimer.current);
          setFileDragOver(true);
        } else if (payload.kind === "leave") {
          clearTimeout(dragLeaveTimer.current);
          dragLeaveTimer.current = setTimeout(() => {
            setFileDragOver(false);
            setDraggedFiles([]);
            if (openedForDragRef.current && !draggingOutRef.current) {
              openedForDragRef.current = false;
              openedByHoverRef.current = false;
              void changeMode(collapsedTargetRef.current);
            }
          }, 260);
        } else if (payload.kind === "drop") {
          clearTimeout(dragLeaveTimer.current);
          setFileDragOver(false);
          setDraggedFiles([]);
          openedForDragRef.current = false;
          openedByHoverRef.current = false;
          panelRef.current = "files";
          setPanel("files");
          void changeMode("expanded", { focus: true, panelOverride: "files" });
        }
      }),
    ];
    return () => {
      for (const sub of subscriptions) {
        void sub.then((unlisten) => unlisten()).catch(console.error);
      }
    };
  }, [changeMode, openLastPanelOnHover]);

  useEffect(() => {
    if (mode === "expanded" && !fileDragOver) {
      shellRef.current?.focus();
    }
  }, [mode, fileDragOver]);

  function handleKeyDown(event: React.KeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      openedForDragRef.current = false;
      openedByHoverRef.current = false;
      void changeMode(collapsedTargetMode);
      return;
    }
    if (mode === "expanded" && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      openPanel("launcher");
      return;
    }
    if (
      mode === "expanded" &&
      (event.key === "ArrowLeft" || event.key === "ArrowRight") &&
      !(
        event.target instanceof HTMLInputElement ||
        event.target instanceof HTMLTextAreaElement ||
        event.target instanceof HTMLSelectElement
      )
    ) {
      const idx = navPanels.indexOf(panel);
      if (idx !== -1) {
        const nextIdx =
          event.key === "ArrowRight"
            ? (idx + 1) % navPanels.length
            : (idx - 1 + navPanels.length) % navPanels.length;
        const nextPanel = navPanels[nextIdx];
        panelRef.current = nextPanel;
        setPanel(nextPanel);
      }
    }
  }

  const musicThemeActive =
    media.status === "ready" &&
    ((mode === "expanded" && panel === "music") || mode === "lyrics");
  const artworkUrl = musicThemeActive ? media.track.artwork : null;
  const albumPalette = useAlbumPalette(artworkUrl);
  const dims = logicalDimensions(
    mode,
    preferences.sizePreset,
    expandedHeight,
    panel,
    lyricsWidth,
    preferences.pinnedAiProviders.length,
  );
  const maxPanelHeight =
    MAX_EXPANDED_HEIGHT[preferences.sizePreset] - (activity.ongoing.length > 0 ? 96 : 64);
  const springTransition = reduceMotion
    ? { duration: 0 }
    : growingTransition
      ? EXPAND_SPRING
      : COLLAPSE_SPRING;
  const contentViewKey =
    mode === "hidden"
      ? "hidden"
      : mode === "activity" && activity.activeBanner
        ? `activity-${activity.activeBanner.id}`
        : mode === "preview"
          ? "preview"
          : mode === "expanded"
            ? "expanded"
            : activity.ongoing.length > 0
              ? "compact-ongoing"
              : mode === "lyrics" && media.status === "ready"
                ? "idle-lyrics"
                : "idle-clock";

  const expandedMusicTheme = mode === "expanded" && panel === "music" && media.status === "ready";
  const earLeftColor = expandedMusicTheme
    ? `rgb(${albumPalette.earLeft.join(", ")})`
    : "#080808";
  const earRightColor = expandedMusicTheme
    ? `rgb(${albumPalette.earRight.join(", ")})`
    : "#080808";
  const shellStyle = musicThemeActive
    ? ({
        "--album-c1": albumPalette.c1.join(", "),
        "--album-c2": albumPalette.c2.join(", "),
        "--album-deep": albumPalette.deep.join(", "),
        "--album-accent": albumPalette.accentRgb,
        "--album-ear-left": albumPalette.earLeft.join(", "),
        "--album-ear-right": albumPalette.earRight.join(", "),
      } as CSSProperties)
    : undefined;

  return (
    <div
      className="island-stage"
      data-reduce-motion={Boolean(reduceMotion)}
      style={{ "--accent": ACCENTS[preferences.accent] } as CSSProperties}
    >
      <div
        className={"notch-wrap" + (mode === "hidden" ? " is-hidden" : "")}
        style={
          {
            "--notch-ear-left": earLeftColor,
            "--notch-ear-right": earRightColor,
          } as CSSProperties
        }
      >
        <svg
          className="notch-ear left"
          width="10"
          height="10"
          viewBox="0 0 10 10"
          aria-hidden="true"
        >
          <path d="M 0 0 L 10 0 L 10 10 L 8.2 10 A 8.2 8.2 0 0 0 0 1.8 Z" fill="currentColor" />
        </svg>

        <motion.main
          ref={shellRef}
          tabIndex={-1}
          className={
            "notch-shell " +
            (mode === "hidden" ? "mode-hidden" : mode) +
            (mode === "hidden" && hiddenSettled ? " hidden-settled" : "") +
            (musicThemeActive ? " music-themed" : "")
          }
          style={shellStyle}
          initial={false}
          animate={{
            width: dims.width,
            height: dims.height,
            borderBottomLeftRadius: dims.radius,
            borderBottomRightRadius: dims.radius,
            opacity: mode === "hidden" && hiddenSettled ? 0 : 1,
          }}
          transition={springTransition}
          onAnimationComplete={settleActiveTransition}
          onMouseEnter={() => {
            clearTimeout(leavePreviewTimer.current);
            clearTimeout(autoCloseTimer.current);
            if (mode === "hidden") {
              openLastPanelOnHover();
              return;
            }
            if (
              (mode === "idle" || mode === "lyrics") &&
              requestedMode.current === mode &&
              preferences.hoverPreview
            ) {
              hoverTimer.current = setTimeout(
                () => openLastPanelOnHover(),
                preferences.hoverDelayMs,
              );
            }
          }}
          onMouseLeave={(e) => {
            if (e.buttons !== 0) return;
            const rect = e.currentTarget.getBoundingClientRect();
            if (
              e.clientX > rect.left + 1 &&
              e.clientX < rect.right - 1 &&
              e.clientY > rect.top + 1 &&
              e.clientY < rect.bottom - 1
            ) {
              return;
            }
            clearTimeout(hoverTimer.current);
            if (requestedMode.current === "preview") {
              leavePreviewTimer.current = setTimeout(() => {
                if (requestedMode.current === "preview") {
                  void changeMode(collapsedTargetRef.current);
                }
              }, 240);
              return;
            }
            if (
              requestedMode.current === "expanded" &&
              (autoCloseModeRef.current === "leaveAndClick" || openedByHoverRef.current) &&
              !draggingOutRef.current &&
              !fileDragOverRef.current
            ) {
              const activeEl = document.activeElement as HTMLElement | null;
              const tag = activeEl?.tagName;
              const isTextInput =
                tag === "INPUT" && (activeEl as HTMLInputElement).type !== "range";
              if (tag === "SELECT" || tag === "TEXTAREA" || isTextInput) {
                return;
              }
              clearTimeout(autoCloseTimer.current);
              autoCloseTimer.current = setTimeout(() => {
                if (
                  requestedMode.current === "expanded" &&
                  (autoCloseModeRef.current === "leaveAndClick" || openedByHoverRef.current) &&
                  !draggingOutRef.current &&
                  !fileDragOverRef.current
                ) {
                  openedForDragRef.current = false;
                  openedByHoverRef.current = false;
                  void changeMode(collapsedTargetRef.current);
                }
              }, 130);
            }
          }}
          onKeyDown={handleKeyDown}
        >
          {musicThemeActive && (
            <div className="music-bg-clip" aria-hidden="true">
              <div
                className={"music-sway-bg" + (artworkUrl ? " has-art" : "")}
                style={artworkUrl ? { backgroundImage: `url("${artworkUrl}")` } : undefined}
              />
            </div>
          )}
          <motion.div
            key={contentViewKey}
            className="notch-inner"
            initial={
              reduceMotion
                ? false
                : { opacity: 0, scale: 0.96, filter: "blur(3px)" }
            }
            animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
            transition={
              reduceMotion
                ? { duration: 0 }
                : { duration: 0.16, ease: [0.22, 1, 0.36, 1] }
            }
          >
            {mode === "hidden" ? (
              <div className="hidden-edge-trigger" aria-hidden="true" />
            ) : mode === "activity" && activity.activeBanner ? (
              <div
                className="activity-banner"
                role="status"
                onClick={() => {
                  const target = activity.activeBanner?.actionPanel as PanelId | null;
                  if (target && target in PANEL_META) {
                    openPanel(target);
                  } else {
                    openPanel("notifications");
                  }
                }}
              >
                <div className="activity-banner-text">
                  <strong>{activity.activeBanner.title}</strong>
                  {activity.activeBanner.body && <span>{activity.activeBanner.body}</span>}
                </div>
                <button
                  type="button"
                  className="banner-dismiss"
                  aria-label="Dismiss notification"
                  onClick={(e) => {
                    e.stopPropagation();
                    if (activity.activeBanner) {
                      void activity.dismissBanner(activity.activeBanner.id);
                    }
                  }}
                >
                  <X size={14} />
                </button>
              </div>
            ) : mode === "activity" && updatePromptActive ? (
              <div
                className="update-pill-prompt"
                role="status"
                onClick={() => openPanel("settings")}
              >
                <div className="update-pill-text">
                  <strong>
                    {updater.snapshot.status === "installing"
                      ? "Installing update…"
                      : updater.snapshot.status === "downloading"
                        ? `Updating ${updater.snapshot.progressPercent ?? 0}%`
                        : `Peek v${updater.snapshot.available?.version} ready`}
                  </strong>
                  {updater.snapshot.status === "downloading" ? (
                    <div className="update-progress-track">
                      <div
                        className="update-progress-fill"
                        style={{ width: `${updater.snapshot.progressPercent ?? 0}%` }}
                      />
                    </div>
                  ) : (
                    <span>
                      {updater.snapshot.status === "installing"
                        ? "Restarting automatically"
                        : "Signed Windows update"}
                    </span>
                  )}
                </div>
                {updater.snapshot.status !== "downloading" &&
                  updater.snapshot.status !== "installing" && (
                    <div className="update-pill-actions">
                      <button
                        type="button"
                        className="btn primary sm"
                        onClick={(e) => {
                          e.stopPropagation();
                          void updater.install();
                        }}
                      >
                        Update
                      </button>
                      <button
                        type="button"
                        className="btn sm"
                        onClick={(e) => {
                          e.stopPropagation();
                          void updater.dismiss();
                        }}
                      >
                        Later
                      </button>
                    </div>
                  )}
              </div>
            ) : mode === "preview" ? (
              <div className="hover-bar" aria-label="Peek quick bar">
                <span className="hover-clock">{clockTime}</span>
                <div className="hover-icons">
                  {navPanels.map((id) => {
                    const { label, icon: Icon } = PANEL_META[id];
                    const hasAlertDot = id === "notifications" && activity.unreadCount > 0;
                    const hasPlayDot = id === "music" && mediaPlaying;
                    const hasUpdateDot = id === "settings" && Boolean(updater.snapshot.available);
                    return (
                      <button
                        key={id}
                        type="button"
                        className="hover-icon-btn"
                        aria-label={`Open ${label}`}
                        title={label}
                        onClick={() => openPanel(id)}
                      >
                        <Icon size={15} strokeWidth={1.85} />
                        {(hasAlertDot || hasPlayDot || hasUpdateDot) && (
                          <span className="nav-badge" aria-hidden="true" />
                        )}
                      </button>
                    );
                  })}
                </div>
              </div>
            ) : mode !== "expanded" ? (
              activity.ongoing.length > 0 ? (
                <div
                  className="pill pill-ongoing-wrap"
                  role="button"
                  tabIndex={0}
                  aria-label="Open active task"
                  onClick={() =>
                    openPanel(activity.ongoing[0].kind === "timer" ? "timer" : "capture")
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      openPanel(activity.ongoing[0].kind === "timer" ? "timer" : "capture");
                    }
                  }}
                >
                  <OngoingPillBadge badge={activity.ongoing[0]} onOpen={openPanel} />
                </div>
              ) : mode === "lyrics" && media.status === "ready" ? (
                <IdleLyricsBar
                  track={media.track}
                  lyrical={lyrical}
                  accentRgb={albumPalette.accentRgb}
                  reduceMotion={Boolean(reduceMotion)}
                  pinnedAiItems={pinnedAiItems}
                  onTargetWidthChange={handleIdleLyricsWidth}
                  onOpenMusic={() => openPanel("music")}
                  onOpenAi={() => openPanel("ai")}
                />
              ) : pinnedAiItems.length > 0 ? (
                <div
                  className="pill idle-with-ai"
                  role="button"
                  tabIndex={0}
                  onClick={() => openPanel(navPanels.includes(panel) ? panel : navPanels[0])}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      openPanel(navPanels.includes(panel) ? panel : navPanels[0]);
                    }
                  }}
                  aria-label="Open Peek"
                >
                  <span className="notch-clock">{clockTime}</span>
                  <span className="idle-ai-divider" aria-hidden="true" />
                  <PinnedAiStrip items={pinnedAiItems} onOpenAi={() => openPanel("ai")} />
                </div>
              ) : (
                <button
                  type="button"
                  className="pill"
                  onClick={() => openPanel(navPanels.includes(panel) ? panel : navPanels[0])}
                  aria-label="Open Peek"
                >
                  <span className="notch-clock">{clockTime}</span>
                </button>
              )
            ) : (
              <div
                ref={expandedMeasureRef}
                className="expanded-shell"
                style={{ "--panel-max-h": `${maxPanelHeight}px` } as CSSProperties}
                onPointerDown={() => {
                  openedByHoverRef.current = false;
                }}
              >
                <header className="island-header">
                  <span className="header-clock">{clockTime}</span>
                  <nav aria-label="Peek tools" className="island-nav">
                    {navPanels.map((id) => {
                      const { label, icon: Icon } = PANEL_META[id];
                      const badgeCount = id === "notifications" ? activity.unreadCount : 0;
                      const hasUpdateBadge = id === "settings" && Boolean(updater.snapshot.available);
                      const selected = panel === id;
                      return (
                        <button
                          key={id}
                          type="button"
                          aria-label={label}
                          title={label}
                          aria-pressed={selected}
                          onClick={() => {
                            openedForDragRef.current = false;
                            panelRef.current = id;
                            setPanel(id);
                          }}
                          className={"nav-button " + (selected ? "selected" : "")}
                        >
                          {selected && (
                            <motion.span
                              layoutId="nav-active-pill"
                              className="nav-active-bg"
                              transition={
                                reduceMotion
                                  ? { duration: 0 }
                                  : { type: "spring", stiffness: 460, damping: 32 }
                              }
                            />
                          )}
                          <span className="nav-icon-layer">
                            <Icon size={15} strokeWidth={1.85} />
                          </span>
                          {(badgeCount > 0 || hasUpdateBadge) && !selected && (
                            <span className="nav-badge" aria-hidden="true" />
                          )}
                        </button>
                      );
                    })}
                  </nav>
                  <button
                    type="button"
                    className="quit-button"
                    onClick={() => void invoke("quit_app")}
                    aria-label="Quit Peek"
                    title="Quit Peek"
                  >
                    <LogOut size={15} strokeWidth={1.9} />
                  </button>
                </header>

                {activity.ongoing.length > 0 && (
                  <div className="ongoing-strip" aria-label="Active tasks">
                    {activity.ongoing.map((badge) => (
                      <OngoingPillBadge key={badge.kind} badge={badge} onOpen={openPanel} />
                    ))}
                  </div>
                )}

                {updatePromptActive && (
                  <div className="update-banner" role="status">
                    <div className="update-banner-top">
                      <div className="update-banner-text">
                        <strong>
                          {updater.snapshot.status === "installing"
                            ? "Installing Peek update…"
                            : updater.snapshot.status === "downloading"
                              ? `Downloading Peek v${updater.snapshot.available?.version} (${updater.snapshot.progressPercent ?? 0}%)`
                              : `Peek v${updater.snapshot.available?.version} available`}
                        </strong>
                        {updater.snapshot.available?.notes &&
                          updater.snapshot.status === "available" && (
                            <span>{updater.snapshot.available.notes}</span>
                          )}
                      </div>
                      {updater.snapshot.status !== "downloading" &&
                        updater.snapshot.status !== "installing" && (
                          <div className="update-banner-actions">
                            <button
                              type="button"
                              className="btn primary sm"
                              onClick={() => void updater.install()}
                            >
                              Update now
                            </button>
                            <button
                              type="button"
                              className="btn sm"
                              onClick={() => void updater.dismiss()}
                            >
                              Later
                            </button>
                          </div>
                        )}
                    </div>
                    {updater.snapshot.status === "downloading" && (
                      <div className="update-progress-track">
                        <div
                          className="update-progress-fill"
                          style={{ width: `${updater.snapshot.progressPercent ?? 0}%` }}
                        />
                      </div>
                    )}
                  </div>
                )}

                <AnimatePresence mode="wait" initial={false}>
                  <motion.div
                    key={panel}
                    initial={
                      reduceMotion
                        ? { opacity: 1 }
                        : { opacity: 0, y: 5, scale: 0.99, filter: "blur(3px)" }
                    }
                    animate={{ opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }}
                    exit={
                      reduceMotion
                        ? { opacity: 0 }
                        : { opacity: 0, y: -3, scale: 0.99, filter: "blur(2px)" }
                    }
                    transition={
                      reduceMotion
                        ? { duration: 0 }
                        : { duration: 0.14, ease: [0.22, 1, 0.36, 1] }
                    }
                    className={
                      "panel-content" + (activity.ongoing.length > 0 ? " with-ongoing" : "")
                    }
                  >
                    {panel === "settings" ? (
                      <SettingsPanel />
                    ) : panel === "music" ? (
                      <MusicPanel
                        snapshot={media}
                        lyrical={lyrical}
                        accentRgb={albumPalette.accentRgb}
                      />
                    ) : panel === "ai" ? (
                      <AiPanel
                        snapshot={aiUsage.snapshot}
                        error={aiUsage.error}
                        onRefresh={aiUsage.refresh}
                      />
                    ) : panel === "files" ? (
                      <FilesPanel
                        isDragOver={fileDragOver}
                        draggedPaths={draggedFiles}
                        onDraggingOutChange={(dragging) => {
                          draggingOutRef.current = dragging;
                        }}
                      />
                    ) : panel === "capture" ? (
                      <CapturePanel />
                    ) : panel === "camera" ? (
                      <CameraPanel />
                    ) : panel === "controls" ? (
                      <ControlsPanel />
                    ) : panel === "notifications" ? (
                      <NotificationsPanel />
                    ) : (
                      <ToolsPanel
                        section={
                          panel === "clipboard" ||
                          panel === "timer" ||
                          panel === "notes" ||
                          panel === "launcher"
                            ? panel
                            : "clipboard"
                        }
                        onNavigate={setPanel}
                      />
                    )}
                  </motion.div>
                </AnimatePresence>
              </div>
            )}
          </motion.div>

          {error && (
            <div role="alert" className="error" title={error}>
              Window update failed. Open Peek again to retry.
            </div>
          )}
        </motion.main>

        <svg
          className="notch-ear right"
          width="10"
          height="10"
          viewBox="0 0 10 10"
          aria-hidden="true"
        >
          <path d="M 0 0 L 10 0 L 10 10 L 8.2 10 A 8.2 8.2 0 0 0 0 1.8 Z" fill="currentColor" />
        </svg>
      </div>
    </div>
  );
}
