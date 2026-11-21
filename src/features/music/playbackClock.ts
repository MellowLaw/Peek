export type ClockSyncInput = {
  songId: string;
  durationSeconds: number;
  offsetMs: number;
  positionSeconds: number;
  sampledAtMs: number;
  playing: boolean;
  rate: number;
};

/**
 * Projects timestamped Windows playback snapshots forward using a monotonic clock,
 * preserving steady lyric pacing by absorbing <=300ms sample jitter.
 */
export class PlaybackClock {
  position = 0;
  duration = 0;
  rate = 1;
  playing = false;
  offset = 0;
  songId = "";
  private anchor = performance.now();

  reset() {
    this.position = 0;
    this.duration = 0;
    this.rate = 1;
    this.playing = false;
    this.offset = 0;
    this.songId = "";
    this.anchor = performance.now();
  }

  sync(data: ClockSyncInput, preserveSmallCorrections = true) {
    const previousPosition = this.read();
    const previousPlaying = this.playing;
    const previousRate = this.rate;
    const previousSong = this.songId;

    this.songId = data.songId;
    this.rate = Number.isFinite(data.rate) && data.rate >= 0 ? data.rate : 1;
    this.playing = Boolean(data.playing);
    this.duration = Math.max(0, Number(data.durationSeconds) || 0);
    this.offset = (Number(data.offsetMs) || 0) / 1000;

    const age =
      Number.isFinite(data.sampledAtMs) && data.sampledAtMs > 0
        ? Math.max(0, (Date.now() - data.sampledAtMs) / 1000)
        : 0;
    this.position = Math.max(
      0,
      (Number(data.positionSeconds) || 0) + (this.playing ? age * this.rate : 0),
    );

    if (
      preserveSmallCorrections &&
      previousSong === this.songId &&
      previousPlaying &&
      this.playing &&
      previousRate === this.rate &&
      Math.abs(this.position - previousPosition) <= 0.3
    ) {
      this.position = previousPosition;
    }
    this.anchor = performance.now();
  }

  seek(positionSeconds: number) {
    this.position = Math.max(0, positionSeconds);
    this.anchor = performance.now();
  }

  read(): number {
    const elapsed = this.playing ? ((performance.now() - this.anchor) / 1000) * this.rate : 0;
    const pos = Math.max(0, this.position + elapsed);
    return this.duration > 0 ? Math.min(this.duration, pos) : pos;
  }

  lyricsTime(): number {
    return this.read() + this.offset;
  }
}
