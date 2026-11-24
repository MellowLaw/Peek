import { useEffect, useRef, useState } from "react";

export function SmoothSlider({
  value,
  min,
  max,
  step = 1,
  disabled = false,
  ariaLabel,
  onChange,
  onCommit,
  onDragStart,
  onDragEnd,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  ariaLabel: string;
  onChange: (value: number) => void;
  onCommit?: (value: number) => void;
  onDragStart?: () => void;
  onDragEnd?: () => void;
}) {
  const railRef = useRef<HTMLDivElement | null>(null);
  const draggingRef = useRef(false);
  const [dragging, setDragging] = useState(false);

  const safeStep = step > 0 ? step : 1;
  const span = Math.max(1e-6, max - min);

  const snapValue = (raw: number) => {
    const clampedRaw = Math.max(min, Math.min(max, raw));
    const stepped = Math.round((clampedRaw - min) / safeStep) * safeStep + min;
    const rounded = Math.round(stepped * 1000) / 1000;
    return Math.max(min, Math.min(max, rounded));
  };

  const clamped = snapValue(value);
  const latestValRef = useRef(clamped);
  useEffect(() => {
    if (!draggingRef.current) {
      latestValRef.current = clamped;
    }
  }, [clamped]);

  const pct = Math.max(0, Math.min(100, ((clamped - min) / span) * 100));

  const valueFromClientX = (clientX: number) => {
    const el = railRef.current;
    if (!el) return latestValRef.current;
    const rect = el.getBoundingClientRect();
    const usable = Math.max(1, rect.width);
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / usable));
    return snapValue(min + ratio * span);
  };

  const stepBy = (deltaSteps: number) => {
    if (disabled) return;
    const next = snapValue(clamped + deltaSteps * safeStep);
    if (next !== clamped) {
      latestValRef.current = next;
      onChange(next);
      onCommit?.(next);
    }
  };

  return (
    <div
      ref={railRef}
      className={
        "smooth-slider" +
        (dragging ? " is-dragging" : "") +
        (disabled ? " is-disabled" : "")
      }
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label={ariaLabel}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={clamped}
      aria-disabled={disabled}
      onPointerDown={(e) => {
        if (disabled || e.button !== 0) return;
        e.preventDefault();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        draggingRef.current = true;
        setDragging(true);
        onDragStart?.();
        const next = valueFromClientX(e.clientX);
        if (next !== latestValRef.current) {
          latestValRef.current = next;
          onChange(next);
        }
      }}
      onPointerMove={(e) => {
        if (!draggingRef.current || disabled) return;
        const next = valueFromClientX(e.clientX);
        if (next !== latestValRef.current) {
          latestValRef.current = next;
          onChange(next);
        }
      }}
      onPointerUp={(e) => {
        if (!draggingRef.current) return;
        const target = e.currentTarget as HTMLElement;
        if (target.hasPointerCapture(e.pointerId)) {
          target.releasePointerCapture(e.pointerId);
        }
        const finalVal = valueFromClientX(e.clientX);
        if (finalVal !== latestValRef.current) {
          latestValRef.current = finalVal;
          onChange(finalVal);
        }
        draggingRef.current = false;
        setDragging(false);
        onDragEnd?.();
        onCommit?.(finalVal);
      }}
      onPointerCancel={() => {
        if (!draggingRef.current) return;
        draggingRef.current = false;
        setDragging(false);
        onDragEnd?.();
        onCommit?.(latestValRef.current);
      }}
      onKeyDown={(e) => {
        if (disabled) return;
        if (e.key === "ArrowLeft" || e.key === "ArrowDown") {
          e.preventDefault();
          stepBy(-1);
        } else if (e.key === "ArrowRight" || e.key === "ArrowUp") {
          e.preventDefault();
          stepBy(1);
        } else if (e.key === "PageDown") {
          e.preventDefault();
          stepBy(-5);
        } else if (e.key === "PageUp") {
          e.preventDefault();
          stepBy(5);
        } else if (e.key === "Home") {
          e.preventDefault();
          if (min !== clamped) {
            latestValRef.current = min;
            onChange(min);
            onCommit?.(min);
          }
        } else if (e.key === "End") {
          e.preventDefault();
          if (max !== clamped) {
            latestValRef.current = max;
            onChange(max);
            onCommit?.(max);
          }
        }
      }}
      onWheel={(e) => {
        if (disabled) return;
        e.preventDefault();
        stepBy(e.deltaY < 0 ? 1 : -1);
      }}
    >
      <div className="smooth-slider-track">
        <div className="smooth-slider-fill" style={{ width: `${pct}%` }} />
        <div className="smooth-slider-thumb" style={{ left: `${pct}%` }} />
      </div>
    </div>
  );
}
