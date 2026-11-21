import { Minus, Plus } from "lucide-react";

export function NumberStepper({
  value,
  min,
  max,
  step = 1,
  suffix = "",
  ariaLabel,
  disabled = false,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  suffix?: string;
  ariaLabel: string;
  disabled?: boolean;
  onChange: (next: number) => void;
}) {
  const clamp = (n: number) => {
    if (!Number.isFinite(n)) return min;
    return Math.min(max, Math.max(min, Math.round(n)));
  };

  return (
    <div className="num-stepper" aria-label={ariaLabel}>
      <button
        type="button"
        className="stepper-btn"
        aria-label={`Decrease ${ariaLabel}`}
        disabled={disabled || value <= min}
        onClick={() => onChange(clamp(value - step))}
      >
        <Minus size={11} />
      </button>
      <div className="stepper-val-wrap">
        <input
          type="text"
          inputMode="numeric"
          aria-label={ariaLabel}
          className="stepper-input"
          disabled={disabled}
          value={String(value)}
          onChange={(e) => {
            const raw = e.target.value.replace(/\D+/g, "");
            if (!raw) {
              onChange(min);
              return;
            }
            onChange(clamp(Number(raw)));
          }}
        />
        {suffix && <span className="stepper-suffix">{suffix}</span>}
      </div>
      <button
        type="button"
        className="stepper-btn"
        aria-label={`Increase ${ariaLabel}`}
        disabled={disabled || value >= max}
        onClick={() => onChange(clamp(value + step))}
      >
        <Plus size={11} />
      </button>
    </div>
  );
}
