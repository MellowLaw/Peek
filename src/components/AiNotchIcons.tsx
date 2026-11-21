import type { AiProviderId } from "../types/peek";

export type PinnedAiItem = {
  id: AiProviderId;
  name: string;
  percent: number;
  tooltip: string;
};

export function AiProviderLogo({
  id,
  size = 14,
}: {
  id: AiProviderId | string;
  size?: number;
}) {
  if (id === "claude") {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        aria-hidden="true"
        className="ai-brand-logo claude"
      >
        <g stroke="#E2956A" strokeWidth="2.1" strokeLinecap="round">
          <line x1="12" y1="3" x2="12" y2="9" />
          <line x1="12" y1="15" x2="12" y2="21" />
          <line x1="3" y1="12" x2="9" y2="12" />
          <line x1="15" y1="12" x2="21" y2="12" />
          <line x1="5.6" y1="5.6" x2="9.8" y2="9.8" />
          <line x1="14.2" y1="14.2" x2="18.4" y2="18.4" />
          <line x1="18.4" y1="5.6" x2="14.2" y2="9.8" />
          <line x1="9.8" y1="14.2" x2="5.6" y2="18.4" />
        </g>
        <circle cx="12" cy="12" r="2.1" fill="#E2956A" />
      </svg>
    );
  }

  if (id === "gemini") {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        aria-hidden="true"
        className="ai-brand-logo gemini"
      >
        <path
          d="M12 2.4C12.95 7.75 16.25 11.05 21.6 12C16.25 12.95 12.95 16.25 12 21.6C11.05 16.25 7.75 12.95 2.4 12C7.75 11.05 11.05 7.75 12 2.4Z"
          fill="#8BB8FF"
        />
      </svg>
    );
  }

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      className="ai-brand-logo codex"
    >
      <g stroke="#74D8B8" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round">
        {[0, 60, 120, 180, 240, 300].map((angle) => (
          <path
            key={angle}
            d="M12 5.2C14.2 5.2 15.8 6.8 15.8 9L15.8 12.8C15.8 13.7 15.1 14.4 14.2 14.4C13.3 14.4 12.6 13.7 12.6 12.8L12.6 8.2C12.6 6.5 11.2 5.2 9.5 5.2"
            transform={`rotate(${angle} 12 12)`}
          />
        ))}
      </g>
    </svg>
  );
}

export function UsageRing({
  percent,
  size = 16,
  strokeWidth = 2.2,
}: {
  percent: number;
  size?: number;
  strokeWidth?: number;
}) {
  const clamped = Math.min(100, Math.max(0, Number.isFinite(percent) ? percent : 0));
  const r = 7.2;
  const circ = 2 * Math.PI * r;
  const offset = circ * (1 - clamped / 100);
  const stroke = clamped >= 50 ? "#62a85a" : clamped >= 20 ? "#e5b962" : "#e67e7e";

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
      className="usage-ring"
    >
      <circle
        cx="10"
        cy="10"
        r={r}
        stroke="rgba(255, 255, 255, 0.18)"
        strokeWidth={strokeWidth}
        fill="none"
      />
      <circle
        cx="10"
        cy="10"
        r={r}
        stroke={stroke}
        strokeWidth={strokeWidth}
        strokeLinecap="round"
        strokeDasharray={circ}
        strokeDashoffset={offset}
        transform="rotate(-90 10 10)"
        fill="none"
        className="usage-ring-arc"
      />
    </svg>
  );
}

export function PinnedAiChip({
  item,
  onOpenAi,
  compact = false,
}: {
  item: PinnedAiItem;
  onOpenAi: () => void;
  compact?: boolean;
}) {
  return (
    <button
      type="button"
      className="pinned-ai-item"
      title={item.tooltip}
      aria-label={item.tooltip}
      onClick={(e) => {
        e.stopPropagation();
        onOpenAi();
      }}
    >
      <AiProviderLogo id={item.id} size={compact ? 12 : 13} />
      <UsageRing percent={item.percent} size={compact ? 15 : 16} strokeWidth={2.2} />
    </button>
  );
}

export function PinnedAiStrip({
  items,
  onOpenAi,
  compact = false,
  className = "",
}: {
  items: PinnedAiItem[];
  onOpenAi: () => void;
  compact?: boolean;
  className?: string;
}) {
  if (items.length === 0) return null;
  return (
    <span className={"pinned-ai-strip" + (className ? ` ${className}` : "")}>
      {items.map((item) => (
        <PinnedAiChip key={item.id} item={item} onOpenAi={onOpenAi} compact={compact} />
      ))}
    </span>
  );
}
