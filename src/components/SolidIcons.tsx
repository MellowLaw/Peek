export function SolidPrevIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <g transform="rotate(180 12 12)">
        <path d="M4.5 4.5 C 3.5 4 2.5 4.7 2.5 5.8 V 18.2 C 2.5 19.3 3.5 20 4.5 19.5 L 12.5 13.3 C 13.3 12.7 13.3 11.3 12.5 10.7 Z" />
        <path d="M13.5 4.5 C 12.5 4 11.5 4.7 11.5 5.8 V 18.2 C 11.5 19.3 12.5 20 13.5 19.5 L 21.5 13.3 C 22.3 12.7 22.3 11.3 21.5 10.7 Z" />
      </g>
    </svg>
  );
}

export function SolidNextIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M4.5 4.5 C 3.5 4 2.5 4.7 2.5 5.8 V 18.2 C 2.5 19.3 3.5 20 4.5 19.5 L 12.5 13.3 C 13.3 12.7 13.3 11.3 12.5 10.7 Z" />
      <path d="M13.5 4.5 C 12.5 4 11.5 4.7 11.5 5.8 V 18.2 C 11.5 19.3 12.5 20 13.5 19.5 L 21.5 13.3 C 22.3 12.7 22.3 11.3 21.5 10.7 Z" />
    </svg>
  );
}

export function SolidPlayIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M7.2 4.3 C 5.8 3.5 4.2 4.5 4.2 6.1 V 17.9 C 4.2 19.5 5.8 20.5 7.2 19.7 L 19.3 13.4 C 20.8 12.6 20.8 11.4 19.3 10.6 Z" />
    </svg>
  );
}

export function SolidPauseIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <rect x="4.5" y="3.5" width="5.2" height="17" rx="2.5" />
      <rect x="14.3" y="3.5" width="5.2" height="17" rx="2.5" />
    </svg>
  );
}

export function SolidStopIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <rect x="4.5" y="4.5" width="15" height="15" rx="3.4" />
    </svg>
  );
}

export function SolidRepeatIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M19.2 12 A7.2 7.2 0 1 1 16.9 6.7"
        stroke="currentColor"
        strokeWidth="3.1"
        strokeLinecap="round"
      />
      <path
        d="M18.6 3.3 C 19.4 3.1 20.2 3.7 20.2 4.6 V 9.4 C 20.2 10.2 19.5 10.8 18.7 10.7 L 13.9 10.1 C 13.1 10.0 12.7 9.0 13.3 8.4 Z"
        fill="currentColor"
      />
    </svg>
  );
}
