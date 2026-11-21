import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Bot, Link2, Pin, RefreshCw, SlidersHorizontal, Unlink } from "lucide-react";
import { AiProviderLogo, UsageRing, type PinnedAiItem } from "../../components/AiNotchIcons";
import { NumberStepper } from "../../components/NumberStepper";
import { usePreferences } from "../../stores/preferences";
import type { AiProviderId, ProviderBudgets } from "../../types/peek";

type AgentStatus = "working" | "needsApproval" | "completed" | "idle";

type RateLimitWindow = {
  label: string;
  usedPercent: number;
  windowMinutes: number | null;
  resetsAt: number | null;
};

export type ProviderUsage = {
  id: string;
  name: string;
  available: boolean;
  status: AgentStatus;
  activeModel: string;
  sessionsCount: number;
  todayInputTokens: number;
  todayOutputTokens: number;
  todayCachedTokens: number;
  todayTotalTokens: number;
  todayCostUsd: number | null;
  fiveHourTokens: number;
  weeklyTokens: number;
  fiveHourResetsAt: number | null;
  weeklyResetsAt: number | null;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCachedTokens: number;
  totalTokens: number;
  totalCostUsd: number | null;
  planType: string;
  quotaNote: string;
  rateLimits: RateLimitWindow[];
  lastActiveMs: number;
};

export type DevMonitorSnapshot = {
  providers: ProviderUsage[];
  claudeStatuslineConnected?: boolean;
};

export type LimitRow = {
  key: "weekly" | "5h";
  title: "Weekly Limit Remaining" | "Five Hour Limit Remaining";
  remainingPercent: number;
  subtitle: string;
  isLive: boolean;
};

const STATUS_LABEL: Record<AgentStatus, string> = {
  working: "Working",
  needsApproval: "Approval",
  completed: "Done",
  idle: "Idle",
};

function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}k`;
  return String(n);
}

function formatResetLong(epochSecs: number | null): string {
  if (!epochSecs) return "soon";
  const diffMs = epochSecs * 1000 - Date.now();
  if (diffMs <= 0) return "shortly";
  const totalMins = Math.max(1, Math.round(diffMs / 60_000));
  const days = Math.floor(totalMins / (60 * 24));
  const hrs = Math.floor((totalMins % (60 * 24)) / 60);
  const mins = totalMins % 60;
  if (days > 0) {
    return hrs > 0
      ? `${days} ${days === 1 ? "day" : "days"}, ${hrs} ${hrs === 1 ? "hour" : "hours"}`
      : `${days} ${days === 1 ? "day" : "days"}`;
  }
  if (hrs > 0) {
    return mins > 0
      ? `${hrs} ${hrs === 1 ? "hour" : "hours"}, ${mins} ${mins === 1 ? "minute" : "minutes"}`
      : `${hrs} ${hrs === 1 ? "hour" : "hours"}`;
  }
  return `${mins} ${mins === 1 ? "minute" : "minutes"}`;
}

export function resolveLimitRows(p: ProviderUsage, budgets: ProviderBudgets): [LimitRow, LimitRow] {
  const [b5h, b7d] =
    p.id === "claude"
      ? [budgets.claude5hTokens, budgets.claudeWeeklyTokens]
      : p.id === "gemini"
        ? [budgets.gemini5hTokens, budgets.geminiWeeklyTokens]
        : [budgets.codex5hTokens, budgets.codexWeeklyTokens];

  const official5h = p.rateLimits.find((r) => r.windowMinutes !== null && r.windowMinutes <= 360);
  const officialWeekly = p.rateLimits.find(
    (r) => r.windowMinutes !== null && r.windowMinutes > 360 && r.windowMinutes <= 11_520,
  );
  const officialOther = p.rateLimits.find((r) => r !== official5h && r !== officialWeekly);
  const weeklyMatch = officialWeekly ?? (!official5h && officialOther ? officialOther : undefined);

  const weeklyRow: LimitRow = weeklyMatch
    ? (() => {
        const used = Math.min(100, Math.max(0, weeklyMatch.usedPercent));
        const rem = Math.round(100 - used);
        return {
          key: "weekly",
          title: "Weekly Limit Remaining",
          remainingPercent: rem,
          subtitle: weeklyMatch.resetsAt
            ? `It will fully refresh in ${formatResetLong(weeklyMatch.resetsAt)}.`
            : "Live plan limit remaining.",
          isLive: true,
        };
      })()
    : (() => {
        const used = Math.min(100, (p.weeklyTokens / Math.max(1, b7d)) * 100);
        const rem = Math.round(100 - used);
        const resetPart = p.weeklyResetsAt ? ` · resets in ${formatResetLong(p.weeklyResetsAt)}` : "";
        return {
          key: "weekly",
          title: "Weekly Limit Remaining",
          remainingPercent: rem,
          subtitle: `Estimated (${formatTokens(p.weeklyTokens)} / ${formatTokens(b7d)})${resetPart}`,
          isLive: false,
        };
      })();

  const fiveHourRow: LimitRow = official5h
    ? (() => {
        const used = Math.min(100, Math.max(0, official5h.usedPercent));
        const rem = Math.round(100 - used);
        return {
          key: "5h",
          title: "Five Hour Limit Remaining",
          remainingPercent: rem,
          subtitle: official5h.resetsAt
            ? `It will fully refresh in ${formatResetLong(official5h.resetsAt)}.`
            : "Live plan limit remaining.",
          isLive: true,
        };
      })()
    : (() => {
        const used = Math.min(100, (p.fiveHourTokens / Math.max(1, b5h)) * 100);
        const rem = Math.round(100 - used);
        const resetPart = p.fiveHourResetsAt
          ? ` · resets in ${formatResetLong(p.fiveHourResetsAt)}`
          : "";
        return {
          key: "5h",
          title: "Five Hour Limit Remaining",
          remainingPercent: rem,
          subtitle: `Estimated (${formatTokens(p.fiveHourTokens)} / ${formatTokens(b5h)})${resetPart}`,
          isLive: false,
        };
      })();

  return [weeklyRow, fiveHourRow];
}

const PROVIDER_NAMES: Record<AiProviderId, string> = {
  claude: "Claude",
  gemini: "Gemini",
  codex: "Codex",
};

export function useAiUsage(active: boolean) {
  const [snapshot, setSnapshot] = useState<DevMonitorSnapshot | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setError("");
    try {
      const snap = await invoke<DevMonitorSnapshot>("get_dev_monitor");
      setSnapshot(snap);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    void invoke("watch_dev_monitor", { enabled: true }).catch(() => {});
    const sub = listen<DevMonitorSnapshot>("dev-monitor-update", ({ payload }) => {
      setSnapshot(payload);
    });
    return () => {
      void sub.then((u) => u()).catch(() => {});
      void invoke("watch_dev_monitor", { enabled: false }).catch(() => {});
    };
  }, [active, refresh]);

  return { snapshot, error, refresh };
}

export function computePinnedAiItems(
  snapshot: DevMonitorSnapshot | null,
  pinnedIds: AiProviderId[],
  budgets: ProviderBudgets,
): PinnedAiItem[] {
  return pinnedIds.map((id) => {
    const provider = snapshot?.providers.find((p) => p.id === id);
    const name = provider?.name ?? PROVIDER_NAMES[id] ?? id;
    if (!provider) {
      return {
        id,
        name,
        percent: 0,
        tooltip: `${name} · Reading usage…`,
      };
    }
    const [weeklyRow, fiveRow] = resolveLimitRows(provider, budgets);
    const percent = Math.min(weeklyRow.remainingPercent, fiveRow.remainingPercent);
    const isLive = weeklyRow.isLive || fiveRow.isLive;
    return {
      id,
      name,
      percent,
      tooltip: `${name} · 5h ${fiveRow.remainingPercent}% left · Weekly ${weeklyRow.remainingPercent}% left (${isLive ? "Live" : "Estimated"})`,
    };
  });
}

export function AiPanel({
  snapshot,
  error = "",
  onRefresh,
}: {
  snapshot: DevMonitorSnapshot | null;
  error?: string;
  onRefresh: () => void | Promise<void>;
}) {
  const { preferences, ready, saving, save } = usePreferences();
  const [editBudgets, setEditBudgets] = useState(false);
  const [bridgeBusy, setBridgeBusy] = useState(false);
  const [bridgeError, setBridgeError] = useState("");

  const togglePin = (id: AiProviderId) => {
    const current = preferences.pinnedAiProviders;
    const next = current.includes(id)
      ? current.filter((item) => item !== id)
      : [...current, id];
    void save({ pinnedAiProviders: next });
  };

  const toggleClaudeBridge = async (connected: boolean) => {
    setBridgeBusy(true);
    setBridgeError("");
    try {
      await invoke<DevMonitorSnapshot>(
        connected ? "remove_claude_statusline" : "install_claude_statusline",
      );
      await onRefresh();
    } catch (e) {
      setBridgeError(String(e));
    } finally {
      setBridgeBusy(false);
    }
  };

  const budgets = preferences.aiBudgets;
  const updateBudgetMillions = (key: keyof ProviderBudgets, millions: number) => {
    void save({
      aiBudgets: {
        ...budgets,
        [key]: Math.max(1, millions) * 1_000_000,
      },
    });
  };

  return (
    <section aria-label="AI usage" className="ai-panel">
      <div className="ai-actions-row">
        <span className="section-heading">Limits & usage</span>
        <div className="tools-row">
          <button
            type="button"
            className={"btn sm" + (editBudgets ? " primary" : "")}
            onClick={() => setEditBudgets((v) => !v)}
          >
            <SlidersHorizontal size={12} /> Budgets
          </button>
          <button
            type="button"
            className="btn sm"
            aria-label="Refresh AI usage"
            onClick={() => void onRefresh()}
          >
            <RefreshCw size={12} /> Refresh
          </button>
        </div>
      </div>

      {editBudgets && (
        <div className="ui-section budget-editor">
          <div className="status-note">
            Local token targets (M) used when providers do not write plan quota % to disk
          </div>
          {(
            [
              ["Claude", "claude5hTokens", "claudeWeeklyTokens"],
              ["Gemini", "gemini5hTokens", "geminiWeeklyTokens"],
              ["Codex", "codex5hTokens", "codexWeeklyTokens"],
            ] as const
          ).map(([label, k5, kw]) => (
            <div key={label} className="budget-row">
              <span className="budget-name">{label}</span>
              <div className="budget-steppers">
                <span className="status-note">5h</span>
                <NumberStepper
                  ariaLabel={`${label} 5-hour token budget in millions`}
                  min={1}
                  max={2000}
                  step={5}
                  suffix="M"
                  disabled={!ready || saving}
                  value={Math.max(1, Math.round(budgets[k5] / 1_000_000))}
                  onChange={(m) => updateBudgetMillions(k5, m)}
                />
                <span className="status-note">7d</span>
                <NumberStepper
                  ariaLabel={`${label} weekly token budget in millions`}
                  min={5}
                  max={9999}
                  step={25}
                  suffix="M"
                  disabled={!ready || saving}
                  value={Math.max(5, Math.round(budgets[kw] / 1_000_000))}
                  onChange={(m) => updateBudgetMillions(kw, m)}
                />
              </div>
            </div>
          ))}
        </div>
      )}

      {!snapshot ? (
        <div className="empty-panel">
          <Bot size={26} strokeWidth={1.5} />
          <span role="status">Reading local usage logs…</span>
        </div>
      ) : (
        <div className="ai-list">
          {snapshot.providers.map((p) => {
            const rows = resolveLimitRows(p, budgets);
            const hasLive = rows.some((r) => r.isLive);
            const providerId = p.id as AiProviderId;
            const isPinned = preferences.pinnedAiProviders.includes(providerId);
            const claudeConnected = Boolean(snapshot.claudeStatuslineConnected);
            return (
              <div key={p.id} className="ui-section ai-provider-section">
                <div className="agent-top">
                  <div className="agent-title-group">
                    <div className="agent-name-row">
                      <AiProviderLogo id={p.id} size={15} />
                      <strong>{p.name}</strong>
                      <span className={"source-badge " + (hasLive ? "live" : "est")}>
                        {hasLive ? "Live plan" : "Estimated"}
                      </span>
                    </div>
                    <span className="status-note">
                      {p.available
                        ? `${p.activeModel} · ${formatTokens(p.todayTotalTokens)} today${p.todayCostUsd !== null ? ` · ~$${p.todayCostUsd.toFixed(2)}` : ""}`
                        : "Not detected locally"}
                    </span>
                  </div>
                  <div className="agent-top-right">
                    {p.id === "claude" && p.available && (
                      <button
                        type="button"
                        className={"bridge-btn" + (claudeConnected ? " connected" : "")}
                        disabled={bridgeBusy}
                        title={
                          claudeConnected
                            ? "Claude Code statusLine linked (click to disconnect)"
                            : "Connect Claude Code statusLine for live 5h & weekly limits"
                        }
                        onClick={() => void toggleClaudeBridge(claudeConnected)}
                      >
                        {claudeConnected ? <Unlink size={11} /> : <Link2 size={11} />}
                        <span>{claudeConnected ? "Live linked" : "Connect live"}</span>
                      </button>
                    )}
                    <span className={"status-pill " + p.status}>{STATUS_LABEL[p.status]}</span>
                    <button
                      type="button"
                      className={"pin-notch-btn" + (isPinned ? " pinned" : "")}
                      aria-pressed={isPinned}
                      aria-label={isPinned ? `Unpin ${p.name} from notch` : `Pin ${p.name} to notch`}
                      title={isPinned ? "Unpin from notch" : "Pin to notch"}
                      onClick={() => togglePin(providerId)}
                    >
                      <Pin size={12} />
                    </button>
                  </div>
                </div>

                <div className="limit-cards">
                  {rows.map((row) => (
                    <div key={row.key} className="limit-ring-row">
                      <div className="limit-row-text">
                        <span className="limit-row-title">{row.title}</span>
                        <span className="limit-row-sub">{row.subtitle}</span>
                      </div>
                      <div
                        className="limit-row-meter"
                        aria-label={`${p.name} ${row.title}: ${row.remainingPercent}% remaining`}
                      >
                        <span className="limit-pct">{row.remainingPercent}%</span>
                        <UsageRing percent={row.remainingPercent} size={26} strokeWidth={2.5} />
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {(bridgeError || error) && (
        <div className="error" role="alert">
          {bridgeError || error}
        </div>
      )}
    </section>
  );
}
