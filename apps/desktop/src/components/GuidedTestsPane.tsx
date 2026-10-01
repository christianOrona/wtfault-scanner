// Guided tests: put the car in a state, and measure it while it is there.
//
// Some readings only mean something in a particular state - under load at a
// steady 2500 rpm, at a warm idle, with the key on and the engine off. This
// asks for the state one thing at a time, reads the car to see whether it
// holds, and only measures once the car - never the person - says it does.
//
// Nothing here asks anybody to drive. A test that does not apply to this
// engine says so and asks nothing.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, describeError } from "../api/client";
import type {
  ProcedureCheck,
  ProcedureInfo,
  ProcedureMeasurement,
  ToolResult,
} from "../api/types";
import { ErrorBanner, FailedResult, Spinner, Value, Warnings } from "./primitives";
import { PaneIntro, useExplain } from "../explain";

/** How often to re-read the car while somebody works towards the state. */
const WATCH_MS = 2000;

export function GuidedTestsPane({
  connected,
  onEvidence,
}: {
  connected: boolean;
  onEvidence?: (ref: number) => void;
}) {
  const [procedures, setProcedures] = useState<ProcedureInfo[]>([]);
  const [chosen, setChosen] = useState<string | null>(null);
  const [check, setCheck] = useState<ToolResult<ProcedureCheck> | null>(null);
  const [measured, setMeasured] = useState<ToolResult<ProcedureMeasurement> | null>(null);
  const [busy, setBusy] = useState<"check" | "run" | null>(null);
  const [watching, setWatching] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const { easy } = useExplain();
  const inFlight = useRef(false);

  useEffect(() => {
    api
      .procedures()
      .then((r) => setProcedures(r.procedures))
      .catch((e) => setError(describeError(e)));
  }, []);

  const runCheck = useCallback(async (id: string) => {
    // A watch tick that lands while the last read is still out is skipped,
    // not queued: a slow adapter must not build up a backlog of reads.
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy("check");
    setError(null);
    try {
      const r = await api.checkProcedure(id);
      setCheck(r);
      // Nothing more to watch for once it cannot or does not apply.
      const state = r.data?.state;
      if (!r.success || state === "does_not_apply" || state === "refused") setWatching(false);
    } catch (e) {
      setError(describeError(e));
      setWatching(false);
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }, []);

  useEffect(() => {
    if (!watching || !chosen) return;
    const timer = window.setInterval(() => void runCheck(chosen), WATCH_MS);
    return () => window.clearInterval(timer);
  }, [watching, chosen, runCheck]);

  const choose = (id: string) => {
    setChosen(id);
    setCheck(null);
    setMeasured(null);
    setWatching(false);
    if (connected) void runCheck(id);
  };

  const measure = async () => {
    if (!chosen) return;
    setWatching(false);
    setBusy("run");
    setError(null);
    try {
      setMeasured(await api.runProcedure(chosen));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  };

  const info = procedures.find((p) => p.id === chosen) ?? null;
  const data = check?.success ? check.data : null;
  const holding = data?.state === "holding";

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="guided_tests" />

      <div style={{ marginBottom: 12 }}>
        <strong>{easy ? "Tests that need the car in a certain state" : "Guided tests"}</strong>
        <div className="faint">
          Some faults only show under load or once the engine is warm. Pick a test, do the one
          thing it asks, and it measures once the car says it is ready.
        </div>
      </div>

      {!connected && (
        <div className="banner caution">
          <span className="b-code">not connected</span>
          <span>Connect to a vehicle first.</span>
        </div>
      )}
      <ErrorBanner error={error} />

      <div className="row" style={{ gap: 8, flexWrap: "wrap", marginBottom: 12 }}>
        {procedures.map((p) => (
          <button
            key={p.id}
            className={p.id === chosen ? "primary" : undefined}
            onClick={() => choose(p.id)}
            disabled={!p.can_run_alone}
            title={p.why_not_alone ?? p.purpose}
          >
            {p.name}
          </button>
        ))}
      </div>

      {info && (
        <div className="card">
          <div>{info.purpose}</div>
          {info.safety_notes.length > 0 && data?.state !== "does_not_apply" && (
            <div className="banner caution" style={{ marginTop: 8 }}>
              <span className="b-code">before you start</span>
              <div>
                {info.safety_notes.map((n) => (
                  <div key={n}>{n}</div>
                ))}
              </div>
            </div>
          )}

          {check && !check.success && <FailedResult result={check} />}
          {/* "Does not apply" has its own banner below, with the reason. */}
          {check && (
            <Warnings
              warnings={check.warnings.filter((w) => w.code !== "procedure_does_not_apply")}
            />
          )}

          {data?.state === "does_not_apply" && (
            <div className="banner info" style={{ marginTop: 8 }}>
              <span className="b-code">not for this engine</span>
              <span>{data.does_not_apply_because}</span>
            </div>
          )}

          {data?.conditions && (
            <>
              <table style={{ marginTop: 8 }}>
                <tbody>
                  {data.conditions.map((c, i) => (
                    <tr key={i}>
                      <td style={{ width: 24, color: c.met ? "var(--ok)" : "var(--caution)" }}>
                        {c.met ? "✓" : "…"}
                      </td>
                      <td>
                        <div>{c.instruction}</div>
                        <div className="faint" style={{ fontSize: 12 }}>
                          {c.value != null
                            ? `${c.signal ?? "reading"}: ${Math.round(c.value * 10) / 10}`
                            : c.unmeasurable ?? "not read yet"}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>

              {data.next_step && (
                <div className={`banner ${holding ? "info" : "caution"}`} style={{ marginTop: 8 }}>
                  <span className="b-code">{holding ? "ready" : "next"}</span>
                  <span>{data.next_step}</span>
                </div>
              )}

              {data.not_on_this_engine && data.not_on_this_engine.length > 0 && (
                <div className="faint" style={{ fontSize: 12, marginTop: 6 }}>
                  Not measured on this engine, and not needed: {data.not_on_this_engine.join(", ")}
                </div>
              )}
            </>
          )}

          <div className="row" style={{ marginTop: 10, gap: 8 }}>
            <button onClick={() => chosen && void runCheck(chosen)} disabled={!connected || !!busy}>
              {busy === "check" && !watching ? <Spinner label="Reading the car" /> : "Check again"}
            </button>
            <label className="row" style={{ gap: 4 }}>
              <input
                type="checkbox"
                checked={watching}
                disabled={!connected || data?.state === "does_not_apply"}
                onChange={(e) => setWatching(e.target.checked)}
              />
              keep checking every {WATCH_MS / 1000} s
            </label>
            <button
              className="primary"
              onClick={() => void measure()}
              disabled={!connected || !holding || !!busy}
              title={holding ? "" : "Measures only once the car is in the state the test needs."}
            >
              {busy === "run" ? <Spinner label="Measuring" /> : "Measure"}
            </button>
          </div>
        </div>
      )}

      {measured && (
        <div className="card">
          {!measured.success && <FailedResult result={measured} />}
          <Warnings warnings={measured.warnings} />
          {measured.data?.held_throughout === false && (
            <div className="banner caution">
              <span className="b-code">state lost</span>
              <span>The car left the state while these were read. Treat them as ordinary readings.</span>
            </div>
          )}
          {measured.data?.complete && (
            <div className="banner info">
              <span className="b-code">measured</span>
              <span>Everything this test exists to measure was read while the state held.</span>
            </div>
          )}
          <table style={{ marginTop: 8 }}>
            <tbody>
              {measured.values.map((v) => (
                <tr key={v.signal_id}>
                  <td>{v.name}</td>
                  <td>
                    <Value value={v} onEvidence={onEvidence} compact />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {procedures.length === 0 && !error && <div className="empty">Loading the tests…</div>}
    </div>
  );
}
