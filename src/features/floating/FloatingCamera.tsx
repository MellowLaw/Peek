import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Aperture,
  Camera,
  CameraOff,
  FlipHorizontal2,
  Maximize2,
  X,
} from "lucide-react";

const SIZE_PRESETS = [
  { label: "S", width: 280, height: 184 },
  { label: "M", width: 360, height: 236 },
  { label: "L", width: 480, height: 314 },
];

function parseHashParams(): { deviceId: string; mirrored: boolean } {
  const hash = window.location.hash;
  const qIdx = hash.indexOf("?");
  if (qIdx === -1) return { deviceId: "", mirrored: true };
  const params = new URLSearchParams(hash.slice(qIdx + 1));
  return {
    deviceId: params.get("device") ?? "",
    mirrored: params.get("mirrored") !== "0",
  };
}

export function FloatingCamera() {
  const initial = parseHashParams();
  const [cameraOn, setCameraOn] = useState(false);
  const [mirrored, setMirrored] = useState(initial.mirrored);
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
      const constraint: MediaTrackConstraints = initial.deviceId
        ? { deviceId: { exact: initial.deviceId }, width: { ideal: 1280 }, height: { ideal: 720 } }
        : { width: { ideal: 1280 }, height: { ideal: 720 } };
      const stream = await navigator.mediaDevices.getUserMedia({ video: constraint, audio: false });
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
    if (!video || !cameraOn || video.videoWidth === 0 || video.videoHeight === 0) return;
    const canvas = document.createElement("canvas");
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    if (mirrored) {
      ctx.translate(canvas.width, 0);
      ctx.scale(-1, 1);
    }
    ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
    const dataUrl = canvas.toDataURL("image/png");
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
    if ((e.target as HTMLElement).closest("button, select, input")) return;
    void invoke("start_floating_drag");
  }

  return (
    <div className="float-win float-cam" onMouseDown={handleDragMouseDown}>
      {cameraOn ? (
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted
          className={"camera-video is-live" + (mirrored ? " is-mirrored" : "")}
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
    </div>
  );
}
