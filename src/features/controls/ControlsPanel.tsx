import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Battery,
  BatteryCharging,
  Bluetooth,
  ChevronRight,
  Cpu,
  Maximize2,
  Mic,
  MicOff,
  PanelLeft,
  PanelRight,
  Plane,
  SlidersHorizontal,
  Square,
  Sun,
  Volume2,
  VolumeX,
  Wifi,
  WifiOff,
  Zap,
} from "lucide-react";
import { AmpKnob } from "../../components/AmpKnob";

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

type ConnectivitySnapshot = {
  wifiAvailable: boolean;
  wifiOn: boolean;
  wifiName: string;
  bluetoothAvailable: boolean;
  bluetoothOn: boolean;
  energySaver: boolean;
};

const WRITE_COOLDOWN_MS = 1600;
const SPARK_POINTS = 18;

function Sparkline({
  values,
  max = 100,
  label,
}: {
  values: number[];
  max?: number;
  label: string;
}) {
  const w = 108;
  const h = 22;
  const safeMax = Math.max(1, max);
  const data = values.length > 0 ? values : [0, 0];
  const step = data.length > 1 ? w / (data.length - 1) : w;
  const points = data.map((v, idx) => {
    const x = idx * step;
    const clamped = Math.max(0, Math.min(safeMax, v));
    const y = h - 2 - (clamped / safeMax) * (h - 4);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  const areaPoints = `0,${h} ${points.join(" ")} ${w},${h}`;

  return (
    <svg className="bento-spark" viewBox={`0 0 ${w} ${h}`} aria-label={label}>
      <polygon points={areaPoints} fill="rgba(255, 255, 255, 0.08)" />
      <polyline
        points={points.join(" ")}
        fill="none"
        stroke="#f4f4f4"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function ControlsPanel() {
  const [audio, setAudio] = useState<AudioSnapshot | null>(null);
  const [displays, setDisplays] = useState<BrightnessDisplay[]>([]);
  const [metrics, setMetrics] = useState<SystemMetrics | null>(null);
  const [connectivity, setConnectivity] = useState<ConnectivitySnapshot>({
    wifiAvailable: false,
    wifiOn: false,
    wifiName: "",
    bluetoothAvailable: false,
    bluetoothOn: false,
    energySaver: false,
  });
  const [radioBusy, setRadioBusy] = useState<"wifi" | "bluetooth" | null>(null);
  const [cpuHistory, setCpuHistory] = useState<number[]>([0, 0, 0, 0]);
  const [netHistory, setNetHistory] = useState<number[]>([0, 0, 0, 0]);
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
      .then((m) => {
        setMetrics(m);
        setCpuHistory((prev) => [...prev.slice(-(SPARK_POINTS - 1)), m.cpuPercent]);
        const netTotal = m.netDownKbps + m.netUpKbps;
        setNetHistory((prev) => [...prev.slice(-(SPARK_POINTS - 1)), netTotal]);
      })
      .catch(() => {});
  }, []);

  const refreshConnectivity = useCallback(async () => {
    await invoke<ConnectivitySnapshot>("get_connectivity_state")
      .then(setConnectivity)
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
    void refreshConnectivity();
    const audioTimer = setInterval(() => void refreshAudio(), 1500);
    const metricsTimer = setInterval(() => void refreshMetrics(), 1500);
    const brightTimer = setInterval(() => void refreshBrightness(), 4000);
    const connTimer = setInterval(() => void refreshConnectivity(), 3500);
    return () => {
      clearInterval(audioTimer);
      clearInterval(metricsTimer);
      clearInterval(brightTimer);
      clearInterval(connTimer);
    };
  }, [refreshAudio, refreshBrightness, refreshMetrics, refreshConnectivity]);

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

  async function handleToggleRadio(kind: "wifi" | "bluetooth") {
    setError("");
    setRadioBusy(kind);
    try {
      const next = await invoke<ConnectivitySnapshot>("toggle_radio", { kind });
      setConnectivity(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setRadioBusy(null);
    }
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
  const netPeak = Math.max(20, ...netHistory);

  return (
    <section aria-label="System controls" className="controls-bento">
      <div className="bento-top-grid">
        {/* Left Hero: Monochrome Amp Volume Dial */}
        <div className="bento-tile bento-audio">
          <div className="bento-tile-head">
            <span className="bento-label">Audio</span>
            <div className="bento-head-actions">
              <button
                type="button"
                className={"bento-mini-btn" + (audio?.masterMuted ? " active-warn" : "")}
                aria-label={audio?.masterMuted ? "Unmute output" : "Mute output"}
                title={audio?.masterMuted ? "Unmute" : "Mute"}
                disabled={!audio?.masterAvailable}
                onClick={() => void toggleMasterMute()}
              >
                {audio?.masterMuted ? <VolumeX size={12} /> : <Volume2 size={12} />}
              </button>
              <button
                type="button"
                className={"bento-mini-btn" + (audio?.micMuted ? " active-warn" : "")}
                aria-label={audio?.micMuted ? "Unmute microphone" : "Mute microphone"}
                title={audio?.micMuted ? "Mic muted" : "Mute mic"}
                disabled={!audio?.micAvailable}
                onClick={() => void toggleMicMute()}
              >
                {audio?.micMuted ? <MicOff size={12} /> : <Mic size={12} />}
              </button>
            </div>
          </div>

          <AmpKnob
            value={audio?.masterVolume ?? 0}
            muted={Boolean(audio?.masterMuted)}
            disabled={!audio?.masterAvailable}
            onDragStart={() => {
              draggingMasterRef.current = true;
            }}
            onDragEnd={() => {
              draggingMasterRef.current = false;
            }}
            onChange={updateMasterVolume}
            onToggleMute={() => void toggleMasterMute()}
          />

          {audio && audio.outputDevices.length > 0 && (
            <select
              className="bento-output-select"
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
          )}
        </div>

        {/* Right Stack: Windows Quick Tiles + Display Brightness */}
        <div className="bento-right-stack">
          <div className="quick-tiles-grid">
            {/* Wi-Fi split tile */}
            <div className={"quick-tile is-split" + (connectivity.wifiOn ? " is-on" : "")}>
              <button
                type="button"
                className="quick-tile-main"
                disabled={!connectivity.wifiAvailable || radioBusy === "wifi"}
                onClick={() => void handleToggleRadio("wifi")}
                title="Toggle Wi-Fi"
              >
                {connectivity.wifiOn ? <Wifi size={15} /> : <WifiOff size={15} />}
                <span className="quick-tile-copy">
                  <strong>Wi-Fi</strong>
                  <span>
                    {!connectivity.wifiAvailable
                      ? "Unavailable"
                      : connectivity.wifiOn
                        ? connectivity.wifiName || "Connected"
                        : "Off"}
                  </span>
                </span>
              </button>
              <button
                type="button"
                className="quick-tile-chevron"
                aria-label="Wi-Fi settings"
                title="Open Wi-Fi settings"
                onClick={() =>
                  void invoke("open_windows_setting", { key: "network-wifi" }).catch((e) =>
                    setError(String(e)),
                  )
                }
              >
                <ChevronRight size={13} />
              </button>
            </div>

            {/* Bluetooth split tile */}
            <div className={"quick-tile is-split" + (connectivity.bluetoothOn ? " is-on" : "")}>
              <button
                type="button"
                className="quick-tile-main"
                disabled={!connectivity.bluetoothAvailable || radioBusy === "bluetooth"}
                onClick={() => void handleToggleRadio("bluetooth")}
                title="Toggle Bluetooth"
              >
                <Bluetooth size={15} />
                <span className="quick-tile-copy">
                  <strong>Bluetooth</strong>
                  <span>
                    {!connectivity.bluetoothAvailable
                      ? "Unavailable"
                      : connectivity.bluetoothOn
                        ? "On"
                        : "Off"}
                  </span>
                </span>
              </button>
              <button
                type="button"
                className="quick-tile-chevron"
                aria-label="Bluetooth settings"
                title="Open Bluetooth settings"
                onClick={() =>
                  void invoke("open_windows_setting", { key: "bluetooth" }).catch((e) =>
                    setError(String(e)),
                  )
                }
              >
                <ChevronRight size={13} />
              </button>
            </div>

            {/* Energy Saver tile */}
            <button
              type="button"
              className={"quick-tile is-single" + (connectivity.energySaver ? " is-on" : "")}
              title="Open Power & Battery settings"
              onClick={() =>
                void invoke("open_windows_setting", { key: "powersleep" }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              <Zap size={14} />
              <span className="quick-tile-copy">
                <strong>Energy</strong>
                <span>{connectivity.energySaver ? "Saver on" : "Standard"}</span>
              </span>
            </button>

            {/* Airplane Mode tile */}
            <button
              type="button"
              className="quick-tile is-single"
              title="Open Airplane mode settings"
              onClick={() =>
                void invoke("open_windows_setting", { key: "network-airplanemode" }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              <Plane size={14} />
              <span className="quick-tile-copy">
                <strong>Airplane</strong>
                <span>Settings</span>
              </span>
            </button>
          </div>

          {/* Display Brightness Bento */}
          <div className="bento-tile bento-bright">
            {displays.length === 0 ? (
              <div className="bento-bright-empty">
                <Sun size={14} />
                <span className="status-note">No adjustable display detected</span>
              </div>
            ) : (
              displays.map((d) => (
                <div key={d.id} className="ctrl-slider-row">
                  <button
                    type="button"
                    className="bento-mini-btn"
                    title="Display settings"
                    aria-label="Open Display settings"
                    onClick={() =>
                      void invoke("open_windows_setting", { key: "display" }).catch((e) =>
                        setError(String(e)),
                      )
                    }
                  >
                    <Sun size={12} />
                  </button>
                  <span className="bento-bright-name truncate" title={d.name}>
                    {displays.length === 1 && d.supported ? "Brightness" : d.name}
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
                    <span className="status-note">Fixed</span>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
      </div>

      {/* Telemetry Bento Row */}
      {metrics && (
        <div className="bento-telemetry-grid">
          <div className="bento-tile bento-metric-card">
            <div className="bento-metric-top">
              <div className="bento-metric-val">
                <span className="bento-metric-k">
                  <Cpu size={11} /> CPU
                </span>
                <strong>{metrics.cpuPercent.toFixed(0)}%</strong>
              </div>
              <Sparkline values={cpuHistory} max={100} label="CPU usage trend" />
            </div>
            <div className="bento-ram-row">
              <div className="bento-ram-head">
                <span className="bento-metric-k">RAM</span>
                <span>
                  {metrics.ramUsedGb}/{metrics.ramTotalGb}G ({metrics.ramPercent}%)
                </span>
              </div>
              <div className="bento-bar-track">
                <div
                  className="bento-bar-fill"
                  style={{ width: `${Math.min(100, Math.max(0, metrics.ramPercent))}%` }}
                />
              </div>
            </div>
          </div>

          <div className="bento-tile bento-metric-card">
            <div className="bento-metric-top">
              <div className="bento-metric-val">
                <span className="bento-metric-k">
                  {metrics.batteryPresent ? (
                    metrics.batteryCharging ? (
                      <BatteryCharging size={12} />
                    ) : (
                      <Battery size={12} />
                    )
                  ) : null}
                  {metrics.batteryPresent ? "Battery" : "Power"}
                </span>
                <strong>
                  {metrics.batteryPresent && metrics.batteryPercent !== null
                    ? `${metrics.batteryPercent}%`
                    : "AC"}
                </strong>
              </div>
              <div className="bento-bar-track compact">
                <div
                  className="bento-bar-fill"
                  style={{
                    width: `${
                      metrics.batteryPresent && metrics.batteryPercent !== null
                        ? Math.min(100, Math.max(0, metrics.batteryPercent))
                        : 100
                    }%`,
                  }}
                />
              </div>
            </div>
            <div className="bento-metric-top">
              <div className="bento-net-readout">
                <span className="bento-metric-k">NET</span>
                <span>
                  ↓{metrics.netDownKbps.toFixed(0)} ↑{metrics.netUpKbps.toFixed(0)} KB/s
                </span>
              </div>
              <Sparkline values={netHistory} max={netPeak} label="Network throughput trend" />
            </div>
          </div>
        </div>
      )}

      {/* Window Snap & Quick Settings Bento Strip */}
      <div className="bento-tile bento-snap-strip">
        <div className="bento-snap-group">
          <span className="bento-label">Window</span>
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
              <PanelLeft size={12} />
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
              <PanelRight size={12} />
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
              <Square size={11} />
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
              <Maximize2 size={11} />
            </button>
          </div>
        </div>

        <button
          type="button"
          className="btn sm"
          title="Open Windows Sound settings"
          onClick={() =>
            void invoke("open_windows_setting", { key: "sound" }).catch((e) =>
              setError(String(e)),
            )
          }
        >
          <SlidersHorizontal size={11} /> Sound
        </button>
      </div>

      {/* App Audio Mixer Bento */}
      {audio && audio.appSessions.length > 0 && (
        <div className="bento-tile bento-mixer">
          <span className="bento-label">App mixer</span>
          <div className="app-mixer-list">
            {audio.appSessions.map((s) => (
              <div key={s.pid} className="ctrl-slider-row">
                <button
                  type="button"
                  className="bento-mini-btn"
                  aria-label={"Mute " + s.name}
                  onClick={() => void toggleAppMute(s.pid, s.muted)}
                >
                  {s.muted ? <VolumeX size={11} /> : <Volume2 size={11} />}
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

      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
    </section>
  );
}
