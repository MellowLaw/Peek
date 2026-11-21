import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type OverlayPayload = {
  mode: "region" | "ocr" | "color" | "record-region";
  previewDataUrl: string;
  virtualWidth: number;
  virtualHeight: number;
};

type Rect = { x: number; y: number; w: number; h: number };

export function CaptureOverlay() {
  const [payload, setPayload] = useState<OverlayPayload | null>(null);
  const [dragStart, setDragStart] = useState<{ x: number; y: number } | null>(null);
  const [rect, setRect] = useState<Rect | null>(null);
  const [hoverPos, setHoverPos] = useState<{ x: number; y: number; hex: string }>({
    x: 0,
    y: 0,
    hex: "#000000",
  });
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    void invoke<OverlayPayload | null>("get_overlay_state")
      .then((res) => {
        if (res) setPayload(res);
      })
      .catch(() => {});
    const sub = listen<OverlayPayload>("overlay-ready", ({ payload: p }) => {
      setPayload(p);
      setRect(null);
      setDragStart(null);
    });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        void invoke("cancel_overlay");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      void sub.then((u) => u()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    if (!payload || payload.mode !== "color") return;
    const img = new Image();
    img.onload = () => {
      const c = document.createElement("canvas");
      c.width = img.width;
      c.height = img.height;
      const ctx = c.getContext("2d", { willReadFrequently: true });
      ctx?.drawImage(img, 0, 0);
      canvasRef.current = c;
    };
    img.src = payload.previewDataUrl;
  }, [payload]);

  if (!payload) {
    return <div style={{ width: "100vw", height: "100vh", background: "#080808" }} />;
  }

  const isColor = payload.mode === "color";

  const sampleHex = (clientX: number, clientY: number) => {
    const c = canvasRef.current;
    if (!c) return "#000000";
    const ctx = c.getContext("2d", { willReadFrequently: true });
    if (!ctx) return "#000000";
    const px = Math.min(c.width - 1, Math.max(0, Math.round((clientX / window.innerWidth) * (c.width - 1))));
    const py = Math.min(c.height - 1, Math.max(0, Math.round((clientY / window.innerHeight) * (c.height - 1))));
    const d = ctx.getImageData(px, py, 1, 1).data;
    return (
      "#" +
      [d[0], d[1], d[2]].map((n) => n.toString(16).padStart(2, "0").toUpperCase()).join("")
    );
  };

  return (
    <div
      style={{
        position: "relative",
        width: "100vw",
        height: "100vh",
        overflow: "hidden",
        cursor: "crosshair",
        userSelect: "none",
        background: "#080808",
      }}
      onMouseDown={(e) => {
        if (e.button === 2) {
          void invoke("cancel_overlay");
          return;
        }
        if (isColor) {
          void invoke("complete_overlay_color", {
            xRatio: e.clientX / window.innerWidth,
            yRatio: e.clientY / window.innerHeight,
          });
          return;
        }
        setDragStart({ x: e.clientX, y: e.clientY });
        setRect({ x: e.clientX, y: e.clientY, w: 0, h: 0 });
      }}
      onMouseMove={(e) => {
        if (isColor) {
          setHoverPos({ x: e.clientX, y: e.clientY, hex: sampleHex(e.clientX, e.clientY) });
          return;
        }
        if (!dragStart) return;
        const x = Math.min(dragStart.x, e.clientX);
        const y = Math.min(dragStart.y, e.clientY);
        const w = Math.abs(e.clientX - dragStart.x);
        const h = Math.abs(e.clientY - dragStart.y);
        setRect({ x, y, w, h });
      }}
      onMouseUp={() => {
        if (isColor || !dragStart || !rect) return;
        setDragStart(null);
        if (rect.w < 4 || rect.h < 4) {
          setRect(null);
          return;
        }
        void invoke("complete_overlay_region", {
          xRatio: rect.x / window.innerWidth,
          yRatio: rect.y / window.innerHeight,
          wRatio: rect.w / window.innerWidth,
          hRatio: rect.h / window.innerHeight,
        });
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        void invoke("cancel_overlay");
      }}
    >
      <img
        src={payload.previewDataUrl}
        alt=""
        draggable={false}
        style={{ width: "100%", height: "100%", objectFit: "fill", display: "block" }}
      />

      {!isColor && !(rect && rect.w > 2 && rect.h > 2) && (
        <div
          style={{
            position: "absolute",
            inset: 0,
            background: "rgba(0, 0, 0, 0.35)",
            pointerEvents: "none",
          }}
        />
      )}

      {!isColor && rect && rect.w > 2 && rect.h > 2 && (
        <div
          style={{
            position: "absolute",
            left: rect.x,
            top: rect.y,
            width: rect.w,
            height: rect.h,
            border: "2px solid #f4f4f4",
            boxShadow: "0 0 0 9999px rgba(0, 0, 0, 0.45)",
            pointerEvents: "none",
          }}
        >
          <span
            style={{
              position: "absolute",
              left: 0,
              top: -26,
              padding: "2px 7px",
              borderRadius: 6,
              background: "#080808",
              color: "#f4f4f4",
              fontSize: 11,
              fontVariantNumeric: "tabular-nums",
              whiteSpace: "nowrap",
            }}
          >
            {Math.round((rect.w / window.innerWidth) * payload.virtualWidth)} ×{" "}
            {Math.round((rect.h / window.innerHeight) * payload.virtualHeight)}
          </span>
        </div>
      )}

      {isColor && (
        <div
          style={{
            position: "absolute",
            left: Math.min(window.innerWidth - 130, hoverPos.x + 14),
            top: Math.min(window.innerHeight - 40, hoverPos.y + 14),
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "5px 9px",
            borderRadius: 8,
            background: "#080808",
            border: "1px solid #2b2b2b",
            color: "#f4f4f4",
            fontSize: 12,
            pointerEvents: "none",
          }}
        >
          <span
            style={{
              width: 14,
              height: 14,
              borderRadius: 4,
              background: hoverPos.hex,
              border: "1px solid #444",
            }}
          />
          <span>{hoverPos.hex}</span>
        </div>
      )}

      <div
        style={{
          position: "absolute",
          top: 16,
          left: "50%",
          transform: "translateX(-50%)",
          padding: "5px 12px",
          borderRadius: 999,
          background: "#080808",
          border: "1px solid #282828",
          color: "#f4f4f4",
          fontSize: 12,
          pointerEvents: "none",
        }}
      >
        {isColor ? "Click to copy pixel color · Esc to cancel" : "Drag to select region · Esc to cancel"}
      </div>
    </div>
  );
}
