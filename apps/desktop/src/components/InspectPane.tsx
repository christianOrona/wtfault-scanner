// The pre-purchase inspection: one button, then a report a non-mechanic can act on.
//
// Presentation rules, all of them load-bearing:
//
//  * The verdict leads. Someone standing next to a car they might buy wants the
//    answer first and the reasoning second.
//  * Every claim shows where it came from. A measured fault and a guess about
//    what it will cost look different on screen, because they are different.
//  * Costs are shown as bands with their basis attached, never as a figure.
//  * `not_checked` is given the same weight as the findings. A buyer who thinks
//    a clean scan means a sound car has been misled by this screen.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, describeError } from "../api/client";
import type {
  AgentStatus, ClaimSource, Finding, InspectResponse, Report, ScanPurpose, TraceEntry,
} from "../api/types";
import { ErrorBanner, Spinner } from "./primitives";
import { useAgentProgress, type ProgressLine } from "../hooks/useAgentProgress";
import { reportToText } from "./reportText";
import { saveFile, scanFilename } from "./exportFile";
import { MonitorTestsCard } from "./MonitorTestsCard";
import { ReadinessCard } from "./ReadinessCard";
import { PaneIntro } from "../explain";

/**
 * The verdict, said the way the reader needs to hear it.
 *
 * The same evidence means something different depending on who is asking. "Walk
 * away" is sound advice to someone considering a purchase and meaningless to
 * someone who has owned the truck for six years - they cannot walk away from
 * it, they have to fix it. Only the framing changes; the verdict itself is the
 * agent's and is not touched.
 */
function verdictCopy(
  v: Report["verdict"],
  purpose: ScanPurpose,
): { label: string; tone: string; blurb: string } {
  const owner = purpose !== "buyer";
  switch (v) {
    case "walk_away":
      return {
        label: owner ? "Needs serious attention" : "Walk away",
        tone: "serious",
        blurb: owner
          ? "Serious faults were found. These are worth dealing with before they get more expensive."
          : "Serious faults were found. Do not buy this unless the price already reflects them.",
      };
    case "negotiate":
      return {
        label: owner ? "Worth booking in" : "Negotiate",
        tone: "caution",
        blurb: owner
          ? "Real problems with a knowable cost. None of them is an emergency, but none will fix itself."
          : "Real problems with a knowable cost. Use them to bring the price down.",
      };
    case "looks_sound":
      return {
        label: "Looks sound",
        tone: "ok",
        blurb: owner
          ? "Nothing alarming turned up in what could be checked electronically. That is a good result."
          : "Nothing alarming turned up in what could be checked electronically.",
      };
    default:
      return {
        label: "Inconclusive",
        tone: "info",
        blurb: "Too little could be read to give an answer. That is not the same as good news.",
      };
  }
}

const SOURCE: Record<ClaimSource, { label: string; title: string }> = {
  measured: {
    label: "measured",
    title: "Read from this vehicle in this session. Click the evidence link to see the raw exchange.",
  },
  catalog: {
    label: "code catalog",
    title: "The standard SAE description for this trouble code.",
  },
  model_knowledge: {
    label: "AI general knowledge",
    title:
      "The model's own automotive knowledge. NOT checked against this vehicle or any data this app owns. Treat as a starting point, not a fact.",
  },
};

export function InspectPane({
  agent,
  connected,
  onEvidence,
  onOpenSettings,
  onFinished,
  sessionId,
  vin,
  descriptor,
  moduleKey,
}: {
  agent: AgentStatus | null;
  connected: boolean;
  /** Session to follow for live progress while a run is in flight. */
  sessionId: string | null;
  /** Shown in the exported report so it identifies the vehicle it came from. */
  vin: string | null;
  /** Which adapter produced it. */
  descriptor: string | null;
  /** Module to run the quick readiness check against. */
  moduleKey: string | null;
  onEvidence: (ref: number) => void;
  onOpenSettings: () => void;
  /** The agent reads the vehicle itself, so what the rest of the app knows
      about modules and the VIN is stale once a run completes. */
  onFinished?: () => void;
}) {
  const [result, setResult] = useState<InspectResponse | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const timer = useRef<number | null>(null);
  const progress = useAgentProgress(sessionId, running);

  // A scan is minutes, not seconds. A running clock is the difference between
  // "this is working" and "this has hung".
  useEffect(() => {
    if (running) {
      const started = Date.now();
      timer.current = window.setInterval(() => setElapsed(Math.round((Date.now() - started) / 1000)), 1000);
    } else if (timer.current) {
      window.clearInterval(timer.current);
      timer.current = null;
    }
    return () => {
      if (timer.current) window.clearInterval(timer.current);
    };
  }, [running]);

  const run = useCallback(async () => {
    setRunning(true);
    setError(null);
    setResult(null);
    setElapsed(0);
    try {
      setResult(await api.inspect());
      onFinished?.();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setRunning(false);
    }
  }, [onFinished]);

  if (!agent?.ready) {
    return (
      <div className="pane">
        <div className="card">
          <strong>No model is set up yet.</strong>
          <p className="muted">
            {agent?.reason ?? "The agent needs a model provider before it can explain anything."}
          </p>
          <button className="primary" onClick={onOpenSettings}>Set one up</button>
        </div>
      </div>
    );
  }

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="inspection" />
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 14 }}>
        <div>
          <strong>Full inspection</strong>
          <div className="faint">
            The assistant decides which tests to run, reads the vehicle, and explains what it
            found. Set whether this is your vehicle or one you are considering in Settings -
            it changes what the report is written for.
          </div>
        </div>
        <div className="row">
          {/* There was no way back from a finished report. The quick checks and
              the Inspect button are replaced by the result and stay replaced,
              so the only route to the pre-inspection screen was to disconnect.
              The report is worth keeping until it is explicitly dismissed, so
              this clears it rather than being a browser-style back button. */}
          {result && !running && (
            <button onClick={() => { setResult(null); setError(null); }}>
              Back to checks
            </button>
          )}
          <button className="primary" onClick={() => void run()} disabled={running || !connected}>
            {running
              ? <Spinner label={`Inspecting… ${elapsed}s`} />
              : result ? "Inspect again" : "Inspect this vehicle"}
          </button>
        </div>
      </div>

      {!connected && (
        <div className="banner caution">
          <span className="b-code">not connected</span>
          <span>Connect to a vehicle first — the agent can only report what it can read.</span>
        </div>
      )}

      <ErrorBanner error={error} />

      {/* Two cheap, instant checks worth having before committing to a full
          inspection. Readiness answers "were the codes just cleared to hide
          something?"; the self-test measurements answer "what is wearing out
          that no warning light will mention?". Both are a handful of reads and
          neither needs the model. */}
      {connected && !running && !result && (
        <>
          <ReadinessCard moduleKey={moduleKey} onEvidence={onEvidence} />
          <MonitorTestsCard moduleKey={moduleKey} onEvidence={onEvidence} />
        </>
      )}

      {running && <LiveProgress elapsed={elapsed} lines={progress} />}

      {result && !result.report && (
        <div className="banner serious">
          <span className="b-code">no report</span>
          <div>
            The agent ran {result.steps} steps but did not produce a report
            {result.truncated ? " before running out of them" : ""}. This usually means the model
            is too small for the job — try a stronger one in Settings.
            {result.text && <div className="faint" style={{ marginTop: 6 }}>{result.text}</div>}
          </div>
        </div>
      )}

      {/* A truncated run still returns a report, assembled from evidence alone.
          It must not be read as a finished assessment, so it is labelled before
          the verdict rather than after it. */}
      {result?.truncated && result.report && (
        <div className="banner serious">
          <span className="b-code">unfinished</span>
          <div>
            The agent used all {result.steps} steps without reaching a conclusion. What follows is
            the raw evidence it had gathered, with the standard description of each code — not a
            diagnosis, and not graded for severity. Run it again, or use a stronger model or a
            larger step budget in Settings.
          </div>
        </div>
      )}

      {result?.report && (
        <>
          <ShareReport result={result} vin={vin} descriptor={descriptor} />
          <ReportView
            report={result.report}
            meta={result}
            onEvidence={onEvidence}
            purpose={agent?.purpose ?? "owner"}
          />
        </>
      )}
    </div>
  );
}

/**
 * Get the report out of the window.
 *
 * Handoff section 1 asks for "a mechanic-style report I can send to a shop", and
 * there was no way to do that at all. Plain text because it has to survive being
 * pasted into a text message or a garage's booking form.
 */
function ShareReport({
  result,
  vin,
  descriptor,
}: {
  result: InspectResponse;
  vin: string | null;
  descriptor: string | null;
}) {
  const [copied, setCopied] = useState(false);

  const text = () => reportToText(result, { vin, descriptor });

  async function copy() {
    try {
      await navigator.clipboard.writeText(text());
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2500);
    } catch {
      // Clipboard access can be refused. Falling back to a selectable textarea
      // is better than a button that silently does nothing.
      setShowText(true);
    }
  }

  const [showText, setShowText] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  async function save() {
    setSaving(true);
    setSaveError(null);
    try {
      const r = await saveFile(scanFilename("report", vin, "txt"), text());
      setSaved(r.path);
    } catch (e) {
      // Said out loud. The previous version failed silently, which is the one
      // outcome a save button must never have.
      setSaveError(describeError(e).message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className="faint">Send this to a mechanic, or keep it for the seller.</span>
        <div className="row">
          <button onClick={() => void copy()}>{copied ? "Copied" : "Copy as text"}</button>
          <button
            disabled={saving}
            onClick={() => void save()}
            title="Save the report into your Downloads folder. To make a PDF, open it and print to PDF - the result is better than anything this app would generate."
          >
            {saving ? <Spinner label="Saving" /> : "Save as file"}
          </button>
          <button onClick={() => setShowText((v) => !v)}>
            {showText ? "Hide" : "Show text"}
          </button>
        </div>
      </div>

      {saved && (
        <div className="banner info" style={{ marginTop: 10, marginBottom: 0 }}>
          <span className="b-code">saved</span>
          <span className="mono" style={{ fontSize: 12, wordBreak: "break-all" }}>{saved}</span>
        </div>
      )}
      {saveError && (
        <div className="banner error" style={{ marginTop: 10, marginBottom: 0 }}>
          <span className="b-code">not saved</span>
          <span>{saveError}</span>
        </div>
      )}
      {showText && (
        <textarea
          readOnly
          value={text()}
          onFocus={(e) => e.currentTarget.select()}
          style={{
            width: "100%",
            height: 260,
            marginTop: 10,
            background: "var(--bg-inset)",
            color: "var(--text)",
            border: "1px solid var(--line)",
            borderRadius: 6,
            padding: 10,
            fontFamily: "var(--mono)",
            fontSize: 12,
          }}
        />
      )}
    </div>
  );
}

function LiveProgress({ elapsed, lines }: { elapsed: number; lines: ProgressLine[] }) {
  const mins = Math.floor(elapsed / 60);
  const clock = mins ? `${mins}m ${elapsed % 60}s` : `${elapsed}s`;
  const last = lines[lines.length - 1];

  return (
    <div className="card">
      <Spinner label={`Inspecting — ${clock}`} />

      {/* Naming the current step is the difference between "working" and
          "hung". Between tool calls the model is thinking, which on local
          hardware is most of the time, so that is said rather than left as a
          silent gap. */}
      <div style={{ marginTop: 10 }}>
        {lines.length === 0 ? (
          <span className="muted">Starting up…</span>
        ) : (
          <div className="events">
            {lines.map((l) => (
              <div key={l.seq} className="row" style={{ gap: 8, padding: "2px 0" }}>
                <span style={{ width: 14, color: l.done ? (l.ok ? "var(--ok)" : "var(--caution)") : "var(--text-faint)" }}>
                  {l.done ? (l.ok ? "✓" : "!") : "·"}
                </span>
                <span className={l.done ? "faint" : ""}>{l.text}</span>
              </div>
            ))}
          </div>
        )}
        {last?.done && (
          <div className="faint" style={{ marginTop: 6 }}>
            Thinking about what it just read…
          </div>
        )}
      </div>

      <div className="faint" style={{ marginTop: 10 }}>
        Reads take milliseconds; the waiting is the model deciding what to check next. A local
        model takes several minutes, a hosted one under a minute. You can switch tabs — this
        keeps running.
      </div>
    </div>
  );
}

function ReportView({
  report,
  meta,
  onEvidence,
  purpose,
}: {
  report: Report;
  meta: InspectResponse;
  onEvidence: (ref: number) => void;
  purpose: ScanPurpose;
}) {
  const v = verdictCopy(report.verdict, purpose);
  const costs = [...(report.findings ?? []), ...(report.watch_items ?? [])]
    .map((f) => f.estimated_cost)
    .filter((c): c is NonNullable<typeof c> => !!c);
  const band = costs.length
    ? {
        low: costs.reduce((s, c) => s + c.low, 0),
        high: costs.reduce((s, c) => s + c.high, 0),
        currency: costs[0].currency,
      }
    : null;

  return (
    <>
      <div className="card" style={{ borderLeft: `3px solid var(--${v.tone})` }}>
        <div className="row" style={{ gap: 10 }}>
          <span className="value-big" style={{ color: `var(--${v.tone})`, fontSize: 18 }}>
            {v.label}
          </span>
        </div>
        <div style={{ fontSize: 16, marginTop: 6 }}>{report.headline}</div>
        <p className="muted" style={{ marginBottom: 0 }}>{report.summary}</p>
        {/* "Inconclusive" covers two different situations and the wrong gloss
            contradicts what is on screen: a scan that read nothing, and one that
            found real faults but could not see enough to judge the whole
            vehicle. Printing "too little could be read" directly above three
            serious findings reads as a bug. */}
        <div className="faint" style={{ marginTop: 8 }}>
          {report.verdict === "inconclusive" && report.findings?.length
            ? "Faults were found, but not enough of the vehicle could be read to judge it overall. Treat the findings below as real and the absence of others as unknown."
            : v.blurb}
        </div>
      </div>

      {band && (
        <div className="card">
          <div className="row" style={{ justifyContent: "space-between" }}>
            <span className="faint">Rough total, if everything found were fixed</span>
            <span className="value-big">
              {band.currency} {Math.round(band.low)}–{Math.round(band.high)}
            </span>
          </div>
          <div className="banner caution" style={{ marginTop: 8, marginBottom: 0 }}>
            <span className="b-code">estimate</span>
            <span>
              These figures are the AI&apos;s general knowledge, not a quote and not measured from
              this vehicle. Use them to know roughly what you are looking at, then get a real quote.
            </span>
          </div>
        </div>
      )}

      {/* Always rendered, even when empty. A report with a verdict and no
          findings must not look like a complete report with nothing wrong —
          it means the model summarised without itemising, which is a different
          thing and the reader needs to know which they are looking at. */}
      <div className="section" style={{ marginTop: 20 }}>
        <h2>What was found</h2>
        {report.findings?.length ? (
          report.findings.map((f, i) => (
            <FindingCard key={i} finding={f} onEvidence={onEvidence} />
          ))
        ) : (
          <div className="banner caution">
            <span className="b-code">no findings</span>
            <span>
              The model wrote a summary but did not list any specific findings. Read the summary
              above with that in mind, check the Codes tab for what was actually read, and
              consider a stronger model in Settings.
            </span>
          </div>
        )}
      </div>

      {!!report.watch_items?.length && (
        <div className="section">
          <h2>Likely to need attention later</h2>
          <div className="banner caution">
            <span className="b-code">prediction</span>
            <span>
              Nothing below was observed failing. These are the AI&apos;s expectations for a vehicle
              in this condition, and it has no service history for this one.
            </span>
          </div>
          {report.watch_items.map((f, i) => (
            <FindingCard key={i} finding={f} onEvidence={onEvidence} />
          ))}
        </div>
      )}

      {!!report.not_checked?.length && (
        <div className="section">
          <h2>What this scan could not check</h2>
          <div className="card">
            <ul className="caveats">
              {report.not_checked.map((n, i) => <li key={i}>{n}</li>)}
            </ul>
            <div className="faint" style={{ marginTop: 6 }}>
              A plug-in scan reads electronics. It cannot see brakes, tyres, suspension, rust, or
              gearbox wear. A clean report here is not a clean bill of health.
            </div>
          </div>
        </div>
      )}

      {!!report.next_steps?.length && (
        <div className="section">
          <h2>What to do next</h2>
          <div className="card">
            <ol style={{ margin: 0, paddingLeft: 20 }}>
              {report.next_steps.map((s, i) => <li key={i} style={{ marginBottom: 4 }}>{s}</li>)}
            </ol>
          </div>
        </div>
      )}

      <div className="section">
        <h2>How this was produced</h2>
        <div className="card">
          <div className="provenance" style={{ marginTop: 0 }}>
            <span>{meta.steps} steps</span>
            <span>{meta.usage.input_tokens.toLocaleString()} tokens in</span>
            <span>{meta.usage.output_tokens.toLocaleString()} out</span>
            {meta.truncated && <span className="tag unverified">stopped early</span>}
          </div>
          <TraceList trace={meta.trace} onEvidence={onEvidence} />
        </div>
      </div>
    </>
  );
}

function FindingCard({
  finding,
  onEvidence,
}: {
  finding: Finding;
  onEvidence: (ref: number) => void;
}) {
  const src = SOURCE[finding.source] ?? SOURCE.model_knowledge;
  const tone =
    finding.severity === "critical" || finding.severity === "serious"
      ? "serious"
      : finding.severity === "caution"
        ? "caution"
        : "info";

  return (
    <div className="card" style={{ borderLeft: `3px solid var(--${tone})` }}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <strong>{finding.title}</strong>
        <div className="row">
          <span className={`tag ${finding.severity === "info" ? "" : "confirmed"}`}>
            {finding.severity}
          </span>
          <span
            className={`tag ${finding.source === "model_knowledge" ? "unverified" : ""}`}
            title={src.title}
          >
            {src.label}
          </span>
        </div>
      </div>

      <p style={{ marginBottom: 0 }}>{finding.plain_english}</p>

      {finding.what_to_do && (
        <div style={{ marginTop: 8 }}>
          <span className="faint">What to do: </span>
          {finding.what_to_do}
        </div>
      )}

      {finding.estimated_cost && (
        <div style={{ marginTop: 8 }}>
          <span className="faint">Rough cost: </span>
          <span className="mono">
            {finding.estimated_cost.currency} {Math.round(finding.estimated_cost.low)}–
            {Math.round(finding.estimated_cost.high)}
          </span>
          <div className="faint" style={{ fontSize: 12 }}>{finding.estimated_cost.basis}</div>
        </div>
      )}

      {(!!finding.evidence?.length || !!finding.evidence_refs?.length) && (
        <div className="provenance">
          {finding.evidence?.map((e, i) => <span key={i}>{e}</span>)}
          {finding.evidence_refs?.map((r) => (
            <button key={r} className="ev" onClick={() => onEvidence(r)}>
              evidence #{r}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export function TraceList({
  trace,
  onEvidence,
}: {
  trace: TraceEntry[];
  onEvidence: (ref: number) => void;
}) {
  if (!trace.length) return null;
  const reads = trace.filter((e) => e.type === "tool").length;
  return (
    // Collapsed, because it grew with the vehicle.
    //
    // A truck with two buses answers with thirty-six modules, and the agent's
    // working — every call, its arguments as raw JSON, every result — ran to
    // hundreds of lines pinned to the end of the report. Somebody looking for
    // what to do about their truck had to scroll past all of it.
    //
    // It is not deleted, and it is not summarised. It is one click away here,
    // the flight recorder holds the same record in full, and since 0.3.8 so
    // does the log file. Evidence nobody can find is not evidence; evidence
    // between somebody and their answer is not either.
    <details style={{ marginTop: 8 }}>
      <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
        every read the assistant made ({reads})
      </summary>
      <div className="events" style={{ marginTop: 8 }}>
      {trace.map((e, i) => {
        if (e.type === "thinking") {
          return (
            <div key={i} style={{ padding: "4px 0", color: "var(--text-dim)" }}>
              {e.text}
            </div>
          );
        }
        if (e.type === "tool") {
          return (
            <div key={i} style={{ padding: "2px 0" }}>
              <span className="kind">{e.name}</span>{" "}
              <span className="faint">{JSON.stringify(e.arguments)}</span>
            </div>
          );
        }
        return (
          <div key={i} style={{ padding: "2px 0 6px 12px" }}>
            <span className={e.success ? "faint" : "cls-bus_error"}>
              {e.success ? "ok" : "failed"}
            </span>
            {e.evidence_ref != null && (
              <button className="ev" style={{ marginLeft: 8 }} onClick={() => onEvidence(e.evidence_ref!)}>
                evidence #{e.evidence_ref}
              </button>
            )}
          </div>
        );
      })}
      </div>
    </details>
  );
}
