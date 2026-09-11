// Reporting a problem without needing to be a developer.
//
// The person using this may have no GitHub account, no interest in one, and a
// truck they would rather be looking at. So there is nothing here to sign up
// for and nothing to fill in: the report is already written, and the three
// buttons are copy it, save it, and show me the folder.
//
// Nothing is sent anywhere. A log carries VINs, fault codes and file paths with
// somebody's own name in them, so it travels only when a person decides to move
// it themselves. Sending one to a service the project runs is a real feature and
// a separate decision, and it is not made here.
//
// The banner is the other half. A run that freezes or is killed cannot report
// itself while it is happening — the only moment that failure can be noticed is
// the launch afterwards, when the core finds a marker the last run never
// removed. That is what this offers to explain.

import { useCallback, useEffect, useState } from "react";
import { api } from "../api/client";
import type { SupportReport } from "../api/types";

/** Filename for a saved report. Dated, because the second one matters most. */
function reportFilename(): string {
  const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
  return `wtfault-report-${stamp}.txt`;
}

/** What the last three buttons are currently saying. */
type Said = { kind: "idle" } | { kind: "done"; message: string } | { kind: "failed"; message: string };

/**
 * The report, and the three things anyone can do with it.
 *
 * `report` is passed in rather than fetched here so the banner and the settings
 * screen show the same one — two components polling the same endpoint could
 * disagree about whether the last run crashed, which is the one fact this is
 * for.
 */
export function ProblemReportPanel({ report }: { report: SupportReport }) {
  const [said, setSaid] = useState<Said>({ kind: "idle" });

  const copy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(report.text);
      setSaid({ kind: "done", message: "Copied. Paste it wherever you need it." });
    } catch {
      // A webview is not a browser and the clipboard can simply be unavailable.
      // Selecting the text is then the honest fallback — it is on screen either
      // way, and the button says which happened.
      setSaid({
        kind: "failed",
        message: "Could not reach the clipboard. Select the text below and copy it.",
      });
    }
  }, [report.text]);

  const save = useCallback(async () => {
    try {
      const saved = await api.exportFile({ filename: reportFilename(), content: report.text });
      setSaid({ kind: "done", message: `Saved to ${saved.path}` });
    } catch (e) {
      setSaid({
        kind: "failed",
        message: e instanceof Error ? e.message : "could not save the report",
      });
    }
  }, [report.text]);

  const reveal = useCallback(async () => {
    try {
      const opened = await api.supportReveal();
      setSaid({ kind: "done", message: `Opened ${opened.opened}` });
    } catch (e) {
      setSaid({
        kind: "failed",
        message: e instanceof Error ? e.message : "could not open the log folder",
      });
    }
  }, []);

  return (
    <div className="report-panel">
      <div className="report-facts">
        <span>
          Version <strong>{report.app_version}</strong>
        </span>
        <span>{report.os}</span>
        {report.previous_run_ended_badly ? (
          <span className="cls-bus_error">
            last run ({report.previous_run_ended_badly.version}) ended unexpectedly
          </span>
        ) : (
          <span className="faint">last run ended cleanly</span>
        )}
      </div>

      <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
        <button className="primary" onClick={copy}>
          Copy report
        </button>
        <button onClick={save}>Save to a file</button>
        {report.log_dir && <button onClick={reveal}>Open log folder</button>}
      </div>

      {said.kind !== "idle" && (
        <div className={said.kind === "failed" ? "cls-bus_error" : "faint"} style={{ fontSize: 11 }}>
          {said.message}
        </div>
      )}

      {/* Monospace here is right rather than lazy: this is log output, and the
          column alignment is part of reading it. */}
      <pre className="report-text">{report.text}</pre>

      <p className="faint" style={{ fontSize: 11, margin: 0 }}>
        Nothing above has been sent anywhere. It is assembled on this machine and
        goes no further unless you send it.
      </p>
    </div>
  );
}

/**
 * The strip that appears when the previous run ended badly.
 *
 * Quiet, and only after a real failure — a banner that appears every launch is
 * a banner nobody reads. It carries the report with it rather than sending
 * somebody hunting through settings while they still remember what happened.
 */
export function ProblemBanner({ coreUp }: { coreUp: boolean }) {
  const [report, setReport] = useState<SupportReport | null>(null);
  const [open, setOpen] = useState(false);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    if (!coreUp) return;
    let cancelled = false;
    api
      .supportReport()
      .then((r) => {
        if (!cancelled) setReport(r);
      })
      // A report that cannot be assembled is not worth interrupting anybody
      // over. The settings screen still offers it explicitly.
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [coreUp]);

  const bad = report?.previous_run_ended_badly;
  if (dismissed || !report || !bad) return null;

  return (
    <div className="problem-bar">
      <div className="row" style={{ gap: 10, flex: 1, minWidth: 0 }}>
        <strong>The last session ended unexpectedly</strong>
        <span className="faint">
          version {bad.version}, started {bad.started.replace("T", " ").replace("Z", " UTC")}
        </span>
      </div>
      <div className="row" style={{ gap: 8 }}>
        <button onClick={() => setOpen((o) => !o)}>
          {open ? "hide the details" : "what happened?"}
        </button>
        <button onClick={() => setDismissed(true)}>Not now</button>
      </div>
      {open && (
        <div style={{ flexBasis: "100%" }}>
          <ProblemReportPanel report={report} />
        </div>
      )}
    </div>
  );
}

/**
 * The settings entry. Fetches its own copy, because somebody opening this has
 * asked for it — unlike the banner, which only speaks when something is wrong.
 */
export function ProblemReportSection() {
  const [report, setReport] = useState<SupportReport | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .supportReport()
      .then((r) => {
        if (!cancelled) setReport(r);
      })
      .catch((e: unknown) => {
        if (!cancelled) setFailed(e instanceof Error ? e.message : "could not assemble a report");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="card">
      <h2 style={{ marginTop: 0, fontSize: 14 }}>Something went wrong?</h2>
      <p className="faint" style={{ fontSize: 12, marginTop: 0 }}>
        This gathers what the app knows about itself — its version, this machine,
        how the last run ended, and the end of its log — into one piece of text
        you can hand to whoever is helping.
      </p>
      {failed && <div className="cls-bus_error">{failed}</div>}
      {report && <ProblemReportPanel report={report} />}
    </div>
  );
}
