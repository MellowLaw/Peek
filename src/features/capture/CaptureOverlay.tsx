import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type OverlayPayload = {
  mode: "region" | "ocr" | "color" | "record-region";
  previewDataUrl: string;
  virtualWidth: number;
  virtualHeight: number;
};

type Point = { x: number; y: number };
type Rect = { x: number; y: number; w: number; h: number };

export function CaptureOverlay() {
  const [payload, setPayload] = useState<OverlayPayload | null>(null);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sampleCtxRef = useRef<CanvasRenderingContext2D | null>(null);
  const sampleSizeRef = useRef<{ w: number; h: number }>({ w: 0, h: 0 });
  const colorBadgeRef = useRef<HTMLDivElement | null>(null);
  const colorSwatchRef = useRef<HTMLSpanElement | null>(null);
  const colorTextRef = useRef<HTMLSpanElement | null>(null);

  const dragStartRef = useRef<Point | null>(null);
  const curPosRef = useRef<Point | null>(null);
  const rafRef = useRef<number>(0);

  const drawOverlay = useCallback(() => {
    rafRef.current = 0;
    const canvas = canvasRef.current;
    if (!canvas || !payload || payload.mode === "color") return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const dpr = window.devicePixelRatio || 1;
    const targetW = Math.max(1, Math.round(vw * dpr));
    const targetH = Math.max(1, Math.round(vh * dpr));
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, vw, vh);

    const start = dragStartRef.current;
    const cur = curPosRef.current;

    if (!start || !cur) {
      ctx.fillStyle = "rgba(0, 0, 0, 0.36)";
      ctx.fillRect(0, 0, vw, vh);
      if (cur) {
        ctx.strokeStyle = "rgba(244, 244, 244, 0.38)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(cur.x + 0.5, 0);
        ctx.lineTo(cur.x + 0.5, vh);
        ctx.moveTo(0, cur.y + 0.5);
        ctx.lineTo(vw, cur.y + 0.5);
        ctx.stroke();
      }
      return;
    }

    const x = Math.min(start.x, cur.x);
    const y = Math.min(start.y, cur.y);
    const w = Math.abs(cur.x - start.x);
    const h = Math.abs(cur.y - start.y);

    ctx.fillStyle = "rgba(0, 0, 0, 0.46)";
    ctx.fillRect(0, 0, vw, vh);

    if (w > 1 && h > 1) {
      ctx.clearRect(x, y, w, h);
      ctx.strokeStyle = "#f4f4f4";
      ctx.lineWidth = 2;
      ctx.strokeRect(x, y, w, h);

      const pxW = Math.round((w / Math.max(1, vw)) * payload.virtualWidth);
      const pxH = Math.round((h / Math.max(1, vh)) * payload.virtualHeight);
      const label = `${pxW} × ${pxH}`;
      ctx.font = '600 11px "Segoe UI Variable", "Segoe UI", sans-serif';
      const tw = ctx.measureText(label).width;
      const bw = Math.ceil(tw + 14);
      const bh = 22;
      const bx = Math.min(Math.max(4, x), Math.max(4, vw - bw - 4));
      const by = y >= bh + 6 ? y - bh - 6 : Math.min(vh - bh - 4, y + 6);

      ctx.fillStyle = "#080808";
      ctx.beginPath();
      ctx.roundRect(bx, by, bw, bh, 6);
      ctx.fill();
      ctx.strokeStyle = "rgba(255, 255, 255, 0.16)";
      ctx.lineWidth = 1;
      ctx.stroke();

      ctx.fillStyle = "#f4f4f4";
      ctx.textBaseline = "middle";
      ctx.fillText(label, bx + 7, by + bh / 2);
    }
  }, [payload]);

  const scheduleDraw = useCallback(() => {
    if (rafRef.current) return;
    rafRef.current = window.requestAnimationFrame(() => {
      if (!payload) {
        rafRef.current = 0;
        return;
      }
      if (payload.mode === "color") {
        rafRef.current = 0;
        const pos = curPosRef.current;
        const badge = colorBadgeRef.current;
        const ctx = sampleCtxRef.current;
        if (!pos || !badge) return;
        let hex = "#000000";
        if (ctx) {
          const { w, h } = sampleSizeRef.current;
          const px = Math.min(
            w - 1,
            Math.max(0, Math.round((pos.x / Math.max(1, window.innerWidth)) * (w - 1))),
          );
          const py = Math.min(
            h - 1,
            Math.max(0, Math.round((pos.y / Math.max(1, window.innerHeight)) * (h - 1))),
          );
          const d = ctx.getImageData(px, py, 1, 1).data;
          hex =
            "#" +
            [d[0], d[1], d[2]]
              .map((n) => n.toString(16).padStart(2, "0").toUpperCase())
              .join("");
        }
        const tx = Math.min(window.innerWidth - 130, pos.x + 14);
        const ty = Math.min(window.innerHeight - 40, pos.y + 14);
        badge.style.transform = `translate3d(${tx}px, ${ty}px, 0)`;
        badge.style.opacity = "1";
        if (colorSwatchRef.current) colorSwatchRef.current.style.background = hex;
        if (colorTextRef.current) colorTextRef.current.textContent = hex;
        return;
      }
      drawOverlay();
    });
  }, [drawOverlay, payload]);

  useEffect(() => {
    void invoke<OverlayPayload | null>("get_overlay_state")
      .then((res) => {
        if (res) setPayload(res);
      })
      .catch(() => {});
    const sub = listen<OverlayPayload>("overlay-ready", ({ payload: p }) => {
      dragStartRef.current = null;
      curPosRef.current = null;
      setPayload(p);
    });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        void invoke("cancel_overlay");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      if (rafRef.current) {
        window.cancelAnimationFrame(rafRef.current);
        rafRef.current = 0;
      }
      void sub.then((u) => u()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    if (!payload) return;
    if (payload.mode === "color") {
      const img = new Image();
      img.onload = () => {
        const c = document.createElement("canvas");
        c.width = img.width;
        c.height = img.height;
        const ctx = c.getContext("2d", { willReadFrequently: true });
        if (ctx) {
          ctx.drawImage(img, 0, 0);
          sampleCtxRef.current = ctx;
          sampleSizeRef.current = { w: img.width, h: img.height };
        }
      };
      img.src = payload.previewDataUrl;
      return;
    }

    drawOverlay();
    const onResize = () => scheduleDraw();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      if (rafRef.current) {
        window.cancelAnimationFrame(rafRef.current);
        rafRef.current = 0;
      }
    };
  }, [payload, drawOverlay, scheduleDraw]);

  if (!payload) {
    return <div style={{ width: "100vw", height: "100vh", background: "#080808" }} />;
  }

  const isColor = payload.mode === "color";

  const computeRect = (a: Point, b: Point): Rect => ({
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(b.x - a.x),
    h: Math.abs(b.y - a.y),
  });

  return (
    <div
      style={{
        position: "relative",
        width: "100vw",
        height: "100vh",
        overflow: "hidden",
        cursor: "crosshair",
        userSelect: "none",
        touchAction: "none",
        background: "#080808",
      }}
      onPointerDown={(e) => {
        if (e.button === 2) {
          dragStartRef.current = null;
          void invoke("cancel_overlay");
          return;
        }
        if (e.button !== 0) return;
        if (isColor) {
          void invoke("complete_overlay_color", {
            xRatio: e.clientX / Math.max(1, window.innerWidth),
            yRatio: e.clientY / Math.max(1, window.innerHeight),
          });
          return;
        }
        e.currentTarget.setPointerCapture(e.pointerId);
        const pt = { x: e.clientX, y: e.clientY };
        dragStartRef.current = pt;
        curPosRef.current = pt;
        scheduleDraw();
      }}
      onPointerMove={(e) => {
        curPosRef.current = { x: e.clientX, y: e.clientY };
        scheduleDraw();
      }}
      onPointerUp={(e) => {
        if (isColor || !dragStartRef.current) return;
        if (e.currentTarget.hasPointerCapture(e.pointerId)) {
          e.currentTarget.releasePointerCapture(e.pointerId);
        }
        const rect = computeRect(dragStartRef.current, { x: e.clientX, y: e.clientY });
        dragStartRef.current = null;
        if (rect.w < 4 || rect.h < 4) {
          scheduleDraw();
          return;
        }
        if (rafRef.current) {
          window.cancelAnimationFrame(rafRef.current);
          rafRef.current = 0;
        }
        void invoke("complete_overlay_region", {
          xRatio: rect.x / Math.max(1, window.innerWidth),
          yRatio: rect.y / Math.max(1, window.innerHeight),
          wRatio: rect.w / Math.max(1, window.innerWidth),
          hRatio: rect.h / Math.max(1, window.innerHeight),
        });
      }}
      onPointerCancel={() => {
        dragStartRef.current = null;
        scheduleDraw();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        dragStartRef.current = null;
        void invoke("cancel_overlay");
      }}
    >
      <img
        src={payload.previewDataUrl}
        alt=""
        draggable={false}
        style={{
          width: "100%",
          height: "100%",
          objectFit: "fill",
          display: "block",
          pointerEvents: "none",
        }}
      />

      {!isColor && (
        <canvas
          ref={canvasRef}
          style={{
            position: "fixed",
            inset: 0,
            width: "100vw",
            height: "100vh",
            pointerEvents: "none",
          }}
        />
      )}

      {isColor && (
        <div
          ref={colorBadgeRef}
          style={{
            position: "fixed",
            left: 0,
            top: 0,
            opacity: 0,
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
            willChange: "transform",
          }}
        >
          <span
            ref={colorSwatchRef}
            style={{
              width: 14,
              height: 14,
              borderRadius: 4,
              background: "#000000",
              border: "1px solid #444",
            }}
          />
          <span ref={colorTextRef}>#000000</span>
        </div>
      )}

      <div
        style={{
          position: "fixed",
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
        {isColor
          ? "Click to copy pixel color · Esc to cancel"
          : "Drag to select region · Esc to cancel"}
      </div>
    </div>
  );
}
