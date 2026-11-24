import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Aperture,
  Camera,
  CameraOff,
  FlipHorizontal2,
  Maximize2,
  Sparkles,
  X,
} from "lucide-react";
import { EnhancedCameraStage } from "../camera/EnhancedCameraStage";
import {
  captureEnhancedSnapshot,
  nextEnhanceMode,
  normalizeEnhanceMode,
  openBestCameraStream,
  type CameraEnhanceMode,
} from "../camera/cameraEnhance";

const SIZE_PRESETS = [
  { label: "S", width: 280, height: 184 },
  { label: "M", width: 360, height: 236 },
  { label: "L", width: 480, height: 314 },
];

function parseHashParams(): {
  deviceId: string;
  mirrored: boolean;
  enhance: CameraEnhanceMode;
} {
  const hash = window.location.hash;
  const qIdx = hash.indexOf("?");
  if (qIdx === -1) return { deviceId: "", mirrored: true, enhance: "off" };
  const params = new URLSearchParams(hash.slice(qIdx + 1));
  return {
    deviceId: params.get("device") ?? "",
    mirrored: params.get("mirrored") !== "0",
    enhance: normalizeEnhanceMode(params.get("enhance")),
  };
}

export function FloatingCamera() {
  const initial = parseHashParams();
  const [cameraOn, setCameraOn] = useState(false);
  const [mirrored, setMirrored] = useState(initial.mirrored);
  const [enhanceMode, setEnhanceMode] = useState<CameraEnhanceMode>(initial.enhance);
  const [sizeIdx, setSizeIdx] = useState(1);
  const [streamVersion, setStreamVersion] = useState(0);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const streamRef = useRef<MediaStream | null>(null);

  const stopStream = useCallback(() => {
    const s = streamRef.current;
    if (s) {
      for (const track of s.getTracks()) {
        track.stop();
      }
      streamRef.current = null;
    }
    if (videoRef.current) {
      videoRef.current.pause();
      videoRef.current.srcObject = null;
      videoRef.current.load();
    }
  }, []);

  const startStream = useCallback(async () => {
    setError("");
    stopStream();
    if (!navigator.mediaDevices?.getUserMedia) {
      setError("Camera unavailable");
      setCameraOn(false);
      return;
    }
    try {
      const stream = await openBestCameraStream(initial.deviceId || undefined);
      streamRef.current = stream;
      setStreamVersion((v) => v + 1);
      setCameraOn(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setCameraOn(false);
    }
  }, [initial.deviceId, stopStream]);

  useEffect(() => {
    if (!cameraOn || !videoRef.current || !streamRef.current) return;
    videoRef.current.srcObject = streamRef.current;
    void videoRef.current.play().catch(() => {});
  }, [cameraOn, streamVersion]);

  useEffect(() => {
    void startStream();
    return () => {
      stopStream();
    };
  }, [startStream, stopStream]);

  async function toggleCamera() {
    if (cameraOn) {
      stopStream();
      setCameraOn(false);
    } else {
      await startStream();
    }
  }

  async function cycleSize() {
    const nextIdx = (sizeIdx + 1) % SIZE_PRESETS.length;
    setSizeIdx(nextIdx);
    const preset = SIZE_PRESETS[nextIdx];
    await invoke("resize_floating_window", {
      width: preset.width,
      height: preset.height,
    }).catch(() => {});
  }

  async function takeSnap() {
    const video = videoRef.current;
    if (!video || !cameraOn) return;
    const dataUrl = captureEnhancedSnapshot(video, mirrored, enhanceMode);
    if (!dataUrl) return;
    try {
      await invoke("save_camera_snapshot", { dataUrl });
      setNotice("Saved");
      setTimeout(() => setNotice(""), 1600);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleClose() {
    stopStream();
    await invoke("close_floating_window").catch(() => window.close());
  }

  function handleDragMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button, select, input, .float-resize-handle")) return;
    void invoke("start_floating_drag");
  }

  function startResize(corner: "se" | "sw" | "nw", e: React.MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    void invoke("start_floating_resize", { corner }).catch(() => {});
  }

  return (
    <div className="float-win float-cam" onMouseDown={handleDragMouseDown}>
      {cameraOn ? (
        <EnhancedCameraStage
          videoRef={videoRef}
          streamVersion={streamVersion}
          mirrored={mirrored}
          enhanceMode={enhanceMode}
        />
      ) : (
        <div className="camera-off-overlay">
          <CameraOff size={24} strokeWidth={1.6} />
          <span className="status-note">{error || "Camera off"}</span>
          <button type="button" className="btn primary sm" onClick={() => void toggleCamera()}>
            <Camera size={12} /> Turn on
          </button>
        </div>
      )}

      <div className="float-cam-top">
        {notice && <span className="float-cam-pill">{notice}</span>}
        <button
          type="button"
          className="float-close-btn"
          aria-label="Close floating camera"
          title="Close"
          onClick={() => void handleClose()}
        >
          <X size={13} />
        </button>
      </div>

      <div className={"float-cam-bar" + (!cameraOn ? " is-visible" : "")}>
        <button
          type="button"
          className="float-bar-btn"
          title={cameraOn ? "Turn off" : "Turn on"}
          aria-label={cameraOn ? "Turn off camera" : "Turn on camera"}
          onClick={() => void toggleCamera()}
        >
          {cameraOn ? <CameraOff size={12} /> : <Camera size={12} />}
        </button>
        <button
          type="button"
          className={"float-bar-btn" + (mirrored ? " active" : "")}
          title="Mirror"
          aria-label="Mirror camera"
          onClick={() => setMirrored((m) => !m)}
        >
          <FlipHorizontal2 size={12} />
        </button>
        <button
          type="button"
          className={"float-bar-btn" + (enhanceMode !== "off" ? " active" : "")}
          title="Cycle enhance (Off / Natural / Studio)"
          aria-label="Cycle camera enhance mode"
          onClick={() => setEnhanceMode((m) => nextEnhanceMode(m))}
        >
          <Sparkles size={12} />
          {enhanceMode !== "off" && <span>{enhanceMode === "natural" ? "Nat" : "Stu"}</span>}
        </button>
        {cameraOn && (
          <button
            type="button"
            className="float-bar-btn"
            title="Snap photo"
            aria-label="Snap photo"
            onClick={() => void takeSnap()}
          >
            <Aperture size={12} />
          </button>
        )}
        <button
          type="button"
          className="float-bar-btn"
          title="Cycle floating size"
          aria-label="Resize floating camera"
          onClick={() => void cycleSize()}
        >
          <Maximize2 size={11} />
          <span>{SIZE_PRESETS[sizeIdx].label}</span>
        </button>
      </div>

      <div
        className="float-resize-handle is-nw"
        aria-hidden="true"
        onMouseDown={(e) => startResize("nw", e)}
      />
      <div
        className="float-resize-handle is-sw"
        aria-hidden="true"
        onMouseDown={(e) => startResize("sw", e)}
      />
      <div
        className="float-resize-handle is-se"
        role="separator"
        aria-label="Drag corner to resize floating camera"
        title="Drag corner to resize"
        onMouseDown={(e) => startResize("se", e)}
      >
        <svg className="float-resize-grip" viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
          <path
            d="M 11.5 5 A 6.5 6.5 0 0 1 5 11.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
        </svg>
      </div>
    </div>
  );
}
