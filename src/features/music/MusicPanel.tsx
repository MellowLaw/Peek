import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Music2, Settings } from "lucide-react";
import { PinnedAiStrip, type PinnedAiItem } from "../../components/AiNotchIcons";
import {
  SolidNextIcon,
  SolidPauseIcon,
  SolidPlayIcon,
  SolidPrevIcon,
} from "../../components/SolidIcons";
import { usePreferences } from "../../stores/preferences";
import { PlaybackClock } from "./playbackClock";

export type MediaSessionSummary = {
  sourceId: string;
  name: string;
  isCurrent: boolean;
};

export type Track = {
  sessionId: number;
  sourceId: string;
  title: string;
  artist: string;
  album: string;
  artwork: string | null;
  playing: boolean;
  canPlayPause: boolean;
  canPrevious: boolean;
  canNext: boolean;
  canSeek: boolean;
  positionSeconds: number;
  durationSeconds: number;
  rate: number;
  sampledAtMs: number;
  sessions: MediaSessionSummary[];
};

export type MediaSnapshot =
  | { status: "loading" | "empty" | "unavailable" }
  | { status: "ready"; track: Track };

export type LyricLine = {
  timeSeconds: number;
  text: string;
  words: { timeSeconds: number; text: string }[];
};

export type LyricalSnapshot =
  | { status: "disabled" }
  | { status: "offline" }
  | { status: "loading" }
  | { status: "mismatch"; reason: string }
  | { status: "noLyrics"; reason: string }
  | { status: "plain"; lines: string[] }
  | {
      status: "synced";
      songId: string;
      durationSeconds: number;
      lines: LyricLine[];
      offsetMs: number;
      positionSeconds: number;
      sampledAtMs: number;
      playing: boolean;
      rate: number;
    };

export type AlbumPalette = {
  c1: [number, number, number];
  c2: [number, number, number];
  deep: [number, number, number];
  earLeft: [number, number, number];
  earRight: [number, number, number];
  accentRgb: string;
};

type Action = "previous" | "playPause" | "next";

type DisplayLine =
  | { key: string; isIntroDots: true; timeSeconds: 0 }
  | {
      key: string;
      isIntroDots: false;
      timeSeconds: number;
      text: string;
      wordTokens: { chars: string[]; trailingSpace: boolean }[];
      letterTimings: { start: number; end: number }[] | null;
    };

type SparkParticle = {
  x: number;
  y: number;
  vx: number;
  vy: number;
  radius: number;
  opacity: number;
};

const DEFAULT_PALETTE: AlbumPalette = {
  c1: [34, 48, 66],
  c2: [19, 26, 40],
  deep: [14, 18, 28],
  earLeft: [24, 34, 52],
  earRight: [20, 28, 44],
  accentRgb: "180, 224, 255",
};

function shadeFromBlurredQuadrant(
  rSum: number,
  gSum: number,
  bSum: number,
  count: number,
  fallback: [number, number, number],
): [number, number, number] {
  if (count <= 0) return fallback;
  const avgR = Math.floor(rSum / count);
  const avgG = Math.floor(gSum / count);
  const avgB = Math.floor(bSum / count);
  const [h, s, l] = rgbToHsl(avgR, avgG, avgB);
  const nextS = s < 0.05 ? s : Math.min(0.85, s * 1.9);
  const nextL = Math.min(0.34, Math.max(0.1, l * 0.54));
  return hslToRgb(h, nextS, nextL);
}

let measureCanvasCtx: CanvasRenderingContext2D | null | undefined;
function measureSingleLineWidth(text: string): number {
  if (typeof document === "undefined") return text.length * 8;
  if (measureCanvasCtx === undefined) {
    measureCanvasCtx = document.createElement("canvas").getContext("2d");
  }
  const ctx = measureCanvasCtx;
  if (!ctx) return text.length * 8;
  ctx.font = '700 13px "Segoe UI Variable", "Segoe UI", sans-serif';
  return ctx.measureText(text).width;
}

function computeIdleLyricsWidth(text: string, isIntroDots: boolean): number {
  if (isIntroDots) return 96;
  const raw = measureSingleLineWidth(text);
  const padded = Math.ceil(raw * 0.97) + 60;
  const clamped = Math.max(116, Math.min(540, padded));
  return clamped % 2 === 0 ? clamped : clamped + 1;
}

function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  let h = 0;
  let s = 0;
  const l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    switch (max) {
      case rn:
        h = (gn - bn) / d + (gn < bn ? 6 : 0);
        break;
      case gn:
        h = (bn - rn) / d + 2;
        break;
      default:
        h = (rn - gn) / d + 4;
    }
    h /= 6;
  }
  return [h, s, l];
}

function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  const hue2rgb = (p: number, q: number, t: number) => {
    let tt = t;
    if (tt < 0) tt += 1;
    if (tt > 1) tt -= 1;
    if (tt < 1 / 6) return p + (q - p) * 6 * tt;
    if (tt < 1 / 2) return q;
    if (tt < 2 / 3) return p + (q - p) * (2 / 3 - tt) * 6;
    return p;
  };
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  return [
    Math.round(hue2rgb(p, q, h + 1 / 3) * 255),
    Math.round(hue2rgb(p, q, h) * 255),
    Math.round(hue2rgb(p, q, h - 1 / 3) * 255),
  ];
}

function makeVibrantAccent(r: number, g: number, b: number): string {
  const [h, rawS, rawL] = rgbToHsl(r, g, b);
  const s = rawS < 0.08 ? rawS : Math.max(rawS, 0.62);
  const l = Math.min(Math.max(rawL, 0.62), 0.76);
  const [rr, gg, bb] = hslToRgb(h, s, l);
  return `${rr}, ${gg}, ${bb}`;
}

function clampRichShade(
  rgb: [number, number, number],
  minL: number,
  maxL: number,
  satFloor = 0.28,
): [number, number, number] {
  const [h, s, l] = rgbToHsl(rgb[0], rgb[1], rgb[2]);
  const nextS = s < 0.06 ? s : Math.min(0.82, Math.max(s, satFloor));
  const nextL = Math.min(maxL, Math.max(minL, l));
  return hslToRgb(h, nextS, nextL);
}

export function useAlbumPalette(artwork: string | null): AlbumPalette {
  const [palette, setPalette] = useState<AlbumPalette>(DEFAULT_PALETTE);
  useEffect(() => {
    if (!artwork) {
      setPalette(DEFAULT_PALETTE);
      return;
    }
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (cancelled) return;
      try {
        const canvas = document.createElement("canvas");
        canvas.width = 36;
        canvas.height = 36;
        const ctx = canvas.getContext("2d");
        if (!ctx) return;
        ctx.drawImage(img, 0, 0, 36, 36);
        const data = ctx.getImageData(0, 0, 36, 36).data;
        const buckets: Record<string, { count: number; r: number; g: number; b: number }> = {};

        let avgR = 0;
        let avgG = 0;
        let avgB = 0;
        let avgN = 0;
        let tlR = 0;
        let tlG = 0;
        let tlB = 0;
        let tlN = 0;
        let trR = 0;
        let trG = 0;
        let trB = 0;
        let trN = 0;

        for (let i = 0; i < data.length; i += 4) {
          const r = data[i];
          const g = data[i + 1];
          const b = data[i + 2];
          if (data[i + 3] < 240) continue;
          const px = (i / 4) % 36;
          const py = Math.floor(i / 4 / 36);
          if (py >= 6 && py < 24) {
            if (px < 14) {
              tlR += r;
              tlG += g;
              tlB += b;
              tlN += 1;
            } else if (px >= 22) {
              trR += r;
              trG += g;
              trB += b;
              trN += 1;
            }
          }
          avgR += r;
          avgG += g;
          avgB += b;
          avgN += 1;

          const max = Math.max(r, g, b);
          const min = Math.min(r, g, b);
          if (max - min < 18 || max < 32 || min > 225) continue;
          const key = `${Math.round(r / 28)},${Math.round(g / 28)},${Math.round(b / 28)}`;
          const bkt = (buckets[key] ??= { count: 0, r: 0, g: 0, b: 0 });
          bkt.count += 1;
          bkt.r += r;
          bkt.g += g;
          bkt.b += b;
        }

        const sorted = Object.values(buckets)
          .sort((a, b) => b.count - a.count)
          .map<[number, number, number]>((b) => [
            Math.floor(b.r / b.count),
            Math.floor(b.g / b.count),
            Math.floor(b.b / b.count),
          ]);

        const fallback: [number, number, number] =
          avgN > 0 ? [Math.floor(avgR / avgN), Math.floor(avgG / avgN), Math.floor(avgB / avgN)] : [44, 52, 68];

        const rawPrimary = sorted[0] ?? fallback;
        let rawSecondary = sorted[0] ?? fallback;
        for (let i = 1; i < sorted.length; i++) {
          const c = sorted[i];
          const dist =
            Math.abs(c[0] - rawPrimary[0]) +
            Math.abs(c[1] - rawPrimary[1]) +
            Math.abs(c[2] - rawPrimary[2]);
          if (dist >= 75) {
            rawSecondary = c;
            break;
          }
        }
        if (rawSecondary === rawPrimary) {
          const [h, s, l] = rgbToHsl(rawPrimary[0], rawPrimary[1], rawPrimary[2]);
          rawSecondary = hslToRgb((h + 0.08) % 1, Math.min(0.8, s + 0.1), Math.max(0.14, l * 0.72));
        }

        const c1 = clampRichShade(rawPrimary, 0.22, 0.36, 0.42);
        const c2 = clampRichShade(rawSecondary, 0.14, 0.27, 0.35);
        const deep = clampRichShade(rawPrimary, 0.11, 0.19, 0.32);
        const earLeft = shadeFromBlurredQuadrant(tlR, tlG, tlB, tlN, deep);
        const earRight = shadeFromBlurredQuadrant(trR, trG, trB, trN, deep);
        const accentRgb = makeVibrantAccent(rawPrimary[0], rawPrimary[1], rawPrimary[2]);

        setPalette({ c1, c2, deep, earLeft, earRight, accentRgb });
      } catch {
        setPalette(DEFAULT_PALETTE);
      }
    };
    img.src = artwork;
    return () => {
      cancelled = true;
    };
  }, [artwork]);
  return palette;
}

function buildLetterTimings(line: LyricLine): { start: number; end: number }[] | null {
  if (!line.words || line.words.length < 2) return null;
  const timings: { start: number; end: number }[] = [];
  for (let s = 0; s < line.words.length; s++) {
    const seg = line.words[s];
    if (!seg.text) continue;
    const end = s + 1 < line.words.length ? line.words[s + 1].timeSeconds : seg.timeSeconds + 1.0;
    const dur = Math.max(0.001, end - seg.timeSeconds);
    const chars = Array.from(seg.text);
    for (let c = 0; c < chars.length; c++) {
      timings.push({
        start: seg.timeSeconds + dur * (c / chars.length),
        end: seg.timeSeconds + dur * ((c + 1) / chars.length),
      });
    }
  }
  if (timings.length === 0) return null;
  const fullChars = Array.from(line.text);
  if (timings.length !== fullChars.length) return null;
  return timings;
}

function buildDisplayLines(lines: LyricLine[]): DisplayLine[] {
  const result: DisplayLine[] = [];
  if (lines.length > 0 && lines[0].timeSeconds > 2) {
    result.push({ key: "intro-dots", isIntroDots: true, timeSeconds: 0 });
  }
  lines.forEach((line, idx) => {
    const words = line.text.split(" ");
    const wordTokens = words.map((word, wi) => ({
      chars: Array.from(word),
      trailingSpace: wi < words.length - 1,
    }));
    result.push({
      key: `${idx}-${line.timeSeconds}`,
      isIntroDots: false,
      timeSeconds: line.timeSeconds,
      text: line.text,
      wordTokens,
      letterTimings: buildLetterTimings(line),
    });
  });
  return result;
}

export function useMedia(active: boolean): MediaSnapshot {
  const [snapshot, setSnapshot] = useState<MediaSnapshot>({ status: "loading" });
  useEffect(() => {
    if (!active) {
      setSnapshot({ status: "loading" });
      return;
    }
    let disposed = false;
    const subscription = listen<MediaSnapshot>("media-update", ({ payload }) => {
      if (!disposed) setSnapshot(payload);
    });
    void subscription
      .then(() => {
        if (!disposed) return invoke("watch_media", { enabled: true });
      })
      .catch(() => {
        if (!disposed) setSnapshot({ status: "unavailable" });
      });
    return () => {
      disposed = true;
      void subscription.then((unlisten) => unlisten()).catch(console.error);
      void invoke("watch_media", { enabled: false }).catch(console.error);
    };
  }, [active]);
  return snapshot;
}

export function useLyrical(
  media: MediaSnapshot,
  active: boolean,
  enabled: boolean,
): LyricalSnapshot {
  const [lyrical, setLyrical] = useState<LyricalSnapshot>({ status: "loading" });
  const track = media.status === "ready" ? media.track : null;
  const title = track?.title ?? "";
  const artist = track?.artist ?? "";
  const album = track?.album ?? "";
  const durationSeconds = track?.durationSeconds ?? 0;

  useEffect(() => {
    if (!enabled) {
      setLyrical({ status: "disabled" });
      return;
    }
    if (!active || !track || !title.trim()) {
      setLyrical({ status: "loading" });
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let enrichChecks = 0;

    const poll = async () => {
      try {
        const res = await invoke<LyricalSnapshot>("get_lyrical_snapshot", {
          title,
          artist,
          album,
          durationSeconds,
        });
        if (cancelled) return;
        setLyrical(res);
        if (res.status === "loading") {
          timer = setTimeout(() => void poll(), 450);
        } else if (
          res.status === "synced" &&
          !res.lines.some((l) => l.words && l.words.length >= 2) &&
          enrichChecks < 3
        ) {
          enrichChecks += 1;
          timer = setTimeout(() => void poll(), 2200);
        }
      } catch {
        if (!cancelled) {
          setLyrical({ status: "offline" });
          timer = setTimeout(() => void poll(), 3000);
        }
      }
    };

    void poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [active, enabled, title, artist, album, durationSeconds, track?.sessionId]);

  return lyrical;
}

function Cover({ track }: { track: Track }) {
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFailed(false);
  }, [track.artwork]);
  return (
    <span className="music-cover-art" aria-hidden="true">
      {track.artwork && !failed ? (
        <img src={track.artwork} alt="" onError={() => setFailed(true)} />
      ) : (
        <Music2 size={24} strokeWidth={1.6} />
      )}
    </span>
  );
}

export function MusicPanel({
  snapshot,
  lyrical,
  accentRgb,
}: {
  snapshot: MediaSnapshot;
  lyrical: LyricalSnapshot;
  accentRgb?: string;
}) {
  if (snapshot.status === "ready") {
    return (
      <Player
        key={snapshot.track.sessionId}
        track={snapshot.track}
        lyrical={lyrical}
        accentRgbOverride={accentRgb}
      />
    );
  }
  const label = {
    loading: "Checking Windows media…",
    empty: "No media playing",
    unavailable: "Windows media unavailable",
  }[snapshot.status];
  return (
    <div className="empty-panel">
      <Music2 size={30} strokeWidth={1.5} />
      <span role="status">{label}</span>
    </div>
  );
}

function LyricalViewport({
  track,
  lyrical,
  accentRgb,
  onSeekLine,
  seekClockSignal,
  reduceMotion,
  singleLine = false,
  extraRightPx = 0,
  onTargetWidthChange,
}: {
  track: Track;
  lyrical: Extract<LyricalSnapshot, { status: "synced" }>;
  accentRgb: string;
  onSeekLine?: (timeSeconds: number) => void;
  seekClockSignal?: { seq: number; time: number } | null;
  reduceMotion: boolean;
  singleLine?: boolean;
  extraRightPx?: number;
  onTargetWidthChange?: (width: number) => void;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const lineRefs = useRef<(HTMLDivElement | null)[]>([]);
  const particlesRef = useRef<SparkParticle[]>([]);
  const clockRef = useRef(new PlaybackClock());
  const onTargetWidthChangeRef = useRef(onTargetWidthChange);
  onTargetWidthChangeRef.current = onTargetWidthChange;
  const lastActiveIdxRef = useRef(-999);
  const lastReportedWidthRef = useRef(-1);

  const displayLines = useMemo(() => buildDisplayLines(lyrical.lines), [lyrical.lines]);
  const linesKey = useMemo(
    () => `${lyrical.songId}:${lyrical.lines.length}:${lyrical.lines[0]?.text ?? ""}`,
    [lyrical.songId, lyrical.lines],
  );

  // Drive monotonic PlaybackClock from Peek's live Windows GSMTC track timing.
  useEffect(() => {
    clockRef.current.sync(
      {
        songId: `${lyrical.songId}:${track.sessionId}:${track.title}`,
        durationSeconds: track.durationSeconds || lyrical.durationSeconds,
        offsetMs: lyrical.offsetMs,
        positionSeconds: track.positionSeconds,
        sampledAtMs: track.sampledAtMs,
        playing: track.playing,
        rate: track.rate,
      },
      true,
    );
  }, [
    lyrical.songId,
    lyrical.durationSeconds,
    lyrical.offsetMs,
    track.sessionId,
    track.title,
    track.durationSeconds,
    track.positionSeconds,
    track.sampledAtMs,
    track.playing,
    track.rate,
  ]);

  useEffect(() => {
    if (seekClockSignal) {
      clockRef.current.seek(seekClockSignal.time);
    }
  }, [seekClockSignal]);

  useEffect(() => {
    particlesRef.current = [];
    lastActiveIdxRef.current = -999;
    lastReportedWidthRef.current = -1;
  }, [linesKey]);

  useEffect(() => {
    const viewport = viewportRef.current;
    const canvas = canvasRef.current;
    if (!viewport || !canvas) return;
    const updateSize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.max(1, Math.floor(viewport.clientWidth * dpr));
      canvas.height = Math.max(1, Math.floor(viewport.clientHeight * dpr));
    };
    updateSize();
    const ro = new ResizeObserver(updateSize);
    ro.observe(viewport);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    lastActiveIdxRef.current = -999;
    lastReportedWidthRef.current = -1;
    let rafId = 0;
    let frameCount = 0;

    const applyFrame = () => {
      const viewport = viewportRef.current;
      if (!viewport) return;
      const vpHeight = viewport.clientHeight || 144;
      const lyricPos = clockRef.current.lyricsTime();

      let activeIdx = -1;
      for (let i = 0; i < displayLines.length; i++) {
        if (lyricPos >= displayLines[i].timeSeconds) {
          activeIdx = i;
        } else {
          break;
        }
      }

      const displayIdx =
        singleLine && activeIdx === -1 && displayLines.length > 0 ? 0 : activeIdx;

      if (singleLine) {
        const activeEl = displayIdx >= 0 ? lineRefs.current[displayIdx] : null;
        const contentEl = activeEl?.querySelector<HTMLElement>(".lyrical-line-content") ?? null;
        const renderedW = contentEl?.offsetWidth ?? 0;
        if (
          displayIdx !== lastActiveIdxRef.current ||
          (lastReportedWidthRef.current <= 0 && renderedW > 0)
        ) {
          lastActiveIdxRef.current = displayIdx;
          const targetEntry = displayIdx >= 0 ? displayLines[displayIdx] : undefined;
          let nextW: number;
          if (targetEntry?.isIntroDots) {
            const rawIntro = 96 + extraRightPx;
            const clampedIntro = Math.min(540, rawIntro);
            nextW = clampedIntro % 2 === 0 ? clampedIntro : clampedIntro + 1;
          } else if (renderedW > 0) {
            const rawW = Math.ceil(renderedW) + 64 + extraRightPx;
            const clamped = Math.max(120, Math.min(540, rawW));
            nextW = clamped % 2 === 0 ? clamped : clamped + 1;
          } else {
            const rawFallback =
              computeIdleLyricsWidth(
                targetEntry && !targetEntry.isIntroDots ? targetEntry.text : "",
                !targetEntry,
              ) + extraRightPx;
            const clampedFallback = Math.min(540, rawFallback);
            nextW = clampedFallback % 2 === 0 ? clampedFallback : clampedFallback + 1;
          }
          const overflowThreshold = Math.max(120, 472 - extraRightPx);
          viewport.classList.toggle(
            "is-overflowing",
            !targetEntry?.isIntroDots && renderedW > overflowThreshold,
          );
          if (
            lastReportedWidthRef.current < 0 ||
            Math.abs(nextW - lastReportedWidthRef.current) >= 4
          ) {
            lastReportedWidthRef.current = nextW;
            onTargetWidthChangeRef.current?.(nextW);
          }
        }
      }

      let targetY = 0;
      if (!singleLine) {
        const activeElem = activeIdx >= 0 ? lineRefs.current[activeIdx] : null;
        if (activeElem) {
          targetY = activeElem.offsetTop - vpHeight / 2 + activeElem.clientHeight / 2;
        }
      }

      let activeLetterRect: { x: number; y: number; w: number; h: number } | null = null;
      const vpRect = viewport.getBoundingClientRect();

      for (let i = 0; i < displayLines.length; i++) {
        const el = lineRefs.current[i];
        if (!el) continue;

        const isCurrent = singleLine ? i === displayIdx : i === activeIdx;
        el.classList.toggle("active", isCurrent);
        el.classList.toggle("past", singleLine ? i < displayIdx : i < activeIdx);

        if (singleLine) {
          const near = displayIdx === -1 ? i === 0 : Math.abs(i - displayIdx) <= 1;
          el.style.visibility = near ? "visible" : "hidden";
          el.style.pointerEvents = isCurrent ? "auto" : "none";
          el.style.filter = "none";
          el.style.transitionDelay = "0s";
          if (i === displayIdx) {
            el.style.transform = "translateY(0px) scale(1)";
            el.style.opacity = "1";
          } else if (i < displayIdx) {
            el.style.transform = "translateY(-22px) scale(0.96)";
            el.style.opacity = "0";
          } else {
            el.style.transform = "translateY(22px) scale(0.96)";
            el.style.opacity = "0";
          }
        } else {
          el.style.visibility = "visible";
          el.style.pointerEvents = "auto";
          el.style.opacity = "";
          if (reduceMotion || activeIdx === -1 || i === activeIdx) {
            el.style.filter = "none";
          } else {
            const dist = Math.abs(i - activeIdx);
            el.style.filter = `blur(${Math.min(dist * 0.65, 2.2).toFixed(2)}px)`;
          }

          const cascadeDelay =
            !reduceMotion && activeIdx !== -1 && i > activeIdx
              ? Math.min((i - activeIdx) * 0.045, 0.36)
              : 0;
          if (i === activeIdx) {
            el.style.transform = `translateY(${(-targetY).toFixed(1)}px) scale(1.035)`;
          } else if (i < activeIdx) {
            el.style.transform = `translateY(${(-targetY - 6).toFixed(1)}px) scale(0.92)`;
          } else {
            el.style.transform = `translateY(${(-targetY + 8).toFixed(1)}px) scale(0.94)`;
          }
          el.style.transitionDelay = `0s, ${cascadeDelay.toFixed(3)}s, 0s, ${cascadeDelay.toFixed(3)}s`;
        }

        const entry = displayLines[i];
        if (entry.isIntroDots) continue;

        const letterSpans = el.querySelectorAll<HTMLSpanElement>(".lyrical-letter");
        if (i === activeIdx) {
          let exactChar = 0;
          const timings = entry.letterTimings;
          if (timings && timings.length === letterSpans.length) {
            exactChar = -1;
            for (let w = 0; w < timings.length; w++) {
              if (lyricPos < timings[w].start) {
                exactChar = w;
                break;
              }
              if (lyricPos < timings[w].end) {
                exactChar =
                  w +
                  (lyricPos - timings[w].start) / Math.max(0.001, timings[w].end - timings[w].start);
                break;
              }
            }
            if (exactChar === -1) {
              const lineEnd = timings[timings.length - 1].end;
              exactChar = timings.length + Math.min(5, (lyricPos - lineEnd) * 10);
            }
          } else {
            let crawlDuration = 4.0;
            if (i + 1 < displayLines.length) {
              crawlDuration = Math.max(0.1, displayLines[i + 1].timeSeconds - entry.timeSeconds);
            }
            const maxCrawl = Math.max(entry.text.length / 3.0, 2.5);
            crawlDuration = Math.min(crawlDuration, maxCrawl);
            const progress = Math.max(0, (lyricPos - entry.timeSeconds) / crawlDuration);
            exactChar = Math.min(progress * letterSpans.length, letterSpans.length + 5);
          }

          const activeLetterIdx = Math.floor(exactChar);
          const fraction = exactChar - activeLetterIdx;

          const liftPx = singleLine ? -0.6 : -0.9;
          const scaleAmp = singleLine ? 0.025 : 0.035;
          for (let w = 0; w < letterSpans.length; w++) {
            const sp = letterSpans[w];
            if (!reduceMotion) {
              const distFromSweep = Math.abs(w - exactChar);
              const influence = Math.max(0, 1 - distFromSweep / 3);
              const ease = influence * influence * (3 - 2 * influence);
              sp.style.transform =
                ease > 0.01
                  ? `translateY(${(liftPx * ease).toFixed(2)}px) scale(${(1 + scaleAmp * ease).toFixed(3)})`
                  : "";
            } else {
              sp.style.transform = "";
            }

            if (w < activeLetterIdx) {
              sp.style.color = "#ffffff";
              sp.style.textShadow = "0 0 16px rgba(255, 255, 255, 0.5)";
              sp.style.background = "";
              sp.style.backgroundClip = "";
              sp.style.webkitBackgroundClip = "";
              sp.style.webkitTextFillColor = "";
            } else if (w > activeLetterIdx) {
              sp.style.color = "rgba(255, 255, 255, 0.4)";
              sp.style.textShadow = "none";
              sp.style.background = "";
              sp.style.backgroundClip = "";
              sp.style.webkitBackgroundClip = "";
              sp.style.webkitTextFillColor = "";
            } else {
              sp.style.background = `linear-gradient(to right, #ffffff 0%, #ffffff 42%, rgba(${accentRgb}, 1) 50%, rgba(255, 255, 255, 0.4) 58%, rgba(255, 255, 255, 0.4) 100%)`;
              sp.style.backgroundSize = "200% 100%";
              sp.style.backgroundPosition = `${(100 - fraction * 100).toFixed(1)}% 0%`;
              sp.style.backgroundClip = "text";
              sp.style.webkitBackgroundClip = "text";
              sp.style.webkitTextFillColor = "transparent";
              sp.style.textShadow = `0 0 13px rgba(${accentRgb}, 0.42)`;

              if (
                !reduceMotion &&
                track.playing &&
                sp.textContent?.trim() &&
                (!timings || !timings[w] || (lyricPos >= timings[w].start && lyricPos < timings[w].end))
              ) {
                const r = sp.getBoundingClientRect();
                activeLetterRect = {
                  x: r.left - vpRect.left,
                  y: r.top - vpRect.top,
                  w: r.width,
                  h: r.height,
                };
              }
            }
          }
        } else {
          const dimColor =
            i < activeIdx ? "rgba(255, 255, 255, 0.18)" : "rgba(255, 255, 255, 0.4)";
          for (let w = 0; w < letterSpans.length; w++) {
            const sp = letterSpans[w];
            sp.style.color = dimColor;
            sp.style.textShadow = "none";
            sp.style.background = "";
            sp.style.backgroundClip = "";
            sp.style.webkitBackgroundClip = "";
            sp.style.webkitTextFillColor = "";
            sp.style.transform = "";
          }
        }
      }

      // Restrained word-only sparkle particles on the active lyric word (zero background particles).
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d");
      if (canvas && ctx) {
        const dpr = canvas.width / Math.max(1, viewport.clientWidth);
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        ctx.clearRect(0, 0, viewport.clientWidth, viewport.clientHeight);
        const list = particlesRef.current;

        if (!reduceMotion && track.playing) {
          frameCount += 1;
          const sparkInterval = singleLine ? 6 : 4;
          const maxSparks = singleLine ? 8 : 18;
          const baseRadius = singleLine ? 0.7 : 0.9;
          const extraRadius = singleLine ? 0.7 : 1.0;
          if (activeLetterRect && frameCount % sparkInterval === 0 && list.length < maxSparks) {
            list.push({
              x: activeLetterRect.x + Math.random() * Math.max(4, activeLetterRect.w),
              y: activeLetterRect.y + Math.random() * Math.max(4, activeLetterRect.h),
              vx: (Math.random() - 0.5) * 0.7,
              vy: -(0.35 + Math.random() * 0.75),
              radius: baseRadius + Math.random() * extraRadius,
              opacity: 0.85,
            });
          }
          for (let pIdx = list.length - 1; pIdx >= 0; pIdx--) {
            const p = list[pIdx];
            p.x += p.vx;
            p.y += p.vy;
            p.opacity -= 0.028;
            if (p.opacity <= 0) {
              list.splice(pIdx, 1);
              continue;
            }
            ctx.beginPath();
            ctx.arc(p.x, p.y, p.radius, 0, Math.PI * 2);
            ctx.shadowBlur = 8;
            ctx.shadowColor = `rgba(${accentRgb}, 0.75)`;
            ctx.fillStyle = `rgba(255, 255, 255, ${p.opacity.toFixed(3)})`;
            ctx.fill();
          }
          ctx.shadowBlur = 0;
        } else {
          particlesRef.current = [];
        }
      }
    };

    const tick = () => {
      applyFrame();
      if (track.playing) {
        rafId = requestAnimationFrame(tick);
      }
    };

    applyFrame();
    if (track.playing) {
      rafId = requestAnimationFrame(tick);
    }
    return () => cancelAnimationFrame(rafId);
  }, [displayLines, linesKey, track.playing, accentRgb, reduceMotion, singleLine, extraRightPx]);

  return (
    <div className={"lyrical-viewport" + (singleLine ? " single-line" : "")} ref={viewportRef}>
      <canvas ref={canvasRef} className="lyric-sparks-canvas" aria-hidden="true" />
      <div className="lyrical-container" key={linesKey}>
        {displayLines.map((line, index) => (
          <div
            key={line.key}
            ref={(el) => {
              lineRefs.current[index] = el;
            }}
            className={
              "lyrical-line" +
              (onSeekLine && track.canSeek && !line.isIntroDots ? " seekable" : "")
            }
            onClick={() => {
              if (onSeekLine && track.canSeek && !line.isIntroDots) {
                onSeekLine(line.timeSeconds);
              }
            }}
          >
            <span className="lyrical-line-content">
              {line.isIntroDots ? (
                <div className={"intro-dots" + (singleLine ? " compact" : "")} aria-hidden="true">
                  <span />
                  <span />
                  <span />
                </div>
              ) : (
                line.wordTokens.map((tok, wi) => (
                  <span key={wi} className="lyrical-word">
                    {tok.chars.map((ch, ci) => (
                      <span key={ci} className="lyrical-letter">
                        {ch}
                      </span>
                    ))}
                    {tok.trailingSpace && <span className="lyrical-letter lyrical-space"> </span>}
                  </span>
                ))
              )}
            </span>
          </div>
        ))}
        {!singleLine && <div className="lyrical-tail-spacer" aria-hidden="true" />}
      </div>
    </div>
  );
}

export function IdleLyricsBar({
  track,
  lyrical,
  accentRgb,
  reduceMotion,
  pinnedAiItems = [],
  onTargetWidthChange,
  onOpenMusic,
  onOpenAi,
}: {
  track: Track;
  lyrical: LyricalSnapshot;
  accentRgb: string;
  reduceMotion: boolean;
  pinnedAiItems?: PinnedAiItem[];
  onTargetWidthChange: (width: number) => void;
  onOpenMusic: () => void;
  onOpenAi?: () => void;
}) {
  const [imgFailed, setImgFailed] = useState(false);
  const fallbackLabel = track.artist
    ? `${track.title} · ${track.artist}`
    : track.title || "Playing";
  const extraRightPx = pinnedAiItems.length > 0 ? pinnedAiItems.length * 42 + 16 : 0;

  useEffect(() => {
    setImgFailed(false);
  }, [track.artwork]);

  useEffect(() => {
    if (lyrical.status === "synced") return;
    if (lyrical.status === "loading") {
      const w = Math.min(540, 96 + extraRightPx);
      onTargetWidthChange(w % 2 === 0 ? w : w + 1);
    } else {
      const w = Math.min(
        360 + extraRightPx,
        Math.min(540, computeIdleLyricsWidth(fallbackLabel, false) + extraRightPx),
      );
      onTargetWidthChange(w % 2 === 0 ? w : w + 1);
    }
  }, [lyrical.status, fallbackLabel, extraRightPx, onTargetWidthChange]);

  return (
    <div
      role="button"
      tabIndex={0}
      className={"idle-lyrics-bar" + (lyrical.status === "loading" ? " is-compact" : "")}
      onClick={onOpenMusic}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpenMusic();
        }
      }}
      aria-label="Open Music panel"
    >
      <span className="idle-lyrics-cover" aria-hidden="true">
        {track.artwork && !imgFailed ? (
          <img src={track.artwork} alt="" onError={() => setImgFailed(true)} />
        ) : (
          <Music2 size={12} strokeWidth={2} />
        )}
      </span>
      {lyrical.status === "synced" ? (
        <LyricalViewport
          track={track}
          lyrical={lyrical}
          accentRgb={accentRgb}
          reduceMotion={reduceMotion}
          singleLine
          extraRightPx={extraRightPx}
          onTargetWidthChange={onTargetWidthChange}
        />
      ) : lyrical.status === "loading" ? (
        <div className="intro-dots compact animating" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>
      ) : (
        <span className="idle-lyrics-fallback">{fallbackLabel}</span>
      )}
      {pinnedAiItems.length > 0 && onOpenAi && (
        <PinnedAiStrip items={pinnedAiItems} onOpenAi={onOpenAi} compact className="in-lyrics" />
      )}
    </div>
  );
}

function Player({
  track,
  lyrical,
  accentRgbOverride,
}: {
  track: Track;
  lyrical: LyricalSnapshot;
  accentRgbOverride?: string;
}) {
  const { preferences, ready, saving, save } = usePreferences();
  const [now, setNow] = useState(Date.now);
  const [pending, setPending] = useState(false);
  const [lyricsMenuOpen, setLyricsMenuOpen] = useState(false);
  const [scrubPos, setScrubPos] = useState<number | null>(null);
  const [seekSignal, setSeekSignal] = useState<{ seq: number; time: number } | null>(null);
  const [selectedSource, setSelectedSource] = useState<string>("");
  const [error, setError] = useState("");
  const trackBarRef = useRef<HTMLDivElement>(null);
  const lyricsPopoverWrapRef = useRef<HTMLDivElement>(null);
  const draggingSeekRef = useRef(false);
  const computedPalette = useAlbumPalette(track.artwork);
  const accentRgb = accentRgbOverride ?? computedPalette.accentRgb;

  useEffect(() => {
    if (!lyricsMenuOpen) return;
    const onPointerDown = (e: PointerEvent) => {
      if (
        lyricsPopoverWrapRef.current &&
        !lyricsPopoverWrapRef.current.contains(e.target as Node)
      ) {
        setLyricsMenuOpen(false);
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setLyricsMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [lyricsMenuOpen]);

  useEffect(() => {
    if (!track.playing) return;
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, [track.playing]);

  const elapsed = track.playing ? (Math.max(0, now - track.sampledAtMs) / 1000) * track.rate : 0;
  const computedPos = Math.min(track.durationSeconds, track.positionSeconds + elapsed);
  const position = scrubPos ?? computedPos;
  const pct =
    track.durationSeconds > 0
      ? Math.max(0, Math.min(100, (position / track.durationSeconds) * 100))
      : 0;

  const formatTime = (seconds: number) => {
    const total = Math.max(0, Math.floor(seconds));
    return Math.floor(total / 60) + ":" + String(total % 60).padStart(2, "0");
  };

  async function control(action: Action) {
    setPending(true);
    setError("");
    try {
      await invoke("control_media", { sessionId: track.sessionId, action });
    } catch {
      setError("Playback action unavailable");
    } finally {
      setPending(false);
    }
  }

  const commitSeek = useCallback(
    async (pos: number) => {
      const clamped = Math.max(0, Math.min(track.durationSeconds, pos));
      setScrubPos(null);
      setSeekSignal((prev) => ({ seq: (prev?.seq ?? 0) + 1, time: clamped }));
      try {
        await invoke("seek_media", { sessionId: track.sessionId, positionSeconds: clamped });
      } catch (e) {
        setError(String(e));
      }
    },
    [track.durationSeconds, track.sessionId],
  );

  const ratioToTime = (clientX: number): number => {
    const bar = trackBarRef.current;
    if (!bar || track.durationSeconds <= 0) return position;
    const rect = bar.getBoundingClientRect();
    const r = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
    return r * track.durationSeconds;
  };

  const subtitle = [track.artist, track.album].filter(Boolean).join(" · ");

  return (
    <section aria-label="Now playing" className="music-layout">
      <div className="music-left" ref={lyricsPopoverWrapRef}>
        {track.sessions.length > 1 && (
          <div className="media-source-row">
            <select
              aria-label="Media player session"
              value={selectedSource}
              onChange={(e) => {
                const val = e.target.value;
                setSelectedSource(val);
                void invoke("select_media_session", { sourceId: val });
              }}
            >
              <option value="">Active (Auto)</option>
              {track.sessions.map((s) => (
                <option key={s.sourceId} value={s.sourceId}>
                  {s.name}
                </option>
              ))}
            </select>
          </div>
        )}

        <div className="music-hero">
          <Cover key={track.artwork ?? track.title} track={track} />
          <div className="music-meta-controls">
            <div className="music-meta-text">
              <strong title={track.title}>{track.title || "Windows media"}</strong>
              {subtitle && <span title={subtitle}>{subtitle}</span>}
            </div>
            <div className="music-transport" aria-label="Playback controls">
              <button
                type="button"
                className="solid-media-btn"
                aria-label="Previous track"
                title="Previous"
                disabled={pending || !track.canPrevious}
                onClick={() => void control("previous")}
              >
                <SolidPrevIcon size={16} />
              </button>
              <button
                type="button"
                className="solid-media-btn play"
                aria-label={track.playing ? "Pause" : "Play"}
                title={track.playing ? "Pause" : "Play"}
                disabled={pending || !track.canPlayPause}
                onClick={() => void control("playPause")}
              >
                {track.playing ? <SolidPauseIcon size={18} /> : <SolidPlayIcon size={18} />}
              </button>
              <button
                type="button"
                className="solid-media-btn"
                aria-label="Next track"
                title="Next"
                disabled={pending || !track.canNext}
                onClick={() => void control("next")}
              >
                <SolidNextIcon size={16} />
              </button>
              <button
                type="button"
                className={
                  "music-lyrics-settings-btn" +
                  (lyricsMenuOpen ? " is-open" : "") +
                  (!preferences.lyricsEnabled || !preferences.idleLyricsEnabled ? " is-dim" : "")
                }
                aria-label="Lyrics settings"
                aria-expanded={lyricsMenuOpen}
                title="Lyrics settings"
                onClick={() => setLyricsMenuOpen((v) => !v)}
              >
                <Settings size={15} strokeWidth={1.75} />
              </button>
            </div>
          </div>
        </div>

        {lyricsMenuOpen && (
          <div className="music-lyrics-popover" role="dialog" aria-label="Lyrics settings">
            <button
              type="button"
              role="switch"
              aria-checked={preferences.idleLyricsEnabled}
              disabled={!ready || saving || !preferences.lyricsEnabled}
              className={
                "music-popover-row" + (preferences.idleLyricsEnabled ? " is-on" : "")
              }
              onClick={() => void save({ idleLyricsEnabled: !preferences.idleLyricsEnabled })}
            >
              <span>Notch lyrics</span>
              <span className="music-popover-pill" aria-hidden="true">
                {preferences.idleLyricsEnabled ? "On" : "Off"}
              </span>
            </button>
            <button
              type="button"
              role="switch"
              aria-checked={preferences.lyricsEnabled}
              disabled={!ready || saving}
              className={"music-popover-row" + (preferences.lyricsEnabled ? " is-on" : "")}
              onClick={() => void save({ lyricsEnabled: !preferences.lyricsEnabled })}
            >
              <span>Synced lyrics</span>
              <span className="music-popover-pill" aria-hidden="true">
                {preferences.lyricsEnabled ? "On" : "Off"}
              </span>
            </button>
          </div>
        )}

        {track.durationSeconds > 0 && (
          <div className="music-progress-row">
            <span>{formatTime(position)}</span>
            <div
              ref={trackBarRef}
              role={track.canSeek ? "slider" : "progressbar"}
              tabIndex={track.canSeek ? 0 : undefined}
              aria-label="Playback position"
              aria-valuemin={0}
              aria-valuemax={Math.round(track.durationSeconds)}
              aria-valuenow={Math.round(position)}
              className={"music-seek-pill" + (track.canSeek ? " seekable" : "")}
              onPointerDown={(e) => {
                if (!track.canSeek) return;
                e.currentTarget.setPointerCapture(e.pointerId);
                draggingSeekRef.current = true;
                setScrubPos(ratioToTime(e.clientX));
              }}
              onPointerMove={(e) => {
                if (!draggingSeekRef.current) return;
                setScrubPos(ratioToTime(e.clientX));
              }}
              onPointerUp={(e) => {
                if (!draggingSeekRef.current) return;
                draggingSeekRef.current = false;
                void commitSeek(ratioToTime(e.clientX));
              }}
              onKeyDown={(e) => {
                if (!track.canSeek) return;
                if (e.key === "ArrowLeft") {
                  e.preventDefault();
                  void commitSeek(position - 5);
                } else if (e.key === "ArrowRight") {
                  e.preventDefault();
                  void commitSeek(position + 5);
                }
              }}
            >
              <div className="music-seek-fill" style={{ width: `${pct.toFixed(2)}%` }} />
            </div>
            <span>{formatTime(track.durationSeconds)}</span>
          </div>
        )}

        {error && (
          <div className="media-error" role="status">
            {error}
          </div>
        )}
      </div>

      <div className="music-right" aria-label="Synced lyrics">
        {lyrical.status === "synced" ? (
          <LyricalViewport
            track={track}
            lyrical={lyrical}
            accentRgb={accentRgb}
            onSeekLine={(t) => void commitSeek(t)}
            seekClockSignal={seekSignal}
            reduceMotion={preferences.reducedMotion}
          />
        ) : lyrical.status === "plain" ? (
          <div className="lyrics-plain">
            {lyrical.lines.map((line, i) => (
              <div key={i}>{line}</div>
            ))}
          </div>
        ) : lyrical.status === "loading" ? (
          <div className="lyrical-status-box">
            <div className="intro-dots" aria-hidden="true">
              <span />
              <span />
              <span />
            </div>
            <span className="status-note">Looking for lyrics…</span>
          </div>
        ) : (
          <div className="lyrical-status-box">
            <span className="status-note">
              {lyrical.status === "disabled"
                ? "Lyrics off in Settings"
                : lyrical.status === "offline"
                  ? "Lyrics offline; retrying…"
                  : lyrical.status === "mismatch" || lyrical.status === "noLyrics"
                    ? lyrical.reason
                    : ""}
            </span>
          </div>
        )}
      </div>
    </section>
  );
}
