import { useRef } from "react";

const TICK_COUNT = 27;
const MIN_DEG = -135;
const MAX_DEG = 135;
const SWEEP_DEG = MAX_DEG - MIN_DEG;

export function AmpKnob({
  value,
  muted,
  disabled,
  onChange,
  onDragStart,
  onDragEnd,
  onToggleMute,
}: {
  value: number;
  muted: boolean;
  disabled?: boolean;
  onChange: (next: number) => void;
  onDragStart?: () => void;
  onDragEnd?: () => void;
  onToggleMute?: () => void;
}) {
  const svgRef = useRef<SVGSVGElement | null>(null);
  const dragRef = useRef<{
    prevAngle: number | null;
    virtualDeg: number;
    lastEmitted: number;
  } | null>(null);
  const clamped = Math.max(0, Math.min(100, Math.round(value)));
  const activeRatio = clamped / 100;
  const knobAngle = MIN_DEG + activeRatio * SWEEP_DEG;

  function clampVal(v: number) {
    return Math.max(0, Math.min(100, Math.round(v)));
  }

  function clampDeg(d: number) {
    return Math.max(MIN_DEG, Math.min(MAX_DEG, d));
  }

  function degToVal(d: number) {
    return clampVal(((clampDeg(d) - MIN_DEG) / SWEEP_DEG) * 100);
  }

  function getPolar(clientX: number, clientY: number) {
    const rect = svgRef.current?.getBoundingClientRect();
    if (!rect || rect.width === 0 || rect.height === 0) return null;
    const cx = rect.left + rect.width * 0.5;
    const cy = rect.top + rect.height * 0.5;
    const dx = clientX - cx;
    const dy = clientY - cy;
    const dist = Math.hypot(dx, dy);
    const deg = (Math.atan2(dx, -dy) * 180) / Math.PI;
    return { dist, deg };
  }

  return (
    <div
      className={"amp-knob" + (disabled ? " is-disabled" : "") + (muted ? " is-muted" : "")}
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label="Master volume amplifier dial"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={clamped}
      aria-valuetext={muted ? "Muted" : `${clamped}%`}
      onPointerDown={(e) => {
        if (disabled || e.button !== 0) return;
        e.preventDefault();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        const polar = getPolar(e.clientX, e.clientY);
        if (polar && polar.dist >= 14 && polar.deg >= -145 && polar.deg <= 145) {
          const snapped = clampDeg(polar.deg);
          const nextVal = degToVal(snapped);
          if (nextVal !== clamped) onChange(nextVal);
          dragRef.current = {
            prevAngle: polar.deg,
            virtualDeg: snapped,
            lastEmitted: nextVal,
          };
        } else {
          dragRef.current = {
            prevAngle: polar && polar.dist >= 8 ? polar.deg : null,
            virtualDeg: MIN_DEG + (clamped / 100) * SWEEP_DEG,
            lastEmitted: clamped,
          };
        }
        onDragStart?.();
      }}
      onPointerMove={(e) => {
        const drag = dragRef.current;
        if (!drag || disabled) return;
        const polar = getPolar(e.clientX, e.clientY);
        if (!polar || polar.dist < 8) return;
        if (drag.prevAngle === null) {
          drag.prevAngle = polar.deg;
          return;
        }
        let delta = polar.deg - drag.prevAngle;
        if (delta > 180) delta -= 360;
        if (delta <= -180) delta += 360;
        drag.prevAngle = polar.deg;
        drag.virtualDeg = clampDeg(drag.virtualDeg + delta);
        const nextVal = degToVal(drag.virtualDeg);
        if (nextVal !== drag.lastEmitted) {
          drag.lastEmitted = nextVal;
          onChange(nextVal);
        }
      }}
      onPointerUp={(e) => {
        if (!dragRef.current) return;
        if ((e.currentTarget as HTMLElement).hasPointerCapture(e.pointerId)) {
          (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
        }
        dragRef.current = null;
        onDragEnd?.();
      }}
      onPointerCancel={() => {
        if (!dragRef.current) return;
        dragRef.current = null;
        onDragEnd?.();
      }}
      onDoubleClick={() => {
        if (!disabled) onToggleMute?.();
      }}
      onWheel={(e) => {
        if (disabled) return;
        e.preventDefault();
        const delta = e.deltaY < 0 ? 3 : -3;
        onChange(clampVal(clamped + delta));
      }}
      onKeyDown={(e) => {
        if (disabled) return;
        if (e.key === "ArrowUp" || e.key === "ArrowRight") {
          e.preventDefault();
          onChange(clampVal(clamped + 2));
        } else if (e.key === "ArrowDown" || e.key === "ArrowLeft") {
          e.preventDefault();
          onChange(clampVal(clamped - 2));
        } else if (e.key === " " || e.key === "Enter") {
          e.preventDefault();
          onToggleMute?.();
        }
      }}
    >
      <svg ref={svgRef} viewBox="0 0 120 112" width="112" height="104" aria-hidden="true">
        {Array.from({ length: TICK_COUNT }, (_, idx) => {
          const frac = idx / (TICK_COUNT - 1);
          const angle = MIN_DEG + frac * SWEEP_DEG;
          const active = !muted && clamped > 0 && frac <= activeRatio + 0.001;
          return (
            <rect
              key={idx}
              x="58.7"
              y="7"
              width="2.6"
              height="9.5"
              rx="1.3"
              fill={active ? "#f4f4f4" : "#2f2f2f"}
              transform={`rotate(${angle} 60 56)`}
            />
          );
        })}

        <circle
          cx="60"
          cy="56"
          r="33.5"
          fill="#0a0a0a"
          stroke={muted ? "#343434" : "#f4f4f4"}
          strokeWidth="2"
        />

        <g transform={`rotate(${knobAngle} 60 56)`}>
          <path d="M 60 27.5 L 63.8 35.5 L 56.2 35.5 Z" fill={muted ? "#666666" : "#ffffff"} />
        </g>

        <text
          x="60"
          y="104"
          textAnchor="middle"
          fontSize="11.5"
          fontWeight="700"
          fill={muted ? "#8f8f8f" : "#f4f4f4"}
          style={{ fontVariantNumeric: "tabular-nums" }}
        >
          {muted ? "MUTE" : `${clamped}%`}
        </text>
      </svg>
    </div>
  );
}
