// The small pieces every screen shares.
//
// These exist mostly to make the API's honesty markers hard to drop by
// accident: warnings always render, an unverified decoder always says so, and
// every decoded value can be traced back to the bytes it came from.

import { useEffect, useRef, useState } from "react";
import type {
  ApiError, ConnectionState, DecodedValue, ToolResult, Warning,
} from "../api/types";
import { useExplain } from "../explain";

export function Pill({ state }: { state: ConnectionState }) {
  const s = state.state;
  const cls =
    s === "ready" ? "ready"
    : s === "degraded" ? "degraded"
    : s === "failed" ? "failed"
    : s === "disconnected" ? ""
    : "busy";
  let label: string = s;
  if (state.state === "reconnecting") label = `reconnecting (attempt ${state.attempt})`;
  return (
    <span className="pill" title={stateDetail(state) ?? undefined}>
      <span className={`dot ${cls}`} />
      {label}
    </span>
  );
}

export function stateDetail(state: ConnectionState): string | null {
  if (state.state === "degraded") return state.reason;
  if (state.state === "failed") return `${state.code}: ${state.detail}`;
  return null;
}

export function Warnings({ warnings }: { warnings: Warning[] }) {
  if (!warnings.length) return null;
  return (
    <div>
      {warnings.map((w, i) => (
        <CodedBanner
          key={`${w.code}-${i}`}
          severity={w.severity}
          code={w.code}
          message={w.message}
        />
      ))}
    </div>
  );
}

/** A request the core rejected structurally, or an unreachable core. */
export function ErrorBanner({ error }: { error: { code: string; message: string } | null }) {
  if (!error) return null;
  return <CodedBanner severity="error" code={error.code} message={error.message} />;
}

/**
 * A banner carrying a machine code, explained.
 *
 * Every warning the core emits is a short identifier plus a sentence written
 * for someone who already knows the domain. `calibration_unavailable: service
 * 09 info type 06: module 7E8 sent an incomplete multi-frame response` is
 * precise and completely opaque, and it was the top-of-screen message a first
 * time user was being shown.
 *
 * In Easy mode the plain-language version replaces it and the identifier is
 * dropped. In Advanced both are kept, because the identifier is what an expert
 * searches for. When this build has no explanation for a code the original
 * message stands — a gap, visibly, rather than a silence.
 */
export function CodedBanner({
  severity,
  code,
  message,
  children,
}: {
  severity: string;
  code: string;
  message: string;
  children?: React.ReactNode;
}) {
  const { easy, lookup } = useExplain();
  const e = lookup("code", code);
  return (
    <div className={`banner ${severity}`}>
      {!easy && <span className="b-code">{code}</span>}
      <div>
        <div>{easy && e ? e.easy : message}</div>
        {!easy && e && <div className="explain">{e.technical}</div>}
        {children}
      </div>
    </div>
  );
}

/**
 * A number that slides to its new value instead of snapping.
 *
 * Purely presentational, and deliberately quick: the reading is real data and
 * must not appear to lag behind the vehicle, so the tween is short enough to
 * finish well inside one sample period at any interval the UI offers.
 */
export function AnimatedNumber({
  value,
  format,
}: {
  value: number;
  format: (n: number) => string;
}) {
  const [shown, setShown] = useState(value);
  const from = useRef(value);
  const started = useRef(0);

  useEffect(() => {
    const DURATION = 220;
    from.current = shown;
    started.current = performance.now();
    let raf = 0;
    const step = (now: number) => {
      const t = Math.min(1, (now - started.current) / DURATION);
      // Ease-out cubic: fast to most of the way, then settles.
      const k = 1 - Math.pow(1 - t, 3);
      setShown(from.current + (value - from.current) * k);
      if (t < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
    // Re-running on `shown` would restart the tween every frame.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  return <>{format(shown)}</>;
}

/**
 * A ToolResult that came back with success: false. This is a diagnostic
 * outcome, not an HTTP failure, so it renders its evidence alongside the error.
 */
export function FailedResult({
  result,
  onEvidence,
}: {
  result: ToolResult<unknown>;
  onEvidence?: (ref: number) => void;
}) {
  const err = result.error;
  return (
    <div>
      <div className="banner error">
        <span className="b-code">{err?.code ?? "failed"}</span>
        <div>
          <div>{err?.message ?? `${result.tool} did not succeed`}</div>
          {err?.details && (
            <pre className="mono faint" style={{ margin: "6px 0 0", whiteSpace: "pre-wrap" }}>
              {JSON.stringify(err.details, null, 2)}
            </pre>
          )}
          {result.raw_evidence_ref != null && onEvidence && (
            <button className="ev" style={{ marginTop: 6 }} onClick={() => onEvidence(result.raw_evidence_ref!)}>
              show the adapter exchange (event #{result.raw_evidence_ref})
            </button>
          )}
        </div>
      </div>
      <Warnings warnings={result.warnings} />
      {err?.capability_state?.caveats?.length ? (
        <div className="card">
          <div className="faint" style={{ marginBottom: 6 }}>
            what the adapter was observed to be when this failed
          </div>
          <ul className="caveats">
            {err.capability_state.caveats.map((c, i) => <li key={i}>{c}</li>)}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

export function formatScalar(v: DecodedValue["value"]): string {
  switch (v.type) {
    case "number":
      return Number.isInteger(v.value) ? String(v.value) : v.value.toFixed(2);
    case "integer":
      return String(v.value);
    case "boolean":
      return v.value ? "true" : "false";
    case "text":
      return v.value;
    case "raw":
      return v.value;
    case "flags":
      return v.value.filter((f) => f.set).map((f) => f.label).join(", ") || "none set";
    default:
      return "";
  }
}

/**
 * One decoded reading, with its provenance attached.
 *
 * docs/API.md: never show an unverified value as a measurement. So an
 * unverified reading is labelled and shown as raw evidence, and one outside its
 * declared range is marked suspect rather than quietly plotted.
 */
export function Value({
  value,
  onEvidence,
  animate = false,
  stale = false,
  compact = false,
}: {
  value: DecodedValue;
  onEvidence?: (ref: number) => void;
  /** Tween the number between samples. Only worth it on a live feed. */
  animate?: boolean;
  /** This came from an earlier sample, not the latest one. */
  stale?: boolean;
  /** One of many on a dashboard: prose goes behind a disclosure. */
  compact?: boolean;
}) {
  const { easy, lookup } = useExplain();
  const p = value.provenance;
  const unverified = p.verification !== "verified";
  const explanation = lookup("signal", value.signal_id);
  const numeric = value.value.type === "number" || value.value.type === "integer";

  return (
    <div className="card value-card">
      <div className="row" style={{ justifyContent: "space-between", flexWrap: "nowrap" }}>
        <div style={{ minWidth: 0 }}>
          {/* The identifier is what an expert searches for and pure noise to
              everyone else, so it only exists in Advanced. */}
          {!easy && <div className="faint mono" style={{ fontSize: 11 }}>{value.signal_id}</div>}
          <div style={{ fontWeight: 500 }}>{value.name}</div>
        </div>
        {/* Only an actual number gets the big-number treatment.

            A bitmask or a flag list rendered at 22px monospace with nowrap does
            not fit in a card and does not try to: "PIDs supported:
            01,04,05,0B,0C,0D,0F,10" ran straight out of its box and over the
            panel beside it. Those are real values and worth showing — they are
            just not gauges, so they wrap at normal size instead. */}
        {numeric ? (
          <div style={{ textAlign: "right", whiteSpace: "nowrap" }}>
            <span className={`value-big${value.out_of_range ? " value-suspect" : ""}${stale ? " value-stale" : ""}`}>
              {animate ? (
                <AnimatedNumber
                  value={(value.value as { value: number }).value}
                  format={(n) => (Number.isInteger((value.value as { value: number }).value)
                    ? String(Math.round(n))
                    : n.toFixed(2))}
                />
              ) : (
                formatScalar(value.value)
              )}
            </span>
            {value.unit && <span className="value-unit">{value.unit}</span>}
          </div>
        ) : null}
      </div>

      {!numeric && (
        <div className={`value-text${stale ? " value-stale" : ""}`}>
          {formatScalar(value.value)}
          {value.unit ? ` ${value.unit}` : ""}
        </div>
      )}

      {stale && (
        <div className="faint" style={{ fontSize: 11, marginTop: 4 }}>
          from an earlier sample — the adapter has not answered this one yet
        </div>
      )}

      {/* What this reading actually means.

          On a dashboard of thirty cards this is three lines of prose each and
          the screen becomes unreadable, so there it goes behind a disclosure.
          On a single card it is the point and stays open. */}
      {explanation && (compact ? (
        <details>
          <summary className="faint" style={{ cursor: "pointer", fontSize: 11 }}>
            what is this?
          </summary>
          <div className="explain">{easy ? explanation.easy : explanation.technical}</div>
        </details>
      ) : (
        <div className="explain">{easy ? explanation.easy : explanation.technical}</div>
      ))}

      {unverified && (
        <CodedBanner
          severity="caution"
          code="unverified"
          message="This decoder definition has not been validated against a real vehicle. Treat the number as raw evidence, not a measurement."
        />
      )}
      {value.out_of_range && (
        <CodedBanner
          severity="serious"
          code="out_of_range"
          message={`Outside the declared valid range${
            value.valid_range ? ` (${value.valid_range.min} to ${value.valid_range.max})` : ""
          }. Suspect.`}
        />
      )}

      {/* Provenance is the promise this project makes: every number can be
          traced to the bytes behind it. Easy mode shrinks it to the one link
          that matters rather than removing it — a simpler screen must not be a
          less honest one. */}
      {easy ? (
        p.evidence_ref != null && onEvidence ? (
          <div className="provenance">
            <button className="ev" onClick={() => onEvidence(p.evidence_ref!)}>
              show me where this number came from
            </button>
          </div>
        ) : null
      ) : (
        <div className="provenance">
          <span>{p.decoder_id ?? "no decoder id"}{p.decoder_version ? ` v${p.decoder_version}` : ""}</span>
          {p.raw_hex && <span>raw {p.raw_hex}</span>}
          <span className={unverified ? "tag unverified" : ""}>{p.verification}</span>
          {p.evidence_ref != null && onEvidence && (
            <button className="ev" onClick={() => onEvidence(p.evidence_ref!)}>
              evidence #{p.evidence_ref}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export function Spinner({ label }: { label?: string }) {
  return (
    <span className="row faint" style={{ gap: 8 }}>
      <span className="spin" />
      {label}
    </span>
  );
}

export function localTime(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleTimeString();
}

export function apiErrorOf(e: ApiError | null | undefined) {
  return e ? { code: e.code, message: e.message } : null;
}
