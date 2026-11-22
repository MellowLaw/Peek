import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Copy, Pin, X } from "lucide-react";

type NoteItem = { id: string; title: string; body: string; updatedAtMs: number };
type NotesStoreData = { notes: NoteItem[]; snippets: unknown[] };

function parseNoteIdFromHash(): string {
  const hash = window.location.hash;
  const qIdx = hash.indexOf("?");
  if (qIdx === -1) return "";
  return new URLSearchParams(hash.slice(qIdx + 1)).get("id") ?? "";
}

export function FloatingNote() {
  const noteId = useRef(parseNoteIdFromHash()).current;
  const [title, setTitle] = useState("Quick Note");
  const [body, setBody] = useState("");
  const [pinnedTop, setPinnedTop] = useState(true);
  const [status, setStatus] = useState("");
  const saveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const dirtyRef = useRef(false);
  const latestRef = useRef({ title: "Quick Note", body: "" });
  latestRef.current = { title, body };

  const flushSave = useCallback(async () => {
    if (!noteId) return;
    clearTimeout(saveTimer.current);
    dirtyRef.current = false;
    await invoke("save_note", {
      id: noteId,
      title: latestRef.current.title,
      body: latestRef.current.body,
    }).catch(() => {});
    setStatus("Saved");
    setTimeout(() => setStatus(""), 1400);
  }, [noteId]);

  useEffect(() => {
    void invoke<NotesStoreData>("get_notes_and_snippets")
      .then((data) => {
        const found = data.notes.find((n) => n.id === noteId);
        if (found) {
          setTitle(found.title);
          setBody(found.body);
        }
      })
      .catch(() => {});

    const sub = listen<NotesStoreData>("notes-updated", ({ payload }) => {
      if (dirtyRef.current) return;
      const found = payload.notes.find((n) => n.id === noteId);
      if (found) {
        setTitle(found.title);
        setBody(found.body);
      }
    });

    return () => {
      clearTimeout(saveTimer.current);
      void sub.then((u) => u()).catch(() => {});
    };
  }, [noteId]);

  function scheduleSave(nextTitle: string, nextBody: string) {
    latestRef.current = { title: nextTitle, body: nextBody };
    dirtyRef.current = true;
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      void flushSave();
    }, 380);
  }

  async function togglePinTop() {
    const next = !pinnedTop;
    setPinnedTop(next);
    await invoke("set_floating_always_on_top", { alwaysOnTop: next }).catch(() => {});
  }

  async function handleClose() {
    if (dirtyRef.current) {
      await flushSave();
    }
    await invoke("close_floating_window").catch(() => window.close());
  }

  function handleHeaderMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button, input")) return;
    void invoke("start_floating_drag");
  }

  return (
    <div className="float-win float-note">
      <header className="float-note-header" onMouseDown={handleHeaderMouseDown}>
        <input
          type="text"
          className="float-note-title"
          aria-label="Note title"
          value={title}
          onChange={(e) => {
            const v = e.target.value;
            setTitle(v);
            scheduleSave(v, body);
          }}
          placeholder="Note title"
        />
        <div className="float-note-actions">
          {status && <span className="float-note-status">{status}</span>}
          <button
            type="button"
            className={"float-bar-btn" + (pinnedTop ? " active" : "")}
            title={pinnedTop ? "Pinned on top" : "Pin on top"}
            aria-label="Toggle always on top"
            onClick={() => void togglePinTop()}
          >
            <Pin size={11} />
          </button>
          <button
            type="button"
            className="float-bar-btn"
            title="Copy note text"
            aria-label="Copy note text"
            onClick={() =>
              void invoke("copy_tool_text", { text: body || title }).then(() => {
                setStatus("Copied");
                setTimeout(() => setStatus(""), 1400);
              })
            }
          >
            <Copy size={11} />
          </button>
          <button
            type="button"
            className="float-close-btn"
            title="Close note"
            aria-label="Close floating note"
            onClick={() => void handleClose()}
          >
            <X size={13} />
          </button>
        </div>
      </header>
      <textarea
        className="float-note-body"
        aria-label="Floating note text"
        placeholder="Jot anything down…"
        value={body}
        onChange={(e) => {
          const v = e.target.value;
          setBody(v);
          scheduleSave(title, v);
        }}
        autoFocus
      />
    </div>
  );
}
