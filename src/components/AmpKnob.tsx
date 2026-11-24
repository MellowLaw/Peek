import { useRef } from "react";

const MIN_DEG = -135;
const MAX_DEG = 135;
const SWEEP_DEG = MAX_DEG - MIN_DEG;

export function AmpKnob({
  value,
  muted,
  disabled,
  size = "md",
  ariaLabel = "Master volume amplifier dial",
  onChange,
  onDragStart,
  onDragEnd,
  onToggleMute,
}: {
  value: number;
  muted: boolean;
  disabled?: boolean;
  size?: "md" | "sm";
  ariaLabel?: string;
  onChange: (next: number) => void;
  onDragStart?: () => void;
  onDragEnd?: () => void;
  onToggleMute?: () => void;
}) {
  const isSm = size === "sm";
  const tickCount = isSm ? 19 : 27;
  const snapDist = isSm ? 9 : 14;
  const moveDist = isSm ? 5 : 8;
  const wheelStep = isSm ? 4 : 3;

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

  const dims = isSm
    ? { w: 74, h: 70, vb: "0 0 92 86", cx: 46, cy: 42, tickY: 6, tickW: 2.2, tickH: 7.2, knobR: 23.5, triTop: 21.5, triBot: 27.5, triHalf: 3, textY: 79, textSize: 10.5 }
    : { w: 112, h: 104, vb: "0 0 120 112", cx: 60, cy: 56, tickY: 7, tickW: 2.6, tickH: 9.5, knobR: 33.5, triTop: 27.5, triBot: 35.5, triHalf: 3.8, textY: 104, textSize: 11.5 };

  return (
    <div
      className={
        "amp-knob" +
        (isSm ? " is-sm" : "") +
        (disabled ? " is-disabled" : "") +
        (muted ? " is-muted" : "")
      }
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label={ariaLabel}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={clamped}
      aria-valuetext={muted ? "Muted" : `${clamped}%`}
      onPointerDown={(e) => {
        if (disabled || e.button !== 0) return;
        e.preventDefault();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        const polar = getPolar(e.clientX, e.clientY);
        if (polar && polar.dist >= snapDist && polar.deg >= -145 && polar.deg <= 145) {
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
            prevAngle: polar && polar.dist >= moveDist ? polar.deg : null,
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
        if (!polar || polar.dist < moveDist) return;
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
        const delta = e.deltaY < 0 ? wheelStep : -wheelStep;
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
      <svg
        ref={svgRef}
        viewBox={dims.vb}
        width={dims.w}
        height={dims.h}
        aria-hidden="true"
      >
        {Array.from({ length: tickCount }, (_, idx) => {
          const frac = idx / (tickCount - 1);
          const angle = MIN_DEG + frac * SWEEP_DEG;
          const active = !muted && clamped > 0 && frac <= activeRatio + 0.001;
          return (
            <rect
              key={idx}
              x={dims.cx - dims.tickW / 2}
              y={dims.tickY}
              width={dims.tickW}
              height={dims.tickH}
              rx={dims.tickW / 2}
              fill={active ? "#f4f4f4" : "#2f2f2f"}
              transform={`rotate(${angle} ${dims.cx} ${dims.cy})`}
            />
          );
        })}

        <circle
          cx={dims.cx}
          cy={dims.cy}
          r={dims.knobR}
          fill="#0a0a0a"
          stroke={muted ? "#343434" : "#f4f4f4"}
          strokeWidth={isSm ? "1.75" : "2"}
        />

        <g transform={`rotate(${knobAngle} ${dims.cx} ${dims.cy})`}>
          <path
            d={`M ${dims.cx} ${dims.triTop} L ${dims.cx + dims.triHalf} ${dims.triBot} L ${dims.cx - dims.triHalf} ${dims.triBot} Z`}
            fill={muted ? "#666666" : "#ffffff"}
          />
        </g>

        <text
          x={dims.cx}
          y={dims.textY}
          textAnchor="middle"
          fontSize={dims.textSize}
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
