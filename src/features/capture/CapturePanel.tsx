import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AppWindow,
  Camera,
  Copy,
  Crop,
  FolderOpen,
  Monitor,
  Pause,
  Pipette,
  Play,
  ScanText,
  Square,
  Video,
} from "lucide-react";
import { usePreferences } from "../../stores/preferences";
import { SettingSwitch } from "../settings/SettingsPanel";

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
    <section aria-label="Screenshots and recording" className="capture-panel">
      <div className="ui-section capture-section">
        <div className="capture-row-header">
          <span className="status-note">Screenshot</span>
          {monitors.length > 1 && (
            <select
              aria-label="Target display"
              value={selectedMonitor}
              onChange={(e) => setSelectedMonitor(e.target.value)}
            >
              <option value="">Primary display</option>
              {monitors.map((m) => (
                <option key={m.deviceName || m.name} value={m.deviceName}>
                  {m.name}
                </option>
              ))}
            </select>
          )}
        </div>

        <div className="capture-grid">
          <button
            type="button"
            className="btn"
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
            <Monitor size={14} /> Display
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() =>
              void runCapture(async () => {
                const res = await invoke<CaptureResult>("capture_active_window", { addToShelf });
                setLastResult(res);
              })
            }
          >
            <AppWindow size={14} /> Window
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={busy}
            onClick={() => void beginOverlay("region")}
          >
            <Crop size={14} /> Region
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void beginOverlay("ocr")}
          >
            <ScanText size={14} /> OCR
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void beginOverlay("color")}
          >
            <Pipette size={14} /> Color
          </button>
        </div>

        <SettingSwitch
          label="Also add captures to File Shelf"
          checked={addToShelf}
          disabled={busy}
          onChange={setAddToShelf}
        />
      </div>

      <div className="ui-section capture-section">
        <div className="capture-row-header">
          <span className="status-note">Screen recording</span>
          <div className="capture-header-actions">
            {recStatus.active && (
              <span className="rec-timer">
                <Video size={13} /> {formatTimer(recStatus.elapsedSeconds)}
              </span>
            )}
            <button
              type="button"
              className="btn sm"
              title="Open Videos\Peek folder"
              onClick={() =>
                void invoke("open_recordings_folder").catch((e) => setError(String(e)))
              }
            >
              <FolderOpen size={12} /> Show recordings
            </button>
          </div>
        </div>

        {!recStatus.ffmpegAvailable ? (
          <div className="status-note">FFmpeg not detected on this system.</div>
        ) : recStatus.active ? (
          <div className="capture-rec-controls">
            <button type="button" className="btn" onClick={() => void togglePause()}>
              {recStatus.paused ? <Play size={14} /> : <Pause size={14} />}
              <span>{recStatus.paused ? "Resume" : "Pause"}</span>
            </button>
            <button type="button" className="btn danger" onClick={() => void stopRec()}>
              <Square size={14} />
              <span>Stop & save</span>
            </button>
          </div>
        ) : (
          <>
            <div className="capture-rec-setup">
              <select
                aria-label="Recording mode"
                value={recMode}
                onChange={(e) => setRecMode(e.target.value as typeof recMode)}
              >
                <option value="display">Display</option>
                <option value="window">Active window</option>
                <option value="region">Region</option>
              </select>
              <button
                type="button"
                className="btn primary"
                disabled={busy}
                onClick={() => void startRec()}
              >
                <Camera size={14} /> Start recording
              </button>
            </div>
            {micDevices.length > 0 && (
              <div className="capture-mic-row">
                <SettingSwitch
                  label="Include microphone"
                  checked={includeMic}
                  disabled={busy}
                  onChange={setIncludeMic}
                />
                {includeMic && (
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
            )}
          </>
        )}
      </div>

      {lastResult && (
        <div className="ui-section capture-result">
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
