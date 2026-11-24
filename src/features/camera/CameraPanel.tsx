import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Aperture,
  Camera,
  CameraOff,
  FlipHorizontal2,
  FolderOpen,
  PictureInPicture2,
  Sparkles,
} from "lucide-react";
import { EnhancedCameraStage } from "./EnhancedCameraStage";
import {
  captureEnhancedSnapshot,
  nextEnhanceMode,
  openBestCameraStream,
  type CameraEnhanceMode,
} from "./cameraEnhance";

type VideoDevice = { deviceId: string; label: string };

export function CameraPanel() {
  const [cameraOn, setCameraOn] = useState(false);
  const [mirrored, setMirrored] = useState(true);
  const [enhanceMode, setEnhanceMode] = useState<CameraEnhanceMode>("off");
  const [devices, setDevices] = useState<VideoDevice[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState("");
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

  const flash = (msg: string) => {
    setNotice(msg);
    setTimeout(() => setNotice(""), 2200);
  };

  const refreshDevices = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    try {
      const list = await navigator.mediaDevices.enumerateDevices();
      const cams = list
        .filter((d) => d.kind === "videoinput")
        .map((d, idx) => ({
          deviceId: d.deviceId,
          label: d.label || `Camera ${idx + 1}`,
        }));
      setDevices(cams);
      if (cams.length > 0 && !cams.some((c) => c.deviceId === selectedDeviceId)) {
        setSelectedDeviceId(cams[0].deviceId);
      }
    } catch {
      // Ignore device enumeration failures before permission is granted
    }
  }, [selectedDeviceId]);

  const startStream = useCallback(
    async (deviceId?: string) => {
      setError("");
      stopStream();
      if (!navigator.mediaDevices?.getUserMedia) {
        setError("Camera API unavailable in WebView");
        setCameraOn(false);
        return false;
      }
      try {
        const stream = await openBestCameraStream(deviceId);
        streamRef.current = stream;
        setStreamVersion((v) => v + 1);
        setCameraOn(true);
        void refreshDevices();
        return true;
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        setCameraOn(false);
        return false;
      }
    },
    [refreshDevices, stopStream],
  );

  useEffect(() => {
    if (!cameraOn || !videoRef.current || !streamRef.current) return;
    videoRef.current.srcObject = streamRef.current;
    void videoRef.current.play().catch(() => {});
  }, [cameraOn, streamVersion]);

  useEffect(() => {
    return () => {
      stopStream();
    };
  }, [stopStream]);

  async function toggleCamera() {
    if (cameraOn) {
      stopStream();
      setCameraOn(false);
    } else {
      await startStream(selectedDeviceId || undefined);
    }
  }

  async function switchDevice(nextId: string) {
    setSelectedDeviceId(nextId);
    if (cameraOn) {
      await startStream(nextId || undefined);
    }
  }

  async function takeSnapshot() {
    const video = videoRef.current;
    if (!video || !cameraOn) return;
    const dataUrl = captureEnhancedSnapshot(video, mirrored, enhanceMode);
    if (!dataUrl) return;
    setError("");
    try {
      const file = await invoke<string>("save_camera_snapshot", { dataUrl });
      flash(`Saved ${file}`);
    } catch (e) {
      setError(String(e));
    }
  }

  async function popOutFloating() {
    setError("");
    if (cameraOn) {
      stopStream();
      setCameraOn(false);
    }
    try {
      await invoke("open_floating_camera", {
        deviceId: selectedDeviceId || null,
        mirrored,
        enhance: enhanceMode,
      });
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <section aria-label="Toy Cam" className="camera-panel">
      <div className="camera-stage">
        {cameraOn ? (
          <EnhancedCameraStage
            videoRef={videoRef}
            streamVersion={streamVersion}
            mirrored={mirrored}
            enhanceMode={enhanceMode}
          />
        ) : (
          <div className="camera-off-overlay">
            <CameraOff size={28} strokeWidth={1.5} />
            <span className="status-note">Camera is off</span>
            <button
              type="button"
              className="btn primary sm"
              onClick={() => void toggleCamera()}
            >
              <Camera size={13} /> Turn camera on
            </button>
          </div>
        )}
      </div>

      <div className="camera-controls">
        <div className="camera-btn-group">
          <button
            type="button"
            className={"btn sm" + (cameraOn ? " danger" : " primary")}
            onClick={() => void toggleCamera()}
          >
            {cameraOn ? <CameraOff size={12} /> : <Camera size={12} />}
            <span>{cameraOn ? "Turn off" : "Turn on"}</span>
          </button>
          <button
            type="button"
            className={"btn sm" + (mirrored ? " primary" : "")}
            title="Mirror preview"
            aria-label="Toggle mirror"
            onClick={() => setMirrored((m) => !m)}
          >
            <FlipHorizontal2 size={12} />
            <span>Mirror</span>
          </button>
          <button
            type="button"
            className={"btn sm" + (enhanceMode !== "off" ? " primary" : "")}
            title="Cycle camera enhance (Off / Natural / Studio)"
            aria-label="Cycle camera enhance mode"
            onClick={() => setEnhanceMode((m) => nextEnhanceMode(m))}
          >
            <Sparkles size={12} />
            <span>
              {enhanceMode === "off"
                ? "Enhance"
                : enhanceMode === "natural"
                  ? "Natural"
                  : "Studio"}
            </span>
          </button>
          {cameraOn && (
            <button
              type="button"
              className="btn sm"
              title="Take photo (Pictures\\Peek & clipboard)"
              onClick={() => void takeSnapshot()}
            >
              <Aperture size={12} />
              <span>Snap</span>
            </button>
          )}
        </div>

        <div className="camera-btn-group">
          <button
            type="button"
            className="btn sm"
            title="Open captures folder (Pictures\Peek)"
            aria-label="Show captures folder"
            onClick={() =>
              void invoke("open_captures_folder").catch((e) => setError(String(e)))
            }
          >
            <FolderOpen size={12} />
            <span>Captures</span>
          </button>
          <button
            type="button"
            className="btn sm"
            title="Open borderless floating camera window"
            onClick={() => void popOutFloating()}
          >
            <PictureInPicture2 size={12} />
            <span>Float</span>
          </button>
        </div>
      </div>

      {devices.length > 1 && (
        <select
          aria-label="Camera device"
          value={selectedDeviceId}
          onChange={(e) => void switchDevice(e.target.value)}
        >
          {devices.map((d) => (
            <option key={d.deviceId} value={d.deviceId}>
              {d.label}
            </option>
          ))}
        </select>
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
