import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Bell,
  BellOff,
  BellRing,
  Eye,
  EyeOff,
  History,
  Trash2,
  X,
} from "lucide-react";
import { useActivity } from "../../stores/activity";
import { usePreferences } from "../../stores/preferences";

type WindowsNotificationStatus =
  | { status: "disabled" | "allowed" }
  | { status: "denied" | "unspecified" | "unavailable"; reason: string };

function relativeTime(ms: number): string {
  const diff = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (diff < 60) return "now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86400)}d`;
}

export function NotificationsPanel() {
  const { preferences, ready, saving, save } = usePreferences();
  const { notifications, dismissNotification, clearNotifications } = useActivity();
  const [winStatus, setWinStatus] = useState<WindowsNotificationStatus>({ status: "disabled" });
  const [sourceFilter, setSourceFilter] = useState<string>("all");

  useEffect(() => {
    void invoke<WindowsNotificationStatus>("get_windows_notification_status")
      .then(setWinStatus)
      .catch(() => {});
    const sub = listen<WindowsNotificationStatus>("windows-notifications-status", ({ payload }) => {
      setWinStatus(payload);
    });
    return () => {
      void sub.then((u) => u()).catch(() => {});
    };
  }, []);

  const sources = useMemo(() => {
    const set = new Set<string>();
    for (const n of notifications) {
      if (n.source) set.add(n.source);
    }
    return Array.from(set);
  }, [notifications]);

  const filtered = useMemo(
    () =>
      sourceFilter === "all"
        ? notifications
        : notifications.filter((n) => n.source === sourceFilter),
    [notifications, sourceFilter],
  );

  function toggleMuteSource(source: string) {
    const exists = preferences.mutedSources.some(
      (s) => s.toLowerCase() === source.toLowerCase(),
    );
    const next = exists
      ? preferences.mutedSources.filter((s) => s.toLowerCase() !== source.toLowerCase())
      : [...preferences.mutedSources, source];
    void save({ mutedSources: next });
  }

  const winToastSublabel = !preferences.notificationsEnabled
    ? "Off"
    : winStatus.status === "allowed"
      ? "Active"
      : winStatus.status === "denied"
        ? "Denied"
        : winStatus.status === "unavailable"
          ? "Unavailable"
          : "Pending";

  const winToastReason =
    preferences.notificationsEnabled &&
    winStatus.status !== "allowed" &&
    "reason" in winStatus
      ? winStatus.reason
      : "";

  return (
    <section aria-label="Notifications" className="notif-panel">
      {/* 2x2 Bento Quick-Toggles */}
      <div className="notif-bento-toggles">
        <button
          type="button"
          className={"notif-toggle-tile" + (preferences.doNotDisturb ? " is-on" : "")}
          aria-pressed={preferences.doNotDisturb}
          disabled={!ready || saving}
          onClick={() => void save({ doNotDisturb: !preferences.doNotDisturb })}
        >
          {preferences.doNotDisturb ? <BellOff size={15} /> : <Bell size={15} />}
          <span className="notif-toggle-copy">
            <strong>Do Not Disturb</strong>
            <span>{preferences.doNotDisturb ? "Silenced" : "Off"}</span>
          </span>
        </button>

        <button
          type="button"
          className={"notif-toggle-tile" + (preferences.hideSensitiveNotifications ? " is-on" : "")}
          aria-pressed={preferences.hideSensitiveNotifications}
          disabled={!ready || saving}
          onClick={() =>
            void save({ hideSensitiveNotifications: !preferences.hideSensitiveNotifications })
          }
        >
          {preferences.hideSensitiveNotifications ? <EyeOff size={15} /> : <Eye size={15} />}
          <span className="notif-toggle-copy">
            <strong>Hide Sensitive</strong>
            <span>{preferences.hideSensitiveNotifications ? "Redacted" : "Visible"}</span>
          </span>
        </button>

        <button
          type="button"
          className={"notif-toggle-tile" + (preferences.notificationHistoryEnabled ? " is-on" : "")}
          aria-pressed={preferences.notificationHistoryEnabled}
          disabled={!ready || saving}
          onClick={() =>
            void save({ notificationHistoryEnabled: !preferences.notificationHistoryEnabled })
          }
        >
          <History size={15} />
          <span className="notif-toggle-copy">
            <strong>Save History</strong>
            <span>{preferences.notificationHistoryEnabled ? "Keeping" : "Off"}</span>
          </span>
        </button>

        <button
          type="button"
          className={"notif-toggle-tile" + (preferences.notificationsEnabled ? " is-on" : "")}
          aria-pressed={preferences.notificationsEnabled}
          disabled={!ready || saving}
          title={winToastReason || "Toggle Windows toast listener"}
          onClick={() => void save({ notificationsEnabled: !preferences.notificationsEnabled })}
        >
          <BellRing size={15} />
          <span className="notif-toggle-copy">
            <strong>Win Toasts</strong>
            <span>{winToastSublabel}</span>
          </span>
        </button>
      </div>

      {winToastReason && <div className="status-note">{winToastReason}</div>}

      <div className="notif-header-row">
        <div className="notif-filters">
          <button
            type="button"
            className={"btn sm" + (sourceFilter === "all" ? " primary" : "")}
            onClick={() => setSourceFilter("all")}
          >
            All ({notifications.length})
          </button>
          {sources.map((src) => {
            const muted = preferences.mutedSources.some(
              (m) => m.toLowerCase() === src.toLowerCase(),
            );
            return (
              <div key={src} className="notif-chip-group">
                <button
                  type="button"
                  className={"btn sm" + (sourceFilter === src ? " primary" : "")}
                  onClick={() => setSourceFilter(src)}
                >
                  {src}
                </button>
                <button
                  type="button"
                  className={"btn sm" + (muted ? " danger" : "")}
                  title={muted ? `Unmute ${src}` : `Mute ${src}`}
                  aria-label={muted ? `Unmute ${src}` : `Mute ${src}`}
                  onClick={() => toggleMuteSource(src)}
                >
                  <BellOff size={11} />
                </button>
              </div>
            );
          })}
        </div>
        {notifications.length > 0 && (
          <button
            type="button"
            className="btn sm"
            onClick={() => void clearNotifications()}
          >
            <Trash2 size={12} /> Clear
          </button>
        )}
      </div>

      {filtered.length === 0 ? (
        <div className="empty-panel notif-empty-bento">
          <Bell size={24} strokeWidth={1.5} />
          <span>No notifications</span>
        </div>
      ) : (
        <div className="notif-list">
          {filtered.map((n) => (
            <div key={n.id} className="notif-card">
              <div className="notif-item-top">
                <div className="notif-item-meta">
                  <span className="notif-source">{n.source}</span>
                  <span className="notif-time">{relativeTime(n.createdAtMs)}</span>
                </div>
                <button
                  type="button"
                  className="notif-dismiss-btn"
                  aria-label="Dismiss notification"
                  onClick={() => void dismissNotification(n.id)}
                >
                  <X size={11} />
                </button>
              </div>
              <strong>{n.title}</strong>
              {n.body && <span className="status-note">{n.body}</span>}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
