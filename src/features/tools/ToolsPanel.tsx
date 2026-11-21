import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Clipboard,
  Copy,
  Pin,
  Plus,
  RefreshCw,
  Rocket,
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

  // Notes & Snippets state
  const [notesData, setNotesData] = useState<NotesStoreData>({ notes: [], snippets: [] });
  const [noteTitle, setNoteTitle] = useState("");
  const [noteBody, setNoteBody] = useState("");
  const [snipTitle, setSnipTitle] = useState("");
  const [snipContent, setSnipContent] = useState("");
  const [noteMode, setNoteMode] = useState<"notes" | "snippets">("notes");

  // Launcher state
  const [apps, setApps] = useState<InstalledApp[]>([]);
  const [launchQuery, setLaunchQuery] = useState("");
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");

  const flash = (msg: string) => {
    setNotice(msg);
    setTimeout(() => setNotice(""), 2000);
  };

  const loadLauncher = useCallback(async () => {
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
    return () => {
      void subClip.then((u) => u()).catch(() => {});
      void subTimer.then((u) => u()).catch(() => {});
    };
  }, [loadLauncher]);

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

  const peekActions: { label: string; run: () => void }[] = useMemo(
    () => [
      { label: "Peek: Music", run: () => onNavigate("music") },
      { label: "Peek: AI Monitor", run: () => onNavigate("ai") },
      { label: "Peek: File Shelf", run: () => onNavigate("files") },
      { label: "Peek: Capture", run: () => onNavigate("capture") },
      {
        label: "Peek: Region Screenshot",
        run: () =>
          void invoke("begin_screen_overlay", {
            mode: "region",
            addToShelf: true,
            includeMic: false,
            micDevice: "",
          }),
      },
      {
        label: "Peek: OCR Screen Region",
        run: () =>
          void invoke("begin_screen_overlay", {
            mode: "ocr",
            addToShelf: false,
            includeMic: false,
            micDevice: "",
          }),
      },
      { label: "Peek: System Controls", run: () => onNavigate("controls") },
      { label: "Peek: Clipboard History", run: () => onNavigate("clipboard") },
      { label: "Peek: Focus Timer & Reminders", run: () => onNavigate("timer") },
      { label: "Peek: Notes & Snippets", run: () => onNavigate("notes") },
      { label: "Peek: Alerts", run: () => onNavigate("notifications") },
      { label: "Peek: Settings", run: () => onNavigate("settings") },
    ],
    [onNavigate],
  );

  const filteredActions = useMemo(() => {
    const q = launchQuery.trim().toLowerCase();
    return q ? peekActions.filter((a) => a.label.toLowerCase().includes(q)) : peekActions.slice(0, 4);
  }, [peekActions, launchQuery]);

  const filteredApps = useMemo(() => {
    const q = launchQuery.trim().toLowerCase();
    const list = q ? apps.filter((a) => a.name.toLowerCase().includes(q)) : apps;
    return list.slice(0, 24);
  }, [apps, launchQuery]);

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

      {subtab === "timer" && (
        <div className="tools-stack">
          <div className="ui-section timer-card">
            <div className="timer-display">
              <Timer size={18} />
              <strong>{formatMmSs(timer.remainingSeconds)}</strong>
              <span className="status-note">{timer.label}</span>
            </div>
            <div className="tools-row">
              {(
                [
                  ["Focus", 25 * 60],
                  ["Break", 5 * 60],
                  ["Long", 15 * 60],
                ] as const
              ).map(([lbl, sec]) => (
                <button
                  key={lbl}
                  type="button"
                  className="btn sm"
                  onClick={() =>
                    void invoke<TimerSnapshot>("start_timer", {
                      label: lbl,
                      durationSeconds: sec,
                    }).then(setTimer)
                  }
                >
                  {lbl} {sec / 60}m
                </button>
              ))}
            </div>
            <div className="tools-row">
              <input
                type="text"
                aria-label="Timer label"
                value={timerLabel}
                onChange={(e) => setTimerLabel(e.target.value)}
                style={{ flex: 1 }}
              />
              <NumberStepper
                ariaLabel="Timer minutes"
                min={1}
                max={240}
                suffix="m"
                value={customMins}
                onChange={setCustomMins}
              />
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
                    className="btn sm"
                    aria-label={timer.paused ? "Resume timer" : "Pause timer"}
                    title={timer.paused ? "Resume" : "Pause"}
                    onClick={() =>
                      void invoke<TimerSnapshot>(
                        timer.paused ? "resume_timer" : "pause_timer",
                      ).then(setTimer)
                    }
                  >
                    {timer.paused ? <SolidPlayIcon size={12} /> : <SolidPauseIcon size={12} />}
                    {timer.paused ? "Resume" : "Pause"}
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

          <div className="ui-section tools-stack">
            <span className="status-note">Reminders</span>
            <div className="tools-row">
              <input
                type="text"
                placeholder="Remind me to…"
                value={remTitle}
                onChange={(e) => setRemTitle(e.target.value)}
                style={{ flex: 1 }}
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
            {timer.reminders.map((r) => (
              <div key={r.id} className="tools-row">
                <span className="status-note" style={{ flex: 1 }}>
                  {r.title} · {new Date(r.dueMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                </span>
                <button
                  type="button"
                  className="btn sm"
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
        </div>
      )}

      {subtab === "notes" && (
        <div className="tools-stack">
          <div className="tools-row">
            <button
              type="button"
              className={"btn sm" + (noteMode === "notes" ? " primary" : "")}
              onClick={() => setNoteMode("notes")}
            >
              Notes ({notesData.notes.length})
            </button>
            <button
              type="button"
              className={"btn sm" + (noteMode === "snippets" ? " primary" : "")}
              onClick={() => setNoteMode("snippets")}
            >
              Snippets ({notesData.snippets.length})
            </button>
          </div>

          {noteMode === "notes" ? (
            <>
              <div className="ui-section tools-stack">
                <input
                  type="text"
                  placeholder="Title"
                  value={noteTitle}
                  onChange={(e) => setNoteTitle(e.target.value)}
                />
                <textarea
                  rows={2}
                  placeholder="Write a quick note…"
                  value={noteBody}
                  onChange={(e) => setNoteBody(e.target.value)}
                />
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
                  <StickyNote size={12} /> Save note
                </button>
              </div>
              {notesData.notes.map((n) => (
                <div key={n.id} className="ui-row clip-item">
                  <div className="note-text">
                    <strong>{n.title}</strong>
                    {n.body && <span className="status-note">{n.body}</span>}
                  </div>
                  <div className="clip-actions">
                    <button
                      type="button"
                      className="btn sm"
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
                      className="btn sm"
                      aria-label="Delete note"
                      onClick={() =>
                        void invoke<NotesStoreData>("delete_note", { id: n.id }).then(setNotesData)
                      }
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                </div>
              ))}
            </>
          ) : (
            <>
              <div className="ui-section tools-stack">
                <input
                  type="text"
                  placeholder="Snippet label"
                  value={snipTitle}
                  onChange={(e) => setSnipTitle(e.target.value)}
                />
                <textarea
                  rows={2}
                  placeholder="Reusable text or command…"
                  value={snipContent}
                  onChange={(e) => setSnipContent(e.target.value)}
                />
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
                  <Plus size={12} /> Add snippet
                </button>
              </div>
              {notesData.snippets.map((s) => (
                <div key={s.id} className="ui-row clip-item">
                  <div className="note-text">
                    <strong>{s.title}</strong>
                    <span className="status-note">{s.content}</span>
                  </div>
                  <div className="clip-actions">
                    <button
                      type="button"
                      className="btn sm primary"
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
                      className="btn sm"
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
            </>
          )}
        </div>
      )}

      {subtab === "launcher" && (
        <div className="tools-stack">
          <div className="tools-row">
            <input
              type="text"
              placeholder="Search apps & Peek commands (Ctrl+K)…"
              value={launchQuery}
              onChange={(e) => setLaunchQuery(e.target.value)}
              style={{ flex: 1 }}
              autoFocus
            />
            <button
              type="button"
              className="btn sm"
              aria-label="Refresh apps"
              onClick={() => void loadLauncher()}
            >
              <RefreshCw size={12} />
            </button>
          </div>
          <div className="launcher-list">
            {filteredActions.map((act) => (
              <button
                key={act.label}
                type="button"
                className="ui-row launch-row"
                onClick={act.run}
              >
                <span>{act.label}</span>
              </button>
            ))}
            {filteredApps.map((app) => (
              <button
                key={app.path}
                type="button"
                className="ui-row launch-row"
                onClick={() =>
                  void invoke("launch_app", { path: app.path }).catch((e) => setError(String(e)))
                }
              >
                <Rocket size={12} />
                <span className="truncate">{app.name}</span>
              </button>
            ))}
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
