import { Fragment, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowRight, Download, Sparkles, X } from "lucide-react";
import { useUpdater } from "../../stores/updater";

function renderReleaseNotes(raw: string) {
  const lines = raw
    .split(/\r?\n/)
    .map((l) => l.trimEnd())
    .filter((l, idx, arr) => !(l.trim() === "" && arr[idx - 1]?.trim() === ""));

  if (lines.length === 0) {
    return (
      <p className="upd-note-line is-muted">
        Signed update ready from GitHub Releases. Includes latest stability and feature updates.
      </p>
    );
  }

  return lines.map((line, i) => {
    const t = line.trim();
    if (!t) return <div key={i} className="upd-note-gap" />;
    if (t.startsWith("### ") || t.startsWith("## ") || t.startsWith("# ")) {
      return (
        <div key={i} className="upd-note-heading">
          {t.replace(/^#+\s*/, "")}
        </div>
      );
    }
    if (t.startsWith("- ") || t.startsWith("* ")) {
      return (
        <div key={i} className="upd-note-bullet">
          <span className="upd-bullet-dot" aria-hidden="true" />
          <span>{renderInlineBold(t.slice(2))}</span>
        </div>
      );
    }
    return (
      <p key={i} className="upd-note-line">
        {renderInlineBold(t)}
      </p>
    );
  });
}

function renderInlineBold(text: string) {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return parts.map((part, idx) => {
    if (part.startsWith("**") && part.endsWith("**") && part.length > 4) {
      return <strong key={idx}>{part.slice(2, -2)}</strong>;
    }
    if (part.startsWith("`") && part.endsWith("`") && part.length > 2) {
      return (
        <code key={idx} className="upd-inline-code">
          {part.slice(1, -1)}
        </code>
      );
    }
    return <Fragment key={idx}>{part}</Fragment>;
  });
}

export function FloatingUpdate() {
  const updater = useUpdater();
  const snap = updater.snapshot;

  useEffect(() => {
    const stop = updater.init();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && snap.status !== "downloading" && snap.status !== "installing") {
        void updater.dismiss();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      stop();
    };
  }, [updater.init, updater.dismiss, snap.status]);

  function handleHeaderMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button")) return;
    void invoke("start_floating_drag");
  }

  const busy = snap.status === "downloading" || snap.status === "installing";
  const pct = snap.progressPercent ?? 0;
  const info = snap.available;

  return (
    <div className="float-win float-update">
      <header className="float-upd-header" onMouseDown={handleHeaderMouseDown}>
        <div className="float-upd-brand">
          <img src="/favicon.svg" alt="" width={20} height={20} className="float-upd-logo" />
          <div className="float-upd-titles">
            <strong>{info ? `Peek v${info.version} available` : "Peek Update"}</strong>
            <span className="float-upd-ver">
              v{snap.currentVersion}
              {info && (
                <>
                  <ArrowRight size={10} />v{info.version}
                </>
              )}
            </span>
          </div>
        </div>
        {!busy && (
          <button
            type="button"
            className="float-close-btn"
            aria-label="Close update window"
            title="Remind later"
            onClick={() => void updater.dismiss()}
          >
            <X size={13} />
          </button>
        )}
      </header>

      <div className="float-upd-body">
        <div className="float-upd-notes-head">
          <Sparkles size={12} />
          <span>Patch Notes</span>
        </div>
        <div className="float-upd-notes-scroll">
          {renderReleaseNotes(info?.fullNotes || info?.notes || "")}
        </div>
      </div>

      {busy && (
        <div className="float-upd-progress">
          <div className="float-upd-progress-row">
            <span>
              {snap.status === "installing" ? "Installing update…" : `Downloading… ${pct}%`}
            </span>
          </div>
          <div className="update-progress-track">
            <div className="update-progress-fill" style={{ width: `${pct}%` }} />
          </div>
        </div>
      )}

      {snap.error && (
        <div className="float-upd-error" role="status">
          {snap.error}
        </div>
      )}

      <footer className="float-upd-footer">
        <button
          type="button"
          className="btn primary sm"
          disabled={busy || !info}
          onClick={() => void updater.install()}
        >
          <Download size={12} />
          <span>{busy ? "Updating…" : "Update"}</span>
        </button>
        <button
          type="button"
          className="btn sm"
          disabled={busy || !info}
          onClick={() => void updater.skip()}
        >
          Skip
        </button>
        <button
          type="button"
          className="btn sm"
          disabled={busy}
          onClick={() => void updater.dismiss()}
        >
          Later
        </button>
      </footer>
    </div>
  );
}
