import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Archive,
  Check,
  Copy,
  File as FileIcon,
  Folder,
  FolderInput,
  FolderOpen,
  GripVertical,
  Image as ImageIcon,
  Pencil,
  Plus,
  QrCode,
  Trash2,
  X,
} from "lucide-react";

type ShelfItem = {
  id: string;
  path: string;
  name: string;
  extension: string;
  sizeBytes: number;
  modifiedMs: number;
  isDir: boolean;
  exists: boolean;
  thumbnail: string | null;
};

type QrTransferInfo = {
  fileName: string;
  url: string;
  qrDataUrl: string;
  expiresInSeconds: number;
};

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "webp", "bmp", "gif"]);

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function FilesPanel({
  isDragOver = false,
  draggedPaths = [],
  onDraggingOutChange,
}: {
  isDragOver?: boolean;
  draggedPaths?: string[];
  onDraggingOutChange?: (dragging: boolean) => void;
}) {
  const [items, setItems] = useState<ShelfItem[]>([]);
  const [preferMove, setPreferMove] = useState(true);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameText, setRenameText] = useState("");
  const [convertingId, setConvertingId] = useState<string | null>(null);
  const [qrInfo, setQrInfo] = useState<QrTransferInfo | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const dragCleanup = useRef<(() => void) | null>(null);

  const load = useCallback(async () => {
    try {
      const list = await invoke<ShelfItem[]>("get_shelf");
      setItems(list);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
    const sub = listen<ShelfItem[]>("shelf-updated", ({ payload }) => {
      setItems(payload);
    });
    return () => {
      dragCleanup.current?.();
      void sub.then((u) => u()).catch(() => {});
    };
  }, [load]);

  const beginNativeDrag = (e: React.MouseEvent, dragIds: string[]) => {
    if (e.button !== 0 || dragIds.length === 0) return;
    e.preventDefault();
    dragCleanup.current?.();
    const startX = e.clientX;
    const startY = e.clientY;
    const moveMode = preferMove;
    let started = false;

    const cleanup = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      dragCleanup.current = null;
    };
    const onMove = (ev: MouseEvent) => {
      if (started) return;
      if (Math.hypot(ev.clientX - startX, ev.clientY - startY) >= 4) {
        started = true;
        cleanup();
        onDraggingOutChange?.(true);
        void invoke<ShelfItem[]>("start_file_drag", {
          ids: dragIds,
          preferMove: moveMode,
        })
          .then((nextItems) => {
            setItems(nextItems);
          })
          .catch((err) => setError(String(err)))
          .finally(() => {
            onDraggingOutChange?.(false);
          });
      }
    };
    const onUp = () => cleanup();

    dragCleanup.current = cleanup;
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  async function runAction<T>(fn: () => Promise<T>, message?: string) {
    setError("");
    try {
      const result = await fn();
      if (message) setStatus(message);
      return result;
    } catch (e) {
      const msg = String(e);
      if (msg !== "Canceled") setError(msg);
      return null;
    }
  }

  async function commitRename(id: string) {
    const next = await runAction(
      () => invoke<ShelfItem[]>("rename_shelf_file", { id, newName: renameText }),
      "File renamed",
    );
    if (next) {
      setItems(next);
      setRenamingId(null);
    }
  }

  async function convertImage(id: string, format: "png" | "jpeg" | "webp") {
    const next = await runAction(
      () => invoke<ShelfItem[]>("convert_shelf_image", { id, format, quality: 88 }),
      `Saved ${format.toUpperCase()} copy`,
    );
    if (next) {
      setItems(next);
      setConvertingId(null);
    }
  }

  async function startQr(id: string) {
    const info = await runAction(() => invoke<QrTransferInfo>("start_qr_transfer", { id }));
    if (info) setQrInfo(info);
  }

  async function stopQr() {
    await invoke("stop_qr_transfer").catch(() => {});
    setQrInfo(null);
  }

  const existingIds = items.filter((i) => i.exists).map((i) => i.id);

  const firstDropName =
    draggedPaths.length > 0 ? draggedPaths[0].split(/[/\\]/).pop() || draggedPaths[0] : "";
  const dropSummary =
    draggedPaths.length === 1
      ? `Drop to stage ${firstDropName}`
      : draggedPaths.length > 1
        ? `Drop to stage ${firstDropName} + ${draggedPaths.length - 1} more`
        : "Release to stage files on Peek Shelf";

  return (
    <section aria-label="File shelf" className="shelf-panel">
      {isDragOver && (
        <div className="files-drop-banner" role="status">
          <FolderOpen size={16} />
          <span className="truncate">{dropSummary}</span>
        </div>
      )}

      <div className="shelf-toolbar">
        <div className="shelf-toolbar-left">
          <button
            type="button"
            className="btn primary sm"
            onClick={() =>
              void runAction(async () => {
                const next = await invoke<ShelfItem[]>("pick_shelf_files");
                setItems(next);
              })
            }
          >
            <Plus size={13} />
            <span>Add</span>
          </button>
          {existingIds.length > 0 && (
            <button
              type="button"
              className="btn sm drag-all-btn"
              title={`Grab & drag ${existingIds.length} staged item(s) to Explorer (${preferMove ? "Move" : "Copy"})`}
              onMouseDown={(e) => beginNativeDrag(e, existingIds)}
            >
              <GripVertical size={12} />
              <span>Drag all ({existingIds.length})</span>
            </button>
          )}
        </div>

        <div className="shelf-toolbar-right">
          <div className="drag-mode-pill" role="group" aria-label="Drag mode">
            <button
              type="button"
              className={"drag-mode-opt" + (preferMove ? " active" : "")}
              title="Default Explorer drag action: Move"
              onClick={() => setPreferMove(true)}
            >
              Move
            </button>
            <button
              type="button"
              className={"drag-mode-opt" + (!preferMove ? " active" : "")}
              title="Default Explorer drag action: Copy"
              onClick={() => setPreferMove(false)}
            >
              Copy
            </button>
          </div>
          {items.length > 0 &&
            (!confirmClear ? (
              <button
                type="button"
                className="btn sm"
                onClick={() => setConfirmClear(true)}
                title="Remove all references from shelf (keeps original files)"
              >
                Clear
              </button>
            ) : (
              <button
                type="button"
                className="btn danger sm"
                onClick={() =>
                  void runAction(async () => {
                    const next = await invoke<ShelfItem[]>("clear_shelf");
                    setItems(next);
                    setConfirmClear(false);
                    setStatus("Shelf cleared; original files kept");
                  })
                }
              >
                Confirm
              </button>
            ))}
        </div>
      </div>

      {qrInfo && (
        <div className="ui-section qr-card" role="dialog" aria-label="Phone QR transfer">
          <div className="qr-header">
            <strong className="truncate">{qrInfo.fileName}</strong>
            <button type="button" className="btn sm" onClick={() => void stopQr()}>
              <X size={12} /> Stop
            </button>
          </div>
          <div className="qr-body">
            <img src={qrInfo.qrDataUrl} alt="QR download code" className="qr-img" />
            <div className="qr-meta">
              <span className="status-note">Scan on the same Wi-Fi network</span>
              <code className="qr-url">{qrInfo.url}</code>
            </div>
          </div>
        </div>
      )}

      {items.length === 0 ? (
        <div className="empty-panel">
          <FolderOpen size={28} strokeWidth={1.5} />
          <span>Drag files from Explorer onto the top notch</span>
        </div>
      ) : (
        <div className="shelf-grid">
          {items.map((item) => {
            const isImg = IMAGE_EXTS.has(item.extension);
            const isZip = item.extension === "zip";
            const isPhoto = Boolean(item.thumbnail && !item.isDir && isImg);
            const isEditing = renamingId === item.id || convertingId === item.id;
            const dragIdsForItem = item.exists ? [item.id] : [];
            return (
              <div
                key={item.id}
                className={
                  "shelf-card" +
                  (item.exists ? " draggable-card" : "") +
                  (isEditing ? " is-editing" : "")
                }
                title={item.path}
                onMouseDown={(e) => {
                  if (!item.exists || renamingId === item.id) return;
                  const target = e.target as HTMLElement;
                  if (target.closest("button, input, a, select")) return;
                  beginNativeDrag(e, dragIdsForItem);
                }}
              >
                <div className="shelf-card-media" aria-hidden="true">
                  {item.thumbnail ? (
                    <img
                      src={item.thumbnail}
                      alt=""
                      draggable={false}
                      className={"shelf-media-img" + (isPhoto ? " is-photo" : " is-icon")}
                    />
                  ) : (
                    <div className="shelf-media-fallback">
                      {item.isDir ? (
                        <Folder size={34} strokeWidth={1.6} />
                      ) : (
                        <FileIcon size={34} strokeWidth={1.6} />
                      )}
                    </div>
                  )}
                </div>

                <div className="shelf-card-overlay">
                  <div className="shelf-card-meta">
                    {renamingId === item.id ? (
                      <div className="rename-row">
                        <input
                          type="text"
                          value={renameText}
                          onChange={(e) => setRenameText(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") void commitRename(item.id);
                            if (e.key === "Escape") setRenamingId(null);
                          }}
                          autoFocus
                        />
                        <button
                          type="button"
                          className="shelf-act-btn primary"
                          aria-label="Save name"
                          onClick={() => void commitRename(item.id)}
                        >
                          <Check size={11} />
                        </button>
                      </div>
                    ) : (
                      <strong title={item.path}>{item.name}</strong>
                    )}
                    <span className="shelf-card-sub">
                      {!item.exists ? (
                        <span className="missing-tag">Missing</span>
                      ) : item.isDir ? (
                        "Folder"
                      ) : (
                        formatBytes(item.sizeBytes)
                      )}
                    </span>
                  </div>

                  <div className="shelf-card-actions">
                    {item.exists && (
                      <>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Copy to folder"
                          aria-label="Copy to folder"
                          onClick={() =>
                            void runAction(async () => {
                              const msg = await invoke<string>("copy_shelf_files", {
                                ids: [item.id],
                              });
                              setStatus(msg);
                            })
                          }
                        >
                          <Copy size={11} />
                        </button>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Move to folder"
                          aria-label="Move to folder"
                          onClick={() =>
                            void runAction(async () => {
                              const next = await invoke<ShelfItem[]>("move_shelf_files", {
                                ids: [item.id],
                              });
                              setItems(next);
                              setStatus("Moved file");
                            })
                          }
                        >
                          <FolderInput size={11} />
                        </button>
                        <button
                          type="button"
                          className="shelf-act-btn"
                          title="Rename"
                          aria-label="Rename"
                          onClick={() => {
                            setRenamingId(item.id);
                            setRenameText(item.name);
                          }}
                        >
                          <Pencil size={11} />
                        </button>
                        {isImg && (
                          <button
                            type="button"
                            className="shelf-act-btn"
                            title="Convert image"
                            aria-label="Convert image"
                            onClick={() =>
                              setConvertingId((cur) => (cur === item.id ? null : item.id))
                            }
                          >
                            <ImageIcon size={11} />
                          </button>
                        )}
                        {isZip && (
                          <button
                            type="button"
                            className="shelf-act-btn"
                            title="Extract ZIP"
                            aria-label="Extract ZIP"
                            onClick={() =>
                              void runAction(async () => {
                                const next = await invoke<ShelfItem[]>("extract_shelf_zip", {
                                  id: item.id,
                                });
                                setItems(next);
                                setStatus("Extracted ZIP");
                              })
                            }
                          >
                            <Archive size={11} />
                          </button>
                        )}
                        {!item.isDir && (
                          <button
                            type="button"
                            className="shelf-act-btn"
                            title="QR phone transfer"
                            aria-label="QR phone transfer"
                            onClick={() => void startQr(item.id)}
                          >
                            <QrCode size={11} />
                          </button>
                        )}
                      </>
                    )}
                    <button
                      type="button"
                      className="shelf-act-btn"
                      title="Remove from shelf (keeps original file)"
                      aria-label="Remove from shelf"
                      onClick={() =>
                        void runAction(async () => {
                          const next = await invoke<ShelfItem[]>("remove_shelf_item", {
                            id: item.id,
                          });
                          setItems(next);
                        })
                      }
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>

                  {convertingId === item.id && (
                    <div className="convert-row">
                      {(["png", "jpeg", "webp"] as const).map((fmt) => (
                        <button
                          key={fmt}
                          type="button"
                          className="btn sm"
                          onClick={() => void convertImage(item.id, fmt)}
                        >
                          {fmt.toUpperCase()}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {status && (
        <div className="status-note shelf-status" role="status">
          {status}
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
