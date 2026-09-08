// What changed between two scans of the same vehicle.
//
// The most useful diagnostic question a single scan cannot answer, and every
// scan needed to answer it was already sitting in the database unused. A fuel
// trim reading 9% is mildly interesting; 9% when it was 2% in March is a
// diagnosis. A catalyst self-test at 88% of its limit means little until you
// know it was at 61% last year.
//
// Deliberately restrained about what a difference means. Conditions are not
// controlled between two visits — a cold engine and a warm one produce very
// different numbers for entirely honest reasons — so this reports movement and
// leaves the conclusion to the reader.

import { useState } from "react";
import { api, describeError } from "../api/client";
import type { ComparisonResponse, SessionSummary } from "../api/types";
import { ErrorBanner, Spinner, localTime } from "./primitives";

export function CompareSessions({ sessions }: { sessions: SessionSummary[] }) {
  const [before, setBefore] = useState("");
  const [after, setAfter] = useState("");
  const [result, setResult] = useState<ComparisonResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  async function run() {
    if (!before || !after || before === after) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await api.compareSessions(before, after));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const c = result?.comparison;
  const appeared = c?.faults.filter((f) => f.change === "appeared") ?? [];
  const gone = c?.faults.filter((f) => f.change === "gone") ?? [];
  const notable = new Set(result?.notable_signals ?? []);

  const options = sessions.map((s) => (
    <option key={s.session.id} value={s.session.id}>
      {localTime(s.session.started_at)}
      {s.session.label ? ` — ${s.session.label}` : ""}
    </option>
  ));

  return (
    <div className="section" style={{ marginTop: 24 }}>
      <h2>Compare two scans</h2>
      <div className="explain">
        A single scan says what is true today. Two say whether something is drifting — which
        is the only way a fuel trim or a catalyst measurement can tell you anything.
      </div>

      <div className="row" style={{ marginTop: 10 }}>
        <select value={before} onChange={(e) => setBefore(e.target.value)}>
          <option value="">earlier scan…</option>
          {options}
        </select>
        <select value={after} onChange={(e) => setAfter(e.target.value)}>
          <option value="">later scan…</option>
          {options}
        </select>
        <button
          className="primary"
          onClick={() => void run()}
          disabled={busy || !before || !after || before === after}
        >
          {busy ? <Spinner label="Comparing" /> : "Compare"}
        </button>
      </div>

      <ErrorBanner error={error} />

      {c && (
        <>
          {/* Comparing two different vehicles produces confident nonsense, so
              it is checked rather than assumed. */}
          {c.same_vehicle === false && (
            <div className="banner serious" style={{ marginTop: 10 }}>
              <span className="b-code">different vehicles</span>
              <span>These two scans recorded different vehicles. Nothing below applies.</span>
            </div>
          )}
          {c.same_vehicle === null && (
            <div className="banner caution" style={{ marginTop: 10 }}>
              <span className="b-code">unconfirmed</span>
              <span>
                At least one of these scans did not read a serial number, so this cannot
                confirm they are the same vehicle.
              </span>
            </div>
          )}

          <div className="row" style={{ gap: 24, marginTop: 12 }}>
            <Stat
              label="Apart"
              value={
                Math.abs(c.days_apart) < 1
                  ? `${Math.round(Math.abs(c.days_apart) * 24)} h`
                  : `${Math.round(Math.abs(c.days_apart))} days`
              }
            />
            <Stat
              label="New faults"
              value={String(appeared.length)}
              tone={appeared.length ? "var(--serious)" : "var(--ok)"}
            />
            <Stat label="No longer reported" value={String(gone.length)} />
          </div>

          {appeared.length > 0 && (
            <div className="banner serious" style={{ marginTop: 12 }}>
              <span className="b-code">new</span>
              <div>
                {appeared.map((f) => (
                  <div key={f.code}>
                    <strong>{f.code}</strong> {f.description ?? ""}
                  </div>
                ))}
              </div>
            </div>
          )}

          {gone.length > 0 && (
            <div className="banner info" style={{ marginTop: 12 }}>
              <span className="b-code">gone</span>
              <div>
                {gone.map((f) => (
                  <div key={f.code}>
                    <strong>{f.code}</strong> {f.description ?? ""}
                  </div>
                ))}
                <div className="explain" style={{ marginTop: 6 }}>
                  A code disappears when it is repaired and when somebody clears it. This
                  comparison cannot tell those apart — the readiness monitors can.
                </div>
              </div>
            </div>
          )}

          {c.signals.length > 0 && (
            <table style={{ marginTop: 12 }}>
              <thead>
                <tr>
                  <th>Reading</th>
                  <th>Was</th>
                  <th>Now</th>
                  <th>Change</th>
                </tr>
              </thead>
              <tbody>
                {c.signals.slice(0, 20).map((s) => (
                  <tr key={s.signal_id}>
                    <td>
                      {s.signal_id}
                      {/* A mean over one reading is not an average. Saying so
                          stops a single noisy sample reading as a trend. */}
                      {(s.samples_before < 3 || s.samples_after < 3) && (
                        <span className="faint" style={{ fontSize: 11 }}> · few samples</span>
                      )}
                    </td>
                    <td className="num">
                      {s.before.toFixed(2)}
                      {s.unit ? ` ${s.unit}` : ""}
                    </td>
                    <td className="num">
                      {s.after.toFixed(2)}
                      {s.unit ? ` ${s.unit}` : ""}
                    </td>
                    <td
                      className="num"
                      style={{ color: notable.has(s.signal_id) ? "var(--caution)" : undefined }}
                    >
                      {s.delta >= 0 ? "+" : ""}
                      {s.delta.toFixed(2)}
                      {s.relative != null && ` (${(s.relative * 100).toFixed(0)}%)`}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {c.signals.length === 0 && appeared.length === 0 && gone.length === 0 && (
            <div className="banner info" style={{ marginTop: 12 }}>
              <span className="b-code">nothing to compare</span>
              <span>
                These two scans have no readings in common. Comparing needs the same signal
                read in both, so run the same checks on each visit.
              </span>
            </div>
          )}
        </>
      )}
    </div>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div>
      <div
        className="faint"
        style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}
      >
        {label}
      </div>
      <div className="value-big" style={{ fontSize: 16, color: tone }}>{value}</div>
    </div>
  );
}
