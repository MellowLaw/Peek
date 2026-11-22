import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Aperture,
  Camera,
  CameraOff,
  FlipHorizontal2,
  PictureInPicture2,
} from "lucide-react";

type VideoDevice = { deviceId: string; label: string };

export function CameraPanel() {
  const [cameraOn, setCameraOn] = useState(false);
  const [mirrored, setMirrored] = useState(true);
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
        const videoConstraint: MediaTrackConstraints | boolean = deviceId
          ? { deviceId: { exact: deviceId }, width: { ideal: 1280 }, height: { ideal: 720 } }
          : { width: { ideal: 1280 }, height: { ideal: 720 } };
        const stream = await navigator.mediaDevices.getUserMedia({
          video: videoConstraint,
          audio: false,
        });
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
    if (!video || !cameraOn || video.videoWidth === 0 || video.videoHeight === 0) return;
    setError("");
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
      });
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <section aria-label="Toy Cam" className="camera-panel">
      <div className="camera-stage">
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
