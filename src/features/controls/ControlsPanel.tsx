import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Battery,
  BatteryCharging,
  Cpu,
  ExternalLink,
  Maximize2,
  Mic,
  MicOff,
  PanelLeft,
  PanelRight,
  Square,
  Sun,
  Volume2,
  VolumeX,
} from "lucide-react";

type AudioDeviceInfo = { id: string; name: string; isDefault: boolean };
type AppAudioSession = { pid: number; name: string; volume: number; muted: boolean };
type AudioSnapshot = {
  masterAvailable: boolean;
  masterVolume: number;
  masterMuted: boolean;
  micAvailable: boolean;
  micVolume: number;
  micMuted: boolean;
  outputDevices: AudioDeviceInfo[];
  appSessions: AppAudioSession[];
};

type MasterAudioState = {
  volume: number;
  muted: boolean;
};

type BrightnessDisplay = {
  id: string;
  name: string;
  supported: boolean;
  level: number;
};

type SystemMetrics = {
  cpuPercent: number;
  ramUsedGb: number;
  ramTotalGb: number;
  ramPercent: number;
  batteryPresent: boolean;
  batteryPercent: number | null;
  batteryCharging: boolean;
  netDownKbps: number;
  netUpKbps: number;
  computerName: string;
  uptimeMinutes: number;
};

const WRITE_COOLDOWN_MS = 1600;

export function ControlsPanel() {
  const [audio, setAudio] = useState<AudioSnapshot | null>(null);
  const [displays, setDisplays] = useState<BrightnessDisplay[]>([]);
  const [metrics, setMetrics] = useState<SystemMetrics | null>(null);
  const [error, setError] = useState("");

  const draggingMasterRef = useRef(false);
  const draggingAppPidRef = useRef<number | null>(null);
  const draggingBrightnessIdRef = useRef<string | null>(null);

  const volBusyRef = useRef(false);
  const pendingVolRef = useRef<number | null>(null);
  const appBusyRef = useRef<Record<number, boolean>>({});
  const pendingAppVolRef = useRef<Record<number, number>>({});
  const brightBusyRef = useRef<Record<string, boolean>>({});
  const pendingBrightRef = useRef<Record<string, number>>({});

  const lastMasterWriteRef = useRef(0);
  const lastMicWriteRef = useRef(0);
  const lastAppWriteRef = useRef<Record<number, number>>({});
  const lastBrightWriteRef = useRef<Record<string, number>>({});

  const refreshAudio = useCallback(async () => {
    try {
      const startedAt = Date.now();
      const a = await invoke<AudioSnapshot>("get_audio_snapshot");
      const now = Date.now();
      setAudio((prev) => {
        if (!prev) return a;
        const holdMaster =
          draggingMasterRef.current ||
          volBusyRef.current ||
          pendingVolRef.current !== null ||
          lastMasterWriteRef.current >= startedAt ||
          now - lastMasterWriteRef.current < WRITE_COOLDOWN_MS;
        const holdMic =
          lastMicWriteRef.current >= startedAt ||
          now - lastMicWriteRef.current < WRITE_COOLDOWN_MS;

        return {
          ...a,
          masterVolume: holdMaster ? prev.masterVolume : a.masterVolume,
          masterMuted: holdMaster ? prev.masterMuted : a.masterMuted,
          micMuted: holdMic ? prev.micMuted : a.micMuted,
          appSessions: a.appSessions.map((s) => {
            const appWrite = lastAppWriteRef.current[s.pid] ?? 0;
            const holdApp =
              draggingAppPidRef.current === s.pid ||
              Boolean(appBusyRef.current[s.pid]) ||
              pendingAppVolRef.current[s.pid] !== undefined ||
              appWrite >= startedAt ||
              now - appWrite < WRITE_COOLDOWN_MS;
            if (holdApp) {
              const old = prev.appSessions.find((p) => p.pid === s.pid);
              return old ? { ...s, volume: old.volume, muted: old.muted } : s;
            }
            return s;
          }),
        };
      });
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const refreshBrightness = useCallback(async () => {
    try {
      const startedAt = Date.now();
      const next = await invoke<BrightnessDisplay[]>("get_brightness_displays");
      const now = Date.now();
      setDisplays((prev) =>
        next.map((d) => {
          const writeAt = lastBrightWriteRef.current[d.id] ?? 0;
          const hold =
            draggingBrightnessIdRef.current === d.id ||
            Boolean(brightBusyRef.current[d.id]) ||
            pendingBrightRef.current[d.id] !== undefined ||
            writeAt >= startedAt ||
            now - writeAt < WRITE_COOLDOWN_MS;
          if (hold) {
            const old = prev.find((p) => p.id === d.id);
            return old ? { ...d, level: old.level } : d;
          }
          return d;
        }),
      );
    } catch (e) {
      setError((prev) => prev || String(e));
    }
  }, []);

  const refreshMetrics = useCallback(async () => {
    await invoke<SystemMetrics>("get_system_metrics")
      .then(setMetrics)
      .catch(() => {});
  }, []);

  useEffect(() => {
    const clearDrags = () => {
      draggingMasterRef.current = false;
      draggingAppPidRef.current = null;
      draggingBrightnessIdRef.current = null;
    };
    window.addEventListener("pointerup", clearDrags);
    window.addEventListener("blur", clearDrags);
    return () => {
      window.removeEventListener("pointerup", clearDrags);
      window.removeEventListener("blur", clearDrags);
    };
  }, []);

  useEffect(() => {
    void refreshAudio();
    void refreshBrightness();
    void refreshMetrics();
    const audioTimer = setInterval(() => void refreshAudio(), 1500);
    const metricsTimer = setInterval(() => void refreshMetrics(), 2000);
    const brightTimer = setInterval(() => void refreshBrightness(), 4000);
    return () => {
      clearInterval(audioTimer);
      clearInterval(metricsTimer);
      clearInterval(brightTimer);
    };
  }, [refreshAudio, refreshBrightness, refreshMetrics]);

  async function flushMasterVolume() {
    if (volBusyRef.current) return;
    volBusyRef.current = true;
    try {
      while (pendingVolRef.current !== null) {
        const level = pendingVolRef.current;
        pendingVolRef.current = null;
        lastMasterWriteRef.current = Date.now();
        try {
          const applied = await invoke<MasterAudioState>("set_master_volume", { level });
          lastMasterWriteRef.current = Date.now();
          if (pendingVolRef.current === null && !draggingMasterRef.current) {
            setAudio((cur) =>
              cur ? { ...cur, masterVolume: applied.volume, masterMuted: applied.muted } : cur,
            );
          }
        } catch (e) {
          setError(String(e));
        }
      }
    } finally {
      volBusyRef.current = false;
    }
  }

  function updateMasterVolume(level: number) {
    setError("");
    lastMasterWriteRef.current = Date.now();
    setAudio((cur) =>
      cur
        ? {
            ...cur,
            masterVolume: level,
            masterMuted: level > 0 ? false : cur.masterMuted,
          }
        : cur,
    );
    pendingVolRef.current = level;
    void flushMasterVolume();
  }

  async function toggleMasterMute() {
    if (!audio) return;
    setError("");
    const muted = !audio.masterMuted;
    lastMasterWriteRef.current = Date.now();
    setAudio({ ...audio, masterMuted: muted });
    try {
      const applied = await invoke<MasterAudioState>("set_master_mute", { muted });
      lastMasterWriteRef.current = Date.now();
      setAudio((cur) =>
        cur ? { ...cur, masterVolume: applied.volume, masterMuted: applied.muted } : cur,
      );
    } catch (e) {
      setError(String(e));
    }
  }

  async function toggleMicMute() {
    if (!audio) return;
    setError("");
    const muted = !audio.micMuted;
    lastMicWriteRef.current = Date.now();
    setAudio({ ...audio, micMuted: muted });
    await invoke("set_mic_mute", { muted })
      .then(() => {
        lastMicWriteRef.current = Date.now();
      })
      .catch((e) => setError(String(e)));
  }

  async function changeOutput(deviceId: string) {
    setError("");
    await invoke("set_default_output_device", { deviceId })
      .then(() => refreshAudio())
      .catch((e) => setError(String(e)));
  }

  async function flushAppVolume(pid: number) {
    if (appBusyRef.current[pid]) return;
    appBusyRef.current[pid] = true;
    try {
      while (pendingAppVolRef.current[pid] !== undefined) {
        const volume = pendingAppVolRef.current[pid];
        delete pendingAppVolRef.current[pid];
        lastAppWriteRef.current[pid] = Date.now();
        await invoke("set_app_audio", { pid, volume, muted: null }).catch((e) =>
          setError(String(e)),
        );
        lastAppWriteRef.current[pid] = Date.now();
      }
    } finally {
      appBusyRef.current[pid] = false;
    }
  }

  function changeAppVolume(pid: number, volume: number) {
    setError("");
    lastAppWriteRef.current[pid] = Date.now();
    setAudio((cur) =>
      cur
        ? {
            ...cur,
            appSessions: cur.appSessions.map((s) => (s.pid === pid ? { ...s, volume } : s)),
          }
        : cur,
    );
    pendingAppVolRef.current[pid] = volume;
    void flushAppVolume(pid);
  }

  async function toggleAppMute(pid: number, currentMuted: boolean) {
    const muted = !currentMuted;
    lastAppWriteRef.current[pid] = Date.now();
    setAudio((cur) =>
      cur
        ? {
            ...cur,
            appSessions: cur.appSessions.map((s) => (s.pid === pid ? { ...s, muted } : s)),
          }
        : cur,
    );
    await invoke("set_app_audio", { pid, volume: null, muted })
      .then(() => {
        lastAppWriteRef.current[pid] = Date.now();
      })
      .catch((e) => setError(String(e)));
  }

  async function flushBrightness(id: string) {
    if (brightBusyRef.current[id]) return;
    brightBusyRef.current[id] = true;
    try {
      while (pendingBrightRef.current[id] !== undefined) {
        const level = pendingBrightRef.current[id];
        delete pendingBrightRef.current[id];
        lastBrightWriteRef.current[id] = Date.now();
        await invoke("set_display_brightness", { id, level }).catch((e) => setError(String(e)));
        lastBrightWriteRef.current[id] = Date.now();
      }
    } finally {
      brightBusyRef.current[id] = false;
    }
  }

  function updateBrightness(id: string, level: number) {
    setError("");
    lastBrightWriteRef.current[id] = Date.now();
    setDisplays((list) => list.map((d) => (d.id === id ? { ...d, level } : d)));
    pendingBrightRef.current[id] = level;
    void flushBrightness(id);
  }

  const defaultDeviceId = audio?.outputDevices.find((d) => d.isDefault)?.id ?? "";

  return (
    <section aria-label="System controls" className="controls-panel">
      {audio && (
        <div className="ui-section ctrl-section">
          <div className="ctrl-slider-row">
            <button
              type="button"
              className="btn sm"
              aria-label={audio.masterMuted ? "Unmute output" : "Mute output"}
              disabled={!audio.masterAvailable}
              onClick={() => void toggleMasterMute()}
            >
              {audio.masterMuted ? <VolumeX size={14} /> : <Volume2 size={14} />}
            </button>
            <input
              type="range"
              aria-label="Master volume"
              min={0}
              max={100}
              disabled={!audio.masterAvailable}
              value={audio.masterVolume}
              onPointerDown={() => {
                draggingMasterRef.current = true;
              }}
              onPointerUp={() => {
                draggingMasterRef.current = false;
              }}
              onPointerCancel={() => {
                draggingMasterRef.current = false;
              }}
              onChange={(e) => updateMasterVolume(Number(e.target.value))}
            />
            <span className="ctrl-val">{audio.masterVolume}%</span>
            <button
              type="button"
              className={"btn sm" + (audio.micMuted ? " danger" : "")}
              aria-label={audio.micMuted ? "Unmute microphone" : "Mute microphone"}
              title={audio.micMuted ? "Microphone muted" : "Mute microphone"}
              disabled={!audio.micAvailable}
              onClick={() => void toggleMicMute()}
            >
              {audio.micMuted ? <MicOff size={14} /> : <Mic size={14} />}
            </button>
          </div>

          {audio.outputDevices.length > 0 && (
            <div className="ctrl-row">
              <span className="status-note">Output</span>
              <select
                aria-label="Audio output device"
                value={defaultDeviceId}
                onChange={(e) => void changeOutput(e.target.value)}
              >
                {audio.outputDevices.map((d) => (
                  <option key={d.id} value={d.id}>
                    {d.name}
                  </option>
                ))}
              </select>
            </div>
          )}
        </div>
      )}

      {displays.length > 0 && (
        <div className="ui-section ctrl-section">
          {displays.map((d) => (
            <div key={d.id} className="ctrl-slider-row">
              <Sun size={14} className="status-note" />
              <span className="ctrl-name truncate" title={d.name}>
                {d.name}
              </span>
              {d.supported ? (
                <>
                  <input
                    type="range"
                    aria-label={"Brightness for " + d.name}
                    min={0}
                    max={100}
                    value={d.level}
                    onPointerDown={() => {
                      draggingBrightnessIdRef.current = d.id;
                    }}
                    onPointerUp={() => {
                      draggingBrightnessIdRef.current = null;
                    }}
                    onPointerCancel={() => {
                      draggingBrightnessIdRef.current = null;
                    }}
                    onChange={(e) => updateBrightness(d.id, Number(e.target.value))}
                  />
                  <span className="ctrl-val">{d.level}%</span>
                </>
              ) : (
                <span className="status-note">Unsupported</span>
              )}
            </div>
          ))}
        </div>
      )}

      {audio && audio.appSessions.length > 0 && (
        <div className="ui-section ctrl-section">
          <span className="section-heading">App mixer</span>
          <div className="app-mixer-list">
            {audio.appSessions.map((s) => (
              <div key={s.pid} className="ctrl-slider-row">
                <button
                  type="button"
                  className="btn sm"
                  aria-label={"Mute " + s.name}
                  onClick={() => void toggleAppMute(s.pid, s.muted)}
                >
                  {s.muted ? <VolumeX size={12} /> : <Volume2 size={12} />}
                </button>
                <span className="ctrl-app-name truncate" title={s.name}>
                  {s.name}
                </span>
                <input
                  type="range"
                  aria-label={"Volume for " + s.name}
                  min={0}
                  max={100}
                  value={s.volume}
                  onPointerDown={() => {
                    draggingAppPidRef.current = s.pid;
                  }}
                  onPointerUp={() => {
                    draggingAppPidRef.current = null;
                  }}
                  onPointerCancel={() => {
                    draggingAppPidRef.current = null;
                  }}
                  onChange={(e) => changeAppVolume(s.pid, Number(e.target.value))}
                />
                <span className="ctrl-val">{s.volume}%</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {metrics && (
        <div className="ui-section metrics-grid">
          <div className="metric-box">
            <Cpu size={13} />
            <span>CPU {metrics.cpuPercent.toFixed(0)}%</span>
          </div>
          <div className="metric-box">
            <span>
              RAM {metrics.ramUsedGb}/{metrics.ramTotalGb}G
            </span>
          </div>
          <div className="metric-box">
            {metrics.batteryPresent ? (
              <>
                {metrics.batteryCharging ? <BatteryCharging size={13} /> : <Battery size={13} />}
                <span>{metrics.batteryPercent}%</span>
              </>
            ) : (
              <span>AC Power</span>
            )}
          </div>
          <div className="metric-box">
            <span>
              ↓{metrics.netDownKbps.toFixed(0)} ↑{metrics.netUpKbps.toFixed(0)} KB/s
            </span>
          </div>
        </div>
      )}

      <div className="ui-section ctrl-section">
        <div className="ctrl-row">
          <span className="status-note">Snap active window</span>
          <div className="ctrl-btn-group">
            <button
              type="button"
              className="btn sm"
              title="Snap Left"
              aria-label="Snap window left"
              onClick={() =>
                void invoke("snap_active_window", { layout: "left" }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              <PanelLeft size={13} />
            </button>
            <button
              type="button"
              className="btn sm"
              title="Snap Right"
              aria-label="Snap window right"
              onClick={() =>
                void invoke("snap_active_window", { layout: "right" }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              <PanelRight size={13} />
            </button>
            <button
              type="button"
              className="btn sm"
              title="Center Window"
              aria-label="Center window"
              onClick={() =>
                void invoke("snap_active_window", { layout: "center" }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              <Square size={12} />
            </button>
            <button
              type="button"
              className="btn sm"
              title="Maximize Window"
              aria-label="Maximize window"
              onClick={() =>
                void invoke("snap_active_window", { layout: "maximize" }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              <Maximize2 size={12} />
            </button>
          </div>
        </div>

        <div className="ctrl-settings-links">
          {(
            [
              ["sound", "Sound"],
              ["display", "Display"],
              ["bluetooth", "Bluetooth"],
              ["network-wifi", "Wi-Fi"],
              ["powersleep", "Power"],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              className="btn sm"
              onClick={() =>
                void invoke("open_windows_setting", { key }).catch((e) => setError(String(e)))
              }
            >
              <ExternalLink size={11} /> {label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
    </section>
  );
}
