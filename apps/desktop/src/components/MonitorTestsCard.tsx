// On-board monitor test results (OBD service 06), and the question they answer:
// "what is about to go wrong that no warning light will tell me about?"
//
// A trouble code is a component that has *already* failed. Service 06 is the
// step before that: for each emissions self-test, the vehicle reports the value
// it measured next to the limit it judges that value by. A catalyst reading 0.58
// against a 0.60 limit is passing — no code, no light, nothing to see from the
// driver's seat — and it is going to stop passing. That gap is the whole reason
// this screen exists, and it is the reading that separates this from a code
// reader.
//
// Two honesty rules are visible in the UI:
//
//  * Pass, fail, and how close to the limit are *exact*, always. The measured
//    value and both limits arrive in the same unit, so comparing them needs no
//    knowledge of what that unit is.
//  * The scaled numbers do need that knowledge, and this build's unit table has
//    not been checked against a real vehicle. So they are shown as supporting
//    detail behind a disclosure, marked unverified, next to the raw counts.
//
// Plenty of vehicles — most built before roughly 2005 — do not implement service
// 06 at all. That is a fact about the vehicle and it is said plainly, not
// dressed up as an error.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { MonitorReading, MonitorTestsData, ToolResult } from "../api/types";
import { ErrorBanner, Spinner } from "./primitives";

/** Under this much of the limit band left, a passing result is worth flagging. */
const MARGINAL = 0.1;

/** How a single result should be read. */
type Standing = "failed" | "marginal" | "passed";

function standing(m: MonitorReading, threshold: number): Standing {
  if (!m.passed) return "failed";
  if (m.margin != null && m.margin < threshold) return "marginal";
  return "passed";
}

const TONE: Record<Standing, string> = {
  failed: "var(--serious)",
  marginal: "var(--caution)",
  passed: "var(--ok)",
};

const LABEL: Record<Standing, string> = {
  failed: "outside limits",
  marginal: "close to limit",
  passed: "within limits",
};

export function MonitorTestsCard({
  moduleKey,
  onEvidence,
}: {
  moduleKey: string | null;
  onEvidence: (ref: number) => void;
}) {
  const [result, setResult] = useState<ToolResult<MonitorTestsData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  const read = useCallback(async () => {
    if (!moduleKey) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await api.monitorTests(moduleKey));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, [moduleKey]);

  useEffect(() => {
    setResult(null);
    void read();
  }, [read]);

  if (!moduleKey) return null;

  const data = result?.success ? result.data : null;

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div>
          <strong>Self-test measurements</strong>
          <div className="faint">
            What the car measured, next to the limit it judges itself by — this is where a
            part that is wearing out shows up before it sets a code.
          </div>
        </div>
        <button onClick={() => void read()} disabled={busy}>
          {busy ? <Spinner label="Reading" /> : "Re-check"}
        </button>
      </div>

      <ErrorBanner error={error} />

      {result && !result.success && (
        <div className="banner caution" style={{ marginTop: 10, marginBottom: 0 }}>
          <span className="b-code">{result.error?.code ?? "unavailable"}</span>
          <span>{result.error?.message ?? "This module did not answer."}</span>
        </div>
      )}

      {data && !data.supported && (
        <div className="banner info" style={{ marginTop: 12, marginBottom: 0 }}>
          <span className="b-code">not offered</span>
          <span>
            This vehicle does not report its self-test measurements. It is optional, and
            most vehicles built before roughly 2005 leave it out. Nothing is wrong — this
            particular check simply is not available here.
          </span>
        </div>
      )}

      {data && data.supported && (
        <MonitorBody data={data} result={result!} onEvidence={onEvidence} />
      )}
    </div>
  );
}

function MonitorBody({
  data,
  result,
  onEvidence,
}: {
  data: MonitorTestsData;
  result: ToolResult<MonitorTestsData>;
  onEvidence: (ref: number) => void;
}) {
  const threshold = data.marginal_threshold ?? MARGINAL;
  const rows = data.monitors.map((m) => ({ m, s: standing(m, threshold) }));
  const failed = rows.filter((r) => r.s === "failed");
  const marginal = rows.filter((r) => r.s === "marginal");

  if (rows.length === 0) {
    return (
      <div className="banner info" style={{ marginTop: 12, marginBottom: 0 }}>
        <span className="b-code">nothing yet</span>
        <span>
          This module offers self-test measurements but has not produced any. Monitors
          only run under specific conditions; a drive and a re-check usually fills them in.
        </span>
      </div>
    );
  }

  return (
    <>
      <div className="row" style={{ marginTop: 12, gap: 20 }}>
        <Stat label="Tests reported" value={String(rows.length)} />
        <Stat
          label="Outside limits"
          value={String(failed.length)}
          tone={failed.length ? "var(--serious)" : "var(--ok)"}
        />
        <Stat
          label="Close to limit"
          value={String(marginal.length)}
          tone={marginal.length ? "var(--caution)" : "var(--ok)"}
        />
      </div>

      {marginal.length > 0 && (
        <div className="banner caution" style={{ marginTop: 12, marginBottom: 0 }}>
          <span className="b-code">wearing out</span>
          <div>
            <strong>
              {marginal.length} self-test{marginal.length === 1 ? " is" : "s are"} passing but
              close to failing.
            </strong>
            <div style={{ marginTop: 6 }}>
              Nothing here has set a warning light, and nothing has to be fixed today. But
              these are the parts heading for a code: {marginal.map((r) => r.m.name).join(", ")}.
              Worth pricing before you buy, and worth re-checking after a longer drive, since a
              single test can be unrepresentative.
            </div>
          </div>
        </div>
      )}

      {failed.length > 0 && (
        <div className="banner serious" style={{ marginTop: 12, marginBottom: 0 }}>
          <span className="b-code">failing</span>
          <div>
            <strong>
              {failed.length} self-test{failed.length === 1 ? "" : "s"} measured outside the
              limits the car itself sets.
            </strong>
            <div style={{ marginTop: 6 }}>
              {failed.map((r) => r.m.name).join(", ")}. A failure here normally becomes a stored
              code and a warning light once the car has confirmed it on a second drive.
            </div>
          </div>
        </div>
      )}

      {!failed.length && !marginal.length && (
        <div className="banner info" style={{ marginTop: 12, marginBottom: 0 }}>
          <span className="b-code">all clear</span>
          <span>
            Every self-test the car reported is comfortably inside its own limits — not just
            passing, but with room to spare. This is the strongest emissions evidence an
            OBD-II scan can give.
          </span>
        </div>
      )}

      <table style={{ marginTop: 12 }}>
        <tbody>
          {rows.map(({ m, s }) => (
            <tr key={`${m.mid}-${m.tid}`}>
              <td>
                {m.name}
                {m.unknown_monitor && (
                  <span className="faint" style={{ marginLeft: 6, fontSize: 11 }}>
                    not in this build's catalogue
                  </span>
                )}
                {m.system && (
                  <div className="faint" style={{ fontSize: 11 }}>
                    {m.system}
                  </div>
                )}
              </td>
              <td style={{ width: 150 }}>
                <MarginBar margin={m.margin} standing={s} />
              </td>
              <td style={{ width: 130 }}>
                <span className="tag" style={{ color: TONE[s], borderColor: "var(--line)" }}>
                  {LABEL[s]}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <details style={{ marginTop: 10 }}>
        <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
          the numbers behind each test
        </summary>
        <div className="banner info" style={{ marginTop: 8 }}>
          <span className="b-code">unverified</span>
          <span>
            Pass, fail and how close to the limit are exact — the car sends the measurement
            and its limits in the same unit, so comparing them needs nothing from us. Turning
            them into volts or degrees does, and that conversion table has not been checked
            against a real vehicle by this project. Raw counts are shown alongside.
          </span>
        </div>
        <table style={{ marginTop: 6 }}>
          <thead>
            <tr>
              <th>Test</th>
              <th>Measured</th>
              <th>Limits</th>
              <th>Raw</th>
            </tr>
          </thead>
          <tbody>
            {rows.map(({ m }) => (
              <tr key={`raw-${m.mid}-${m.tid}`}>
                <td className="mono">
                  {hex(m.mid)}.{hex(m.tid)}
                </td>
                <td className="mono">{scaled(m.value, m.unit)}</td>
                <td className="mono">
                  {scaled(m.min, m.unit)} – {scaled(m.max, m.unit)}
                </td>
                <td className="mono faint">
                  {m.raw.value} in {m.raw.min}–{m.raw.max}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>

      {result.raw_evidence_ref != null && (
        <div className="provenance">
          <button className="ev" onClick={() => onEvidence(result.raw_evidence_ref!)}>
            evidence #{result.raw_evidence_ref}
          </button>
          <span>read from {result.module}</span>
        </div>
      )}
    </>
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
      <div className="value-big" style={{ fontSize: 16, color: tone }}>
        {value}
      </div>
    </div>
  );
}

/**
 * How much of the limit band is left, drawn.
 *
 * The bar fills toward the limit, so a short bar is healthy and a full bar is
 * about to fail — which is the direction a person reads intuitively. A failed
 * test is drawn full rather than overflowing.
 */
function MarginBar({ margin, standing }: { margin: number | null; standing: Standing }) {
  if (margin == null) {
    return (
      <span className="faint" style={{ fontSize: 11 }}>
        limits give no range
      </span>
    );
  }
  // margin is headroom (0.5 = mid-band, 0 = at the limit). Invert it so the bar
  // grows as the part gets worse.
  const used = standing === "failed" ? 1 : Math.min(1, Math.max(0, 1 - margin / 0.5));
  return (
    <div title={`${Math.round(margin * 100)}% of the limit band remaining`}>
      <div
        style={{
          height: 6,
          borderRadius: 3,
          background: "var(--bg-inset)",
          border: "1px solid var(--line-soft)",
          overflow: "hidden",
        }}
      >
        <div style={{ width: `${used * 100}%`, height: "100%", background: TONE[standing] }} />
      </div>
      <div className="faint" style={{ fontSize: 11, marginTop: 2 }}>
        {standing === "failed" ? "past the limit" : `${Math.round(margin * 100)}% margin left`}
      </div>
    </div>
  );
}

function hex(n: number): string {
  return n.toString(16).toUpperCase().padStart(2, "0");
}

function scaled(v: number | null, unit: string | null): string {
  if (v == null) return "—";
  const n = Math.abs(v) >= 100 ? v.toFixed(0) : v.toFixed(3).replace(/0+$/, "").replace(/\.$/, "");
  return unit ? `${n} ${unit}` : n;
}
