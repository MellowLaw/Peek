import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Bell,
  Bot,
  Camera,
  Clipboard,
  Code2,
  Copy,
  Crop,
  Files,
  Music2,
  PictureInPicture2,
  Pin,
  Plus,
  RefreshCw,
  ScanLine,
  ScanText,
  Search,
  Settings2,
  SlidersHorizontal,
  StickyNote,
  Timer,
  Trash2,
} from "lucide-react";
import {
  SolidPauseIcon,
  SolidPlayIcon,
  SolidRepeatIcon,
  SolidStopIcon,
} from "../../components/SolidIcons";
import { usePreferences } from "../../stores/preferences";
import type { PanelId } from "../../types/peek";
import { NumberStepper } from "../../components/NumberStepper";
import { SettingSwitch } from "../settings/SettingsPanel";

type ClipboardItem = {
  id: string;
  kind?: "text" | "image";
  text: string;
  imageThumbnail?: string | null;
  width?: number | null;
  height?: number | null;
  pinned: boolean;
  copiedAtMs: number;
};
type NoteItem = { id: string; title: string; body: string; updatedAtMs: number };
type SnippetItem = { id: string; title: string; content: string };
type NotesStoreData = { notes: NoteItem[]; snippets: SnippetItem[] };
type ReminderItem = { id: string; title: string; dueMs: number };
type TimerSnapshot = {
  running: boolean;
  paused: boolean;
  label: string;
  totalSeconds: number;
  remainingSeconds: number;
  reminders: ReminderItem[];
};
type InstalledApp = { name: string; path: string };

function formatMmSs(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function appMonogram(name: string): string {
  const words = name.trim().split(/[\s\-_]+/).filter(Boolean);
  if (words.length >= 2) {
    return (words[0][0] + words[1][0]).toUpperCase();
  }
  return name.trim().slice(0, 2).toUpperCase() || "AP";
}

function relativeShort(ms: number): string {
  const diff = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (diff < 60) return "now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86400)}d`;
}

export function ToolsPanel({
  section,
  onNavigate,
}: {
  section: "clipboard" | "timer" | "notes" | "launcher";
  onNavigate: (panel: PanelId) => void;
}) {
  const { preferences, save } = usePreferences();
  const subtab = section;

  // Clipboard state
  const [clipItems, setClipItems] = useState<ClipboardItem[]>([]);
  const [clipFilter, setClipFilter] = useState("");

  // Timer & Reminders state
  const [timer, setTimer] = useState<TimerSnapshot>({
    running: false,
    paused: false,
    label: "Focus",
    totalSeconds: 1500,
    remainingSeconds: 1500,
    reminders: [],
  });
  const [customMins, setCustomMins] = useState(25);
  const [timerLabel, setTimerLabel] = useState("Focus");
  const [remTitle, setRemTitle] = useState("");
  const [remMins, setRemMins] = useState(15);
  const [localClock, setLocalClock] = useState(() =>
    new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }),
  );

  // Notes & Snippets state
  const [notesData, setNotesData] = useState<NotesStoreData>({ notes: [], snippets: [] });
  const [noteTitle, setNoteTitle] = useState("");
  const [noteBody, setNoteBody] = useState("");
  const [snipTitle, setSnipTitle] = useState("");
  const [snipContent, setSnipContent] = useState("");
  const [noteMode, setNoteMode] = useState<"notes" | "snippets">("notes");

  // Launcher state
  const [apps, setApps] = useState<InstalledApp[]>([]);
  const [appIcons, setAppIcons] = useState<Record<string, string>>({});
  const requestedIconsRef = useRef<Set<string>>(new Set());
  const [launchQuery, setLaunchQuery] = useState("");
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");

  const flash = (msg: string) => {
    setNotice(msg);
    setTimeout(() => setNotice(""), 2000);
  };

  const loadLauncher = useCallback(async () => {
    requestedIconsRef.current.clear();
    const list = await invoke<InstalledApp[]>("scan_installed_apps").catch(() => []);
    setApps(list);
  }, []);

  useEffect(() => {
    void invoke<ClipboardItem[]>("get_clipboard_history")
      .then(setClipItems)
      .catch(() => {});
    void invoke<TimerSnapshot>("get_timer_state")
      .then(setTimer)
      .catch(() => {});
    void invoke<NotesStoreData>("get_notes_and_snippets")
      .then(setNotesData)
      .catch(() => {});
    void loadLauncher();

    const subClip = listen<ClipboardItem[]>("clipboard-updated", ({ payload }) =>
      setClipItems(payload),
    );
    const subTimer = listen<TimerSnapshot>("tools-timer", ({ payload }) => setTimer(payload));
    const subNotes = listen<NotesStoreData>("notes-updated", ({ payload }) =>
      setNotesData(payload),
    );
    return () => {
      void subClip.then((u) => u()).catch(() => {});
      void subTimer.then((u) => u()).catch(() => {});
      void subNotes.then((u) => u()).catch(() => {});
    };
  }, [loadLauncher]);

  useEffect(() => {
    if (subtab !== "timer") return;
    const tick = () =>
      setLocalClock(new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }));
    tick();
    const id = setInterval(tick, 15000);
    return () => clearInterval(id);
  }, [subtab]);

  const filteredClip = useMemo(() => {
    const q = clipFilter.trim().toLowerCase();
    if (!q) return clipItems;
    return clipItems.filter((c) => {
      if (c.kind === "image") {
        const dims = c.width && c.height ? `${c.width}x${c.height}` : "";
        return "image".includes(q) || "screenshot".includes(q) || dims.includes(q);
      }
      return c.text.toLowerCase().includes(q);
    });
  }, [clipItems, clipFilter]);

  const peekActions = useMemo(
    () => [
      { id: "music", title: "Music", subtitle: "Panel", icon: Music2, run: () => onNavigate("music") },
      { id: "ai", title: "AI Usage", subtitle: "Panel", icon: Bot, run: () => onNavigate("ai") },
      { id: "files", title: "Files", subtitle: "Shelf", icon: Files, run: () => onNavigate("files") },
      {
        id: "region",
        title: "Region",
        subtitle: "Capture",
        icon: Crop,
        run: () =>
          void invoke("begin_screen_overlay", {
            mode: "region",
            addToShelf: true,
            includeMic: false,
            micDevice: "",
          }),
      },
      {
        id: "ocr",
        title: "OCR Text",
        subtitle: "Capture",
        icon: ScanText,
        run: () =>
          void invoke("begin_screen_overlay", {
            mode: "ocr",
            addToShelf: false,
            includeMic: false,
            micDevice: "",
          }),
      },
      {
        id: "controls",
        title: "Controls",
        subtitle: "Panel",
        icon: SlidersHorizontal,
        run: () => onNavigate("controls"),
      },
      { id: "capture", title: "Capture", subtitle: "Panel", icon: ScanLine, run: () => onNavigate("capture") },
      { id: "camera", title: "Toy Cam", subtitle: "Panel", icon: Camera, run: () => onNavigate("camera") },
      { id: "clipboard", title: "Clipboard", subtitle: "Panel", icon: Clipboard, run: () => onNavigate("clipboard") },
      { id: "timer", title: "Timer", subtitle: "Panel", icon: Timer, run: () => onNavigate("timer") },
      { id: "notes", title: "Notes", subtitle: "Panel", icon: StickyNote, run: () => onNavigate("notes") },
      { id: "alerts", title: "Alerts", subtitle: "Panel", icon: Bell, run: () => onNavigate("notifications") },
      { id: "settings", title: "Settings", subtitle: "Panel", icon: Settings2, run: () => onNavigate("settings") },
    ],
    [onNavigate],
  );

  const filteredActions = useMemo(() => {
    const q = launchQuery.trim().toLowerCase();
    return q
      ? peekActions.filter(
          (a) => a.title.toLowerCase().includes(q) || a.subtitle.toLowerCase().includes(q),
        )
      : peekActions.slice(0, 6);
  }, [peekActions, launchQuery]);

  const filteredApps = useMemo(() => {
    const q = launchQuery.trim().toLowerCase();
    const list = q ? apps.filter((a) => a.name.toLowerCase().includes(q)) : apps;
    return list.slice(0, 24);
  }, [apps, launchQuery]);

  useEffect(() => {
    if (subtab !== "launcher" || filteredApps.length === 0) return;
    const missing = filteredApps
      .map((a) => a.path)
      .filter((p) => !requestedIconsRef.current.has(p));
    if (missing.length === 0) return;
    for (const p of missing) {
      requestedIconsRef.current.add(p);
    }
    void invoke<Record<string, string>>("get_app_icons", { paths: missing })
      .then((res) => {
        if (Object.keys(res).length > 0) {
          setAppIcons((prev) => ({ ...prev, ...res }));
        }
      })
      .catch(() => {});
  }, [subtab, filteredApps]);

  return (
    <section aria-label="Productivity tools" className="tools-panel">

      {subtab === "clipboard" && (
        <div className="tools-stack">
          <SettingSwitch
            label="Track clipboard text & images (opt-in)"
            checked={preferences.clipboardHistoryEnabled}
            disabled={false}
            onChange={(checked) => void save({ clipboardHistoryEnabled: checked })}
          />
          <div className="tools-row">
            <input
              type="text"
              placeholder="Search clipboard…"
              value={clipFilter}
              onChange={(e) => setClipFilter(e.target.value)}
              style={{ flex: 1, minWidth: 0 }}
            />
            {clipItems.length > 0 && (
              <button
                type="button"
                className="btn sm"
                onClick={() =>
                  void invoke<ClipboardItem[]>("clear_clipboard_history").then(setClipItems)
                }
              >
                Clear unpinned
              </button>
            )}
          </div>
          {filteredClip.length === 0 ? (
            <div className="empty-panel">
              <Clipboard size={26} strokeWidth={1.5} />
              <span>
                {preferences.clipboardHistoryEnabled
                  ? "Copied text and images appear here"
                  : "Enable clipboard history above"}
              </span>
            </div>
          ) : (
            <div className="clip-grid">
              {filteredClip.map((item) => {
                const isImg = item.kind === "image";
                return (
                  <div
                    key={item.id}
                    className={
                      "clip-card" +
                      (isImg ? " is-image" : " is-text") +
                      (item.pinned ? " is-pinned" : "")
                    }
                    title={isImg ? "Click to copy image" : item.text}
                    onClick={(e) => {
                      if ((e.target as HTMLElement).closest("button")) return;
                      void invoke("copy_clipboard_item", { id: item.id })
                        .then(() => flash("Copied"))
                        .catch((err) => setError(String(err)));
                    }}
                  >
                    {item.pinned && (
                      <span className="clip-pin-badge" aria-label="Pinned">
                        <Pin size={10} />
                      </span>
                    )}

                    {isImg ? (
                      item.imageThumbnail ? (
                        <img
                          className="clip-card-img"
                          src={item.imageThumbnail}
                          alt="Copied image preview"
                          draggable={false}
                        />
                      ) : (
                        <div className="clip-card-img-fallback">Image</div>
                      )
                    ) : (
                      <span className="clip-card-text">{item.text}</span>
                    )}

                    <div className="clip-card-overlay">
                      <span className="clip-card-meta">
                        {isImg
                          ? item.width && item.height
                            ? `${item.width} × ${item.height}`
                            : "Image"
                          : ""}
                      </span>
                      <div className="clip-card-actions">
                        <button
                          type="button"
                          className={"shelf-act-btn" + (item.pinned ? " primary" : "")}
                          title={item.pinned ? "Unpin" : "Pin"}
                          aria-label={item.pinned ? "Unpin" : "Pin"}
                          onClick={() =>
                            void invoke<ClipboardItem[]>("toggle_pin_clipboard_item", {
                              id: item.id,
                            }).then(setClipItems)
                          }
                        >
                          <Pin size={11} />
                        </button>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Copy"
                          aria-label="Copy"
                          onClick={() =>
                            void invoke("copy_clipboard_item", { id: item.id })
                              .then(() => flash("Copied"))
                              .catch((err) => setError(String(err)))
                          }
                        >
                          <Copy size={11} />
                        </button>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Delete"
                          aria-label="Delete"
                          onClick={() =>
                            void invoke<ClipboardItem[]>("delete_clipboard_item", {
                              id: item.id,
                            }).then(setClipItems)
                          }
                        >
                          <Trash2 size={11} />
                        </button>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}

      {subtab === "timer" && (() => {
        const totalSec = Math.max(1, timer.totalSeconds || customMins * 60);
        const pctRatio = Math.max(0, Math.min(1, timer.remainingSeconds / totalSec));
        const r = 44;
        const circ = 2 * Math.PI * r;
        const offset = circ * (1 - pctRatio);

        return (
          <div className="timer-bento">
            <div className="timer-top-grid">
              {/* Left Hero: Circular Dial Readout */}
              <div className="bento-tile timer-dial-tile">
                <div className="bento-tile-head">
                  <span className="bento-label">Timer</span>
                  <span className="timer-clock-readout">{localClock}</span>
                </div>

                <div className="timer-ring-wrap">
                  <svg viewBox="0 0 112 112" width="112" height="112" aria-hidden="true">
                    {Array.from({ length: 24 }, (_, idx) => (
                      <rect
                        key={idx}
                        x="55.2"
                        y="2.5"
                        width="1.6"
                        height="4.5"
                        rx="0.8"
                        fill="#262626"
                        transform={`rotate(${idx * 15} 56 56)`}
                      />
                    ))}
                    <circle
                      cx="56"
                      cy="56"
                      r={r}
                      fill="none"
                      stroke="#222222"
                      strokeWidth="4.5"
                    />
                    <circle
                      cx="56"
                      cy="56"
                      r={r}
                      fill="none"
                      stroke={timer.paused ? "#8f8f8f" : "#f4f4f4"}
                      strokeWidth="4.5"
                      strokeLinecap="round"
                      strokeDasharray={circ}
                      strokeDashoffset={offset}
                      transform="rotate(-90 56 56)"
                      style={{ transition: "stroke-dashoffset 280ms ease" }}
                    />
                  </svg>
                  <div className="timer-ring-center">
                    <strong>{formatMmSs(timer.remainingSeconds)}</strong>
                    <span>{timer.paused ? "Paused" : timer.label}</span>
                  </div>
                </div>

                <div className="timer-hero-actions">
                  {!timer.running ? (
                    <button
                      type="button"
                      className="btn primary sm"
                      onClick={() =>
                        void invoke<TimerSnapshot>("start_timer", {
                          label: timerLabel,
                          durationSeconds: customMins * 60,
                        }).then(setTimer)
                      }
                    >
                      <SolidPlayIcon size={12} /> Start
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        className="btn primary sm"
                        aria-label={timer.paused ? "Resume timer" : "Pause timer"}
                        onClick={() =>
                          void invoke<TimerSnapshot>(
                            timer.paused ? "resume_timer" : "pause_timer",
                          ).then(setTimer)
                        }
                      >
                        {timer.paused ? <SolidPlayIcon size={12} /> : <SolidPauseIcon size={12} />}
                        <span>{timer.paused ? "Resume" : "Pause"}</span>
                      </button>
                      <button
                        type="button"
                        className="btn sm"
                        aria-label="Stop timer"
                        title="Stop"
                        onClick={() => void invoke<TimerSnapshot>("reset_timer").then(setTimer)}
                      >
                        <SolidStopIcon size={12} />
                      </button>
                      <button
                        type="button"
                        className="btn sm"
                        aria-label="Restart timer"
                        title="Restart"
                        onClick={() => void invoke<TimerSnapshot>("restart_timer").then(setTimer)}
                      >
                        <SolidRepeatIcon size={12} />
                      </button>
                    </>
                  )}
                </div>
              </div>

              {/* Right Stack: Presets + Custom */}
              <div className="timer-right-stack">
                <div className="timer-presets-grid">
                  {(
                    [
                      ["Focus", 25 * 60],
                      ["Break", 5 * 60],
                      ["Long", 15 * 60],
                      ["Sprint", 45 * 60],
                    ] as const
                  ).map(([lbl, sec]) => (
                    <button
                      key={lbl}
                      type="button"
                      className={
                        "timer-preset-tile" +
                        (timer.running && timer.label === lbl && timer.totalSeconds === sec
                          ? " is-active"
                          : "")
                      }
                      onClick={() => {
                        setTimerLabel(lbl);
                        setCustomMins(sec / 60);
                        void invoke<TimerSnapshot>("start_timer", {
                          label: lbl,
                          durationSeconds: sec,
                        }).then(setTimer);
                      }}
                    >
                      <strong>{lbl}</strong>
                      <span>{sec / 60}m</span>
                    </button>
                  ))}
                </div>

                <div className="bento-tile timer-custom-tile">
                  <span className="bento-label">Custom</span>
                  <input
                    type="text"
                    aria-label="Timer label"
                    value={timerLabel}
                    onChange={(e) => setTimerLabel(e.target.value)}
                    placeholder="Label"
                  />
                  <div className="timer-custom-foot">
                    <NumberStepper
                      ariaLabel="Timer minutes"
                      min={1}
                      max={240}
                      suffix="m"
                      value={customMins}
                      onChange={setCustomMins}
                    />
                    <button
                      type="button"
                      className="btn sm primary"
                      onClick={() =>
                        void invoke<TimerSnapshot>("start_timer", {
                          label: timerLabel,
                          durationSeconds: customMins * 60,
                        }).then(setTimer)
                      }
                    >
                      <SolidPlayIcon size={11} /> Go
                    </button>
                  </div>
                </div>
              </div>
            </div>

            {/* Bottom Reminders Bento */}
            <div className="bento-tile timer-reminders-tile">
              <div className="bento-tile-head">
                <span className="bento-label">Reminders</span>
                {timer.reminders.length > 0 && (
                  <span className="timer-count-pill">{timer.reminders.length}</span>
                )}
              </div>
              <div className="tools-row">
                <input
                  type="text"
                  placeholder="Remind me to…"
                  value={remTitle}
                  onChange={(e) => setRemTitle(e.target.value)}
                  style={{ flex: 1, minWidth: 0 }}
                />
                <NumberStepper
                  ariaLabel="Minutes until reminder"
                  min={1}
                  max={1440}
                  suffix="m"
                  value={remMins}
                  onChange={setRemMins}
                />
                <button
                  type="button"
                  className="btn primary sm"
                  onClick={() =>
                    void invoke<TimerSnapshot>("add_reminder", {
                      title: remTitle,
                      minutes: remMins,
                    })
                      .then((s) => {
                        setTimer(s);
                        setRemTitle("");
                      })
                      .catch((e) => setError(String(e)))
                  }
                >
                  <Plus size={12} /> Add
                </button>
              </div>
              {timer.reminders.length > 0 && (
                <div className="reminder-list">
                  {timer.reminders.map((r) => (
                    <div key={r.id} className="reminder-row">
                      <span className="reminder-title truncate">{r.title}</span>
                      <span className="reminder-due">
                        {new Date(r.dueMs).toLocaleTimeString([], {
                          hour: "2-digit",
                          minute: "2-digit",
                        })}
                      </span>
                      <button
                        type="button"
                        className="notif-dismiss-btn"
                        aria-label="Remove reminder"
                        onClick={() =>
                          void invoke<TimerSnapshot>("remove_reminder", { id: r.id }).then(setTimer)
                        }
                      >
                        <Trash2 size={11} />
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        );
      })()}

      {subtab === "notes" && (
        <div className="notes-bento">
          <div className="notes-bento-head">
            <div className="drag-mode-pill" role="tablist" aria-label="Notes and snippets">
              <button
                type="button"
                role="tab"
                aria-selected={noteMode === "notes"}
                className={"drag-mode-opt" + (noteMode === "notes" ? " active" : "")}
                onClick={() => setNoteMode("notes")}
              >
                Notes ({notesData.notes.length})
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={noteMode === "snippets"}
                className={"drag-mode-opt" + (noteMode === "snippets" ? " active" : "")}
                onClick={() => setNoteMode("snippets")}
              >
                Snippets ({notesData.snippets.length})
              </button>
            </div>

            <button
              type="button"
              className="btn sm"
              title="Open a floating sticky note window"
              onClick={() =>
                void invoke("open_floating_note", { id: null }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              <PictureInPicture2 size={12} />
              <span>Float note</span>
            </button>
          </div>

          {noteMode === "notes" ? (
            <>
              <div className="bento-tile notes-composer-tile">
                <input
                  type="text"
                  className="notes-composer-title"
                  aria-label="Note title"
                  placeholder="Title"
                  value={noteTitle}
                  onChange={(e) => setNoteTitle(e.target.value)}
                />
                <textarea
                  className="notes-composer-body"
                  rows={3}
                  aria-label="Note content"
                  placeholder="Write a quick note…"
                  value={noteBody}
                  onChange={(e) => setNoteBody(e.target.value)}
                  onKeyDown={(e) => {
                    if (
                      (e.ctrlKey || e.metaKey) &&
                      e.key === "Enter" &&
                      (noteTitle.trim() || noteBody.trim())
                    ) {
                      e.preventDefault();
                      void invoke<NotesStoreData>("save_note", {
                        id: null,
                        title: noteTitle,
                        body: noteBody,
                      }).then((d) => {
                        setNotesData(d);
                        setNoteTitle("");
                        setNoteBody("");
                      });
                    }
                  }}
                />
                <div className="notes-composer-foot">
                  <span className="notes-hint">Ctrl+Enter to save</span>
                  <button
                    type="button"
                    className="btn primary sm"
                    disabled={!noteTitle.trim() && !noteBody.trim()}
                    onClick={() =>
                      void invoke<NotesStoreData>("save_note", {
                        id: null,
                        title: noteTitle,
                        body: noteBody,
                      }).then((d) => {
                        setNotesData(d);
                        setNoteTitle("");
                        setNoteBody("");
                      })
                    }
                  >
                    <StickyNote size={12} />
                    <span>Save note</span>
                  </button>
                </div>
              </div>

              {notesData.notes.length === 0 ? (
                <div className="empty-panel notes-empty-bento">
                  <StickyNote size={22} strokeWidth={1.5} />
                  <span>No saved notes</span>
                </div>
              ) : (
                <div className="notes-cards-grid">
                  {notesData.notes.map((n) => (
                    <div key={n.id} className="note-bento-card">
                      <div className="note-card-head">
                        <strong title={n.title}>{n.title}</strong>
                        <span className="note-card-time">{relativeShort(n.updatedAtMs)}</span>
                      </div>
                      {n.body && <p className="note-card-body">{n.body}</p>}
                      <div className="note-card-actions">
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Float sticky note"
                          aria-label="Float note"
                          onClick={() =>
                            void invoke("open_floating_note", { id: n.id }).catch((e) =>
                              setError(String(e)),
                            )
                          }
                        >
                          <PictureInPicture2 size={11} />
                        </button>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Copy note"
                          aria-label="Copy note"
                          onClick={() =>
                            void invoke("copy_tool_text", { text: n.body || n.title }).then(() =>
                              flash("Copied"),
                            )
                          }
                        >
                          <Copy size={11} />
                        </button>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Delete note"
                          aria-label="Delete note"
                          onClick={() =>
                            void invoke<NotesStoreData>("delete_note", { id: n.id }).then(
                              setNotesData,
                            )
                          }
                        >
                          <Trash2 size={11} />
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </>
          ) : (
            <>
              <div className="bento-tile notes-composer-tile">
                <input
                  type="text"
                  className="notes-composer-title"
                  aria-label="Snippet label"
                  placeholder="Snippet label"
                  value={snipTitle}
                  onChange={(e) => setSnipTitle(e.target.value)}
                />
                <textarea
                  className="notes-composer-body is-mono"
                  rows={3}
                  aria-label="Snippet content"
                  placeholder="Reusable text or command…"
                  value={snipContent}
                  onChange={(e) => setSnipContent(e.target.value)}
                  onKeyDown={(e) => {
                    if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && snipContent.trim()) {
                      e.preventDefault();
                      void invoke<NotesStoreData>("save_snippet", {
                        id: null,
                        title: snipTitle,
                        content: snipContent,
                      }).then((d) => {
                        setNotesData(d);
                        setSnipTitle("");
                        setSnipContent("");
                      });
                    }
                  }}
                />
                <div className="notes-composer-foot">
                  <span className="notes-hint">Ctrl+Enter to add</span>
                  <button
                    type="button"
                    className="btn primary sm"
                    disabled={!snipContent.trim()}
                    onClick={() =>
                      void invoke<NotesStoreData>("save_snippet", {
                        id: null,
                        title: snipTitle,
                        content: snipContent,
                      }).then((d) => {
                        setNotesData(d);
                        setSnipTitle("");
                        setSnipContent("");
                      })
                    }
                  >
                    <Plus size={12} />
                    <span>Add snippet</span>
                  </button>
                </div>
              </div>

              {notesData.snippets.length === 0 ? (
                <div className="empty-panel notes-empty-bento">
                  <Code2 size={22} strokeWidth={1.5} />
                  <span>No saved snippets</span>
                </div>
              ) : (
                <div className="notes-cards-grid">
                  {notesData.snippets.map((s) => (
                    <div
                      key={s.id}
                      className="note-bento-card is-snippet"
                      title="Click to copy snippet"
                      onClick={(e) => {
                        if ((e.target as HTMLElement).closest("button")) return;
                        void invoke("copy_tool_text", { text: s.content }).then(() =>
                          flash("Copied"),
                        );
                      }}
                    >
                      <div className="note-card-head">
                        <strong title={s.title}>{s.title || "Snippet"}</strong>
                      </div>
                      <p className="note-card-body is-mono">{s.content}</p>
                      <div className="note-card-actions">
                        <button
                          type="button"
                          className="shelf-act-btn primary"
                          title="Copy snippet"
                          aria-label="Copy snippet"
                          onClick={() =>
                            void invoke("copy_tool_text", { text: s.content }).then(() =>
                              flash("Copied"),
                            )
                          }
                        >
                          <Copy size={11} />
                        </button>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Delete snippet"
                          aria-label="Delete snippet"
                          onClick={() =>
                            void invoke<NotesStoreData>("delete_snippet", { id: s.id }).then(
                              setNotesData,
                            )
                          }
                        >
                          <Trash2 size={11} />
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}

      {subtab === "launcher" && (
        <div className="tools-stack">
          <div className="launcher-search-bar">
            <Search size={13} className="launcher-search-ico" />
            <input
              type="text"
              placeholder="Search apps & Peek actions (Ctrl+K)…"
              value={launchQuery}
              onChange={(e) => setLaunchQuery(e.target.value)}
              autoFocus
            />
            <span className="launcher-count-pill">{apps.length}</span>
            <button
              type="button"
              className="btn sm"
              aria-label="Refresh apps"
              title="Refresh installed apps"
              onClick={() => void loadLauncher()}
            >
              <RefreshCw size={12} />
            </button>
          </div>

          {filteredActions.length > 0 && (
            <div className="launcher-section">
              <div className="launcher-section-head">
                <span className="bento-label">Peek Actions</span>
              </div>
              <div className="launcher-actions-grid">
                {filteredActions.map((act) => {
                  const Icon = act.icon;
                  return (
                    <button
                      key={act.id}
                      type="button"
                      className="launcher-action-tile"
                      onClick={act.run}
                    >
                      <Icon size={15} strokeWidth={1.85} />
                      <span className="launcher-action-copy">
                        <strong>{act.title}</strong>
                        <span>{act.subtitle}</span>
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          <div className="launcher-section">
            <div className="launcher-section-head">
              <span className="bento-label">Apps</span>
            </div>
            {filteredApps.length === 0 ? (
              <div className="empty-panel">
                <span>No matching apps</span>
              </div>
            ) : (
              <div className="launcher-apps-grid">
                {filteredApps.map((app) => {
                  const iconUrl = appIcons[app.path];
                  return (
                    <button
                      key={app.path}
                      type="button"
                      className="launcher-app-tile"
                      title={app.name}
                      onClick={() =>
                        void invoke("launch_app", { path: app.path }).catch((e) =>
                          setError(String(e)),
                        )
                      }
                    >
                      {iconUrl ? (
                        <img
                          src={iconUrl}
                          alt=""
                          className="launcher-app-icon"
                          draggable={false}
                        />
                      ) : (
                        <span className="launcher-app-monogram" aria-hidden="true">
                          {appMonogram(app.name)}
                        </span>
                      )}
                      <span className="launcher-app-name">{app.name}</span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      )}

      {notice && (
        <div className="status-note" role="status">
          {notice}
        </div>
      )}
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
    </section>
  );
}
