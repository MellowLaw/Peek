import { useEffect, useRef, useState, type RefObject } from "react";
import {
  CameraGlRenderer,
  fallbackCssFilter,
  type CameraEnhanceMode,
} from "./cameraEnhance";

export function EnhancedCameraStage({
  videoRef,
  streamVersion,
  mirrored,
  enhanceMode,
}: {
  videoRef: RefObject<HTMLVideoElement | null>;
  streamVersion: number;
  mirrored: boolean;
  enhanceMode: CameraEnhanceMode;
}) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [glFailed, setGlFailed] = useState(false);

  const shaderActive = enhanceMode !== "off" && !glFailed;

  useEffect(() => {
    if (enhanceMode === "off") {
      setGlFailed(false);
      return;
    }
    const canvas = canvasRef.current;
    const video = videoRef.current;
    if (!canvas || !video) return;

    let renderer: CameraGlRenderer | null = null;
    try {
      renderer = new CameraGlRenderer(canvas, false);
      setGlFailed(false);
    } catch {
      setGlFailed(true);
      return;
    }

    let cancelled = false;
    let rafId = 0;
    let vfcId = 0;

    const renderCurrent = () => {
      if (cancelled || !renderer || !videoRef.current) return;
      renderer.render(videoRef.current, enhanceMode, mirrored, true);
    };

    const scheduleNext = () => {
      if (cancelled) return;
      if (typeof video.requestVideoFrameCallback === "function") {
        vfcId = video.requestVideoFrameCallback(() => {
          renderCurrent();
          scheduleNext();
        });
      } else {
        rafId = requestAnimationFrame(() => {
          renderCurrent();
          scheduleNext();
        });
      }
    };

    const onLoadedData = () => renderCurrent();
    video.addEventListener("loadeddata", onLoadedData);
    renderCurrent();
    scheduleNext();

    const ro = new ResizeObserver(() => renderCurrent());
    if (wrapRef.current) ro.observe(wrapRef.current);

    return () => {
      cancelled = true;
      video.removeEventListener("loadeddata", onLoadedData);
      ro.disconnect();
      if (vfcId && typeof video.cancelVideoFrameCallback === "function") {
        video.cancelVideoFrameCallback(vfcId);
      }
      if (rafId) cancelAnimationFrame(rafId);
      renderer?.dispose();
    };
  }, [enhanceMode, mirrored, streamVersion, videoRef]);

  return (
    <div ref={wrapRef} className="camera-stage-media">
      <video
        ref={videoRef}
        autoPlay
        playsInline
        muted
        className={
          "camera-video is-live" +
          (mirrored ? " is-mirrored" : "") +
          (shaderActive ? " is-shader-backed" : "")
        }
        style={
          !shaderActive && enhanceMode !== "off"
            ? { filter: fallbackCssFilter(enhanceMode) }
            : undefined
        }
      />
      {enhanceMode !== "off" && !glFailed && (
        <canvas
          ref={canvasRef}
          className="camera-enhance-canvas"
          aria-hidden="true"
        />
      )}
    </div>
  );
}
