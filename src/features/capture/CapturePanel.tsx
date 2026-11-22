import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AppWindow,
  Check,
  Copy,
  Crop,
  FolderOpen,
  Mic,
  MicOff,
  Monitor,
  Pause,
  Pipette,
  Play,
  ScanText,
  Square,
  Video,
} from "lucide-react";
import { usePreferences } from "../../stores/preferences";

type CaptureResult =
  | { kind: "image"; path: string; thumbnail: string; width: number; height: number }
  | { kind: "ocr"; text: string }
  | { kind: "color"; hex: string; rgb: string; r: number; g: number; b: number }
  | { kind: "video"; path: string };

type RecordingStatus = {
  active: boolean;
  paused: boolean;
  elapsedSeconds: number;
  ffmpegAvailable: boolean;
  mode: string;
};

function formatTimer(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

export function CapturePanel() {
  const { monitors, preferences } = usePreferences();
  const [selectedMonitor, setSelectedMonitor] = useState(preferences.monitorName);
  const [addToShelf, setAddToShelf] = useState(true);
  const [recMode, setRecMode] = useState<"display" | "window" | "region">("display");
  const [includeMic, setIncludeMic] = useState(false);
  const [micDevices, setMicDevices] = useState<string[]>([]);
  const [selectedMic, setSelectedMic] = useState("");
  const [recStatus, setRecStatus] = useState<RecordingStatus>({
    active: false,
    paused: false,
    elapsedSeconds: 0,
    ffmpegAvailable: true,
    mode: "",
  });
  const [lastResult, setLastResult] = useState<CaptureResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  useEffect(() => {
    void invoke<RecordingStatus>("get_recording_status")
      .then(setRecStatus)
      .catch(() => {});
    void invoke<string[]>("list_audio_inputs")
      .then((list) => {
        setMicDevices(list);
        if (list.length > 0) setSelectedMic(list[0]);
      })
      .catch(() => {});

    const subStatus = listen<RecordingStatus>("recording-status", ({ payload }) => {
      setRecStatus(payload);
    });
    const subRes = listen<CaptureResult>("capture-result", ({ payload }) => {
      setLastResult(payload);
    });
    return () => {
      void subStatus.then((u) => u()).catch(() => {});
      void subRes.then((u) => u()).catch(() => {});
    };
  }, []);

  async function runCapture<T>(fn: () => Promise<T>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      return await fn();
    } catch (e) {
      setError(String(e));
      return null;
    } finally {
      setBusy(false);
    }
  }

  async function beginOverlay(mode: "region" | "ocr" | "color" | "record-region") {
    await runCapture(() =>
      invoke("begin_screen_overlay", {
        mode,
        addToShelf,
        includeMic,
        micDevice: selectedMic,
      }),
    );
  }

  async function startRec() {
    if (recMode === "region") {
      await beginOverlay("record-region");
      return;
    }
    const ok = await runCapture(() =>
      invoke("start_recording", {
        mode: recMode,
        monitorName: selectedMonitor,
        includeMic,
        micDevice: selectedMic,
        addToShelf,
      }),
    );
    if (ok !== null) {
      const next = await invoke<RecordingStatus>("get_recording_status");
      setRecStatus(next);
    }
  }

  async function togglePause() {
    await runCapture(async () => {
      if (recStatus.paused) {
        await invoke("resume_recording");
      } else {
        await invoke("pause_recording");
      }
      const next = await invoke<RecordingStatus>("get_recording_status");
      setRecStatus(next);
    });
  }

  async function stopRec() {
    const res = await runCapture(() => invoke<CaptureResult>("stop_recording"));
    if (res) setLastResult(res);
    const next = await invoke<RecordingStatus>("get_recording_status").catch(() => null);
    if (next) setRecStatus(next);
  }

  return (
    <section aria-label="Screenshots and recording" className="capture-bento">
      {/* Capture Header & Screenshot Bento */}
      <div className="capture-bento-section">
        <div className="capture-bento-head">
          <span className="bento-label">Capture</span>
          <div className="capture-head-right">
            {monitors.length > 1 && (
              <select
                className="bento-compact-select"
                aria-label="Target display"
                value={selectedMonitor}
                onChange={(e) => setSelectedMonitor(e.target.value)}
              >
                <option value="">Primary</option>
                {monitors.map((m) => (
                  <option key={m.deviceName || m.name} value={m.deviceName}>
                    {m.name}
                  </option>
                ))}
              </select>
            )}
            <button
              type="button"
              className={"capture-shelf-chip" + (addToShelf ? " is-on" : "")}
              disabled={busy}
              aria-pressed={addToShelf}
              onClick={() => setAddToShelf((v) => !v)}
              title="Automatically add screenshots & recordings to Files tab"
            >
              <Check size={11} />
              <span>Save to Files</span>
            </button>
          </div>
        </div>

        <div className="capture-bento-grid">
          <button
            type="button"
            className="capture-hero-tile"
            disabled={busy}
            onClick={() => void beginOverlay("region")}
          >
            <Crop size={20} />
            <div className="capture-hero-copy">
              <strong>Region</strong>
              <span>{preferences.captureShortcut}</span>
            </div>
          </button>

          <div className="capture-sub-tiles">
            <button
              type="button"
              className="capture-sub-tile"
              disabled={busy}
              onClick={() =>
                void runCapture(async () => {
                  const res = await invoke<CaptureResult>("capture_full_display", {
                    monitorName: selectedMonitor,
                    addToShelf,
                  });
                  setLastResult(res);
                })
              }
            >
              <Monitor size={15} />
              <span>Display</span>
            </button>

            <button
              type="button"
              className="capture-sub-tile"
              disabled={busy}
              onClick={() =>
                void runCapture(async () => {
                  const res = await invoke<CaptureResult>("capture_active_window", { addToShelf });
                  setLastResult(res);
                })
              }
            >
              <AppWindow size={15} />
              <span>Window</span>
            </button>

            <button
              type="button"
              className="capture-sub-tile"
              disabled={busy}
              onClick={() => void beginOverlay("ocr")}
            >
              <ScanText size={15} />
              <span>OCR Text</span>
            </button>

            <button
              type="button"
              className="capture-sub-tile"
              disabled={busy}
              onClick={() => void beginOverlay("color")}
            >
              <Pipette size={15} />
              <span>Color</span>
            </button>
          </div>
        </div>
      </div>

      {/* Screen Recording Bento Tile */}
      <div className="bento-tile capture-rec-bento">
        <div className="capture-bento-head">
          <div className="capture-rec-title">
            <span className="bento-label">Screen recording</span>
            {recStatus.active && (
              <span className="rec-timer">
                <Video size={12} /> {formatTimer(recStatus.elapsedSeconds)}
              </span>
            )}
          </div>
          <button
            type="button"
            className="btn sm"
            title="Open Videos\Peek folder"
            onClick={() =>
              void invoke("open_recordings_folder").catch((e) => setError(String(e)))
            }
          >
            <FolderOpen size={11} /> Recordings
          </button>
        </div>

        {!recStatus.ffmpegAvailable ? (
          <div className="status-note">FFmpeg not detected on this system.</div>
        ) : recStatus.active ? (
          <div className="capture-rec-controls">
            <button type="button" className="btn sm" onClick={() => void togglePause()}>
              {recStatus.paused ? <Play size={13} /> : <Pause size={13} />}
              <span>{recStatus.paused ? "Resume" : "Pause"}</span>
            </button>
            <button type="button" className="btn danger sm" onClick={() => void stopRec()}>
              <Square size={13} />
              <span>Stop & save</span>
            </button>
          </div>
        ) : (
          <div className="capture-rec-bar">
            <div className="drag-mode-pill" role="group" aria-label="Recording target">
              {(
                [
                  ["display", "Display"],
                  ["window", "Window"],
                  ["region", "Region"],
                ] as const
              ).map(([modeVal, lbl]) => (
                <button
                  key={modeVal}
                  type="button"
                  className={"drag-mode-opt" + (recMode === modeVal ? " active" : "")}
                  onClick={() => setRecMode(modeVal)}
                >
                  {lbl}
                </button>
              ))}
            </div>

            <div className="capture-rec-right">
              {micDevices.length > 0 && (
                <button
                  type="button"
                  className={"btn sm" + (includeMic ? " primary" : "")}
                  aria-pressed={includeMic}
                  title={includeMic ? "Microphone enabled" : "Include microphone"}
                  onClick={() => setIncludeMic((v) => !v)}
                >
                  {includeMic ? <Mic size={12} /> : <MicOff size={12} />}
                  <span>Mic</span>
                </button>
              )}
              <button
                type="button"
                className="btn primary sm"
                disabled={busy}
                onClick={() => void startRec()}
              >
                <Video size={12} /> Record
              </button>
            </div>
          </div>
        )}

        {!recStatus.active && includeMic && micDevices.length > 1 && (
          <select
            aria-label="Microphone device"
            value={selectedMic}
            onChange={(e) => setSelectedMic(e.target.value)}
          >
            {micDevices.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        )}
      </div>

      {lastResult && (
        <div className="bento-tile capture-result">
          {lastResult.kind === "image" && (
            <div className="capture-image-preview">
              <img src={lastResult.thumbnail} alt="Captured screenshot" />
              <div className="capture-meta">
                <span>
                  {lastResult.width} × {lastResult.height}
                </span>
                <button
                  type="button"
                  className="btn sm"
                  onClick={() =>
                    void invoke("reveal_capture_file", { path: lastResult.path }).catch((e) =>
                      setError(String(e)),
                    )
                  }
                >
                  <FolderOpen size={12} /> Reveal
                </button>
              </div>
            </div>
          )}
          {lastResult.kind === "ocr" && (
            <div className="capture-ocr-preview">
              <div className="capture-row-header">
                <span className="status-note">Extracted text</span>
                <button
                  type="button"
                  className="btn sm"
                  onClick={() =>
                    void navigator.clipboard
                      .writeText(lastResult.text)
                      .then(() => setNotice("Copied text"))
                  }
                >
                  <Copy size={12} /> Copy
                </button>
              </div>
              <pre>{lastResult.text}</pre>
            </div>
          )}
          {lastResult.kind === "color" && (
            <div className="capture-color-preview">
              <span className="color-swatch" style={{ background: lastResult.hex }} />
              <div>
                <strong>{lastResult.hex}</strong>
                <div className="status-note">{lastResult.rgb}</div>
              </div>
              <button
                type="button"
                className="btn sm"
                onClick={() =>
                  void navigator.clipboard
                    .writeText(lastResult.hex)
                    .then(() => setNotice("Copied " + lastResult.hex))
                }
              >
                <Copy size={12} /> Copy
              </button>
            </div>
          )}
          {lastResult.kind === "video" && (
            <div className="capture-row-header">
              <span className="status-note">Recording saved</span>
              <button
                type="button"
                className="btn sm"
                onClick={() =>
                  void invoke("reveal_capture_file", { path: lastResult.path }).catch((e) =>
                    setError(String(e)),
                  )
                }
              >
                <FolderOpen size={12} /> Reveal MP4
              </button>
            </div>
          )}
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
