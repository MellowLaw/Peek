import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, Copy, ListChecks, Pin, Plus, Trash2, X } from "lucide-react";

type ChecklistItem = { id: string; text: string; done: boolean };
type NoteItem = {
  id: string;
  title: string;
  body: string;
  kind?: "note" | "checklist";
  items?: ChecklistItem[];
  updatedAtMs: number;
};
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
  const [kind, setKind] = useState<"note" | "checklist">("note");
  const [items, setItems] = useState<ChecklistItem[]>([]);
  const [pinnedTop, setPinnedTop] = useState(true);
  const [status, setStatus] = useState("");
  const saveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const dirtyRef = useRef(false);
  const latestRef = useRef<{
    title: string;
    body: string;
    kind: "note" | "checklist";
    items: ChecklistItem[];
  }>({ title: "Quick Note", body: "", kind: "note", items: [] });
  latestRef.current = { title, body, kind, items };

  const flushSave = useCallback(async () => {
    if (!noteId) return;
    clearTimeout(saveTimer.current);
    dirtyRef.current = false;
    await invoke("save_note", {
      id: noteId,
      title: latestRef.current.title,
      body: latestRef.current.body,
      kind: latestRef.current.kind,
      items: latestRef.current.items,
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
          setKind(found.kind ?? "note");
          setItems(found.items ?? []);
        }
      })
      .catch(() => {});

    const sub = listen<NotesStoreData>("notes-updated", ({ payload }) => {
      if (dirtyRef.current) return;
      const found = payload.notes.find((n) => n.id === noteId);
      if (found) {
        setTitle(found.title);
        setBody(found.body);
        setKind(found.kind ?? "note");
        setItems(found.items ?? []);
      }
    });

    return () => {
      clearTimeout(saveTimer.current);
      void sub.then((u) => u()).catch(() => {});
    };
  }, [noteId]);

  function scheduleSave(
    nextTitle: string,
    nextBody: string,
    nextKind: "note" | "checklist" = kind,
    nextItems: ChecklistItem[] = items,
  ) {
    latestRef.current = { title: nextTitle, body: nextBody, kind: nextKind, items: nextItems };
    dirtyRef.current = true;
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      void flushSave();
    }, 320);
  }

  function switchKind() {
    if (kind === "note") {
      const converted: ChecklistItem[] =
        items.length > 0
          ? items
          : body
              .split(/\r?\n/)
              .map((line) => line.trim())
              .filter(Boolean)
              .map((text, idx) => ({ id: `chk-${Date.now()}-${idx}`, text, done: false }));
      const nextItems =
        converted.length > 0 ? converted : [{ id: `chk-${Date.now()}-0`, text: "", done: false }];
      setKind("checklist");
      setItems(nextItems);
      scheduleSave(title, body, "checklist", nextItems);
    } else {
      const nextBody =
        items.length > 0
          ? items
              .map((i) => i.text.trim())
              .filter(Boolean)
              .join("\n")
          : body;
      setKind("note");
      setBody(nextBody);
      scheduleSave(title, nextBody, "note", items);
    }
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

  const copyContent =
    kind === "checklist" && items.length > 0
      ? items
          .filter((i) => i.text.trim())
          .map((i) => `${i.done ? "[x]" : "[ ]"} ${i.text}`)
          .join("\n") || title
      : body || title;

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
            scheduleSave(v, body, kind, items);
          }}
          placeholder="Note title"
        />
        <div className="float-note-actions">
          {status && <span className="float-note-status">{status}</span>}
          <button
            type="button"
            className={"float-bar-btn" + (kind === "checklist" ? " active" : "")}
            title={kind === "checklist" ? "Switch to plain note" : "Switch to checklist"}
            aria-label="Toggle checklist mode"
            onClick={switchKind}
          >
            <ListChecks size={11} />
          </button>
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
              void invoke("copy_tool_text", { text: copyContent }).then(() => {
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
      {kind === "checklist" ? (
        <div className="float-checklist-wrap">
          <div className="float-checklist-list">
            {items.map((item, idx) => (
              <div key={item.id} className={"check-item-row" + (item.done ? " is-done" : "")}>
                <button
                  type="button"
                  className={"check-box-btn" + (item.done ? " is-checked" : "")}
                  aria-label={item.done ? "Mark incomplete" : "Mark complete"}
                  aria-pressed={item.done}
                  onClick={() => {
                    const next = items.map((it) =>
                      it.id === item.id ? { ...it, done: !it.done } : it,
                    );
                    setItems(next);
                    scheduleSave(title, body, "checklist", next);
                  }}
                >
                  {item.done && <Check size={10} strokeWidth={2.6} />}
                </button>
                <input
                  type="text"
                  className="check-item-input"
                  value={item.text}
                  placeholder={`Item ${idx + 1}`}
                  onChange={(e) => {
                    const v = e.target.value;
                    const next = items.map((it) =>
                      it.id === item.id ? { ...it, text: v } : it,
                    );
                    setItems(next);
                    scheduleSave(title, body, "checklist", next);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      const created: ChecklistItem = {
                        id: `chk-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
                        text: "",
                        done: false,
                      };
                      const next = [...items.slice(0, idx + 1), created, ...items.slice(idx + 1)];
                      setItems(next);
                      scheduleSave(title, body, "checklist", next);
                    }
                  }}
                />
                <button
                  type="button"
                  className="check-remove-btn"
                  aria-label="Remove item"
                  onClick={() => {
                    const next = items.filter((it) => it.id !== item.id);
                    const normalized =
                      next.length > 0
                        ? next
                        : [{ id: `chk-${Date.now()}-0`, text: "", done: false }];
                    setItems(normalized);
                    scheduleSave(title, body, "checklist", normalized);
                  }}
                >
                  <Trash2 size={11} />
                </button>
              </div>
            ))}
          </div>
          <button
            type="button"
            className="check-add-btn"
            onClick={() => {
              const next = [
                ...items,
                {
                  id: `chk-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
                  text: "",
                  done: false,
                },
              ];
              setItems(next);
              scheduleSave(title, body, "checklist", next);
            }}
          >
            <Plus size={11} />
            <span>Add item</span>
          </button>
        </div>
      ) : (
        <textarea
          className="float-note-body"
          aria-label="Floating note text"
          placeholder="Jot anything down…"
          value={body}
          onChange={(e) => {
            const v = e.target.value;
            setBody(v);
            scheduleSave(title, v, "note", items);
          }}
          autoFocus
        />
      )}
    </div>
  );
}
