// Emissions readiness, and the question it really answers:
// "has someone just cleared the codes to hide something?"
//
// This is the single most useful check a used-car buyer can make, and it is
// invisible from the driver's seat. Clearing the trouble codes also resets every
// self-test monitor to "not complete", and they only finish again after the car
// has been driven through specific conditions — typically 50-100 miles of mixed
// driving. So a car with no stored codes but incomplete monitors was very
// probably cleared shortly before you arrived.
//
// It is not proof of dishonesty: a recently disconnected battery does the same
// thing. But it is a fact worth knowing and worth asking about, and this screen
// states it plainly rather than leaving it in a bitfield.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { DecodedValue, ReadinessData, ToolResult } from "../api/types";
import { ErrorBanner, Spinner, Warnings } from "./primitives";

/** A monitor and the pair of flags that describe it. */
interface Monitor {
  supported: string;
  incomplete: string;
  /** Name on a petrol engine. */
  petrol: string;
  /** Name on a diesel. The same bits mean different systems. */
  diesel: string | null;
}

const CONTINUOUS: Monitor[] = [
  { supported: "misfire_supported", incomplete: "misfire_incomplete", petrol: "Misfire", diesel: "Misfire" },
  { supported: "fuel_supported", incomplete: "fuel_incomplete", petrol: "Fuel system", diesel: "Fuel system" },
  { supported: "component_supported", incomplete: "component_incomplete", petrol: "Engine components", diesel: "Engine components" },
];

// Bytes C and D carry the same bit positions for both engine types, but the
// systems differ, so a diesel must not be told it has a catalyst monitor.
const NON_CONTINUOUS: Monitor[] = [
  { supported: "catalyst_supported", incomplete: "catalyst_incomplete", petrol: "Catalytic converter", diesel: "NMHC catalyst" },
  { supported: "heated_catalyst_supported", incomplete: "heated_catalyst_incomplete", petrol: "Heated catalytic converter", diesel: "NOx / SCR aftertreatment" },
  { supported: "evap_supported", incomplete: "evap_incomplete", petrol: "Fuel vapour system", diesel: null },
  { supported: "secondary_air_supported", incomplete: "secondary_air_incomplete", petrol: "Secondary air injection", diesel: "Boost pressure" },
  { supported: "ac_refrigerant_supported", incomplete: "ac_refrigerant_incomplete", petrol: "Air-conditioning refrigerant", diesel: null },
  { supported: "o2_sensor_supported", incomplete: "o2_sensor_incomplete", petrol: "Oxygen sensors", diesel: "Exhaust gas sensor" },
  { supported: "o2_heater_supported", incomplete: "o2_heater_incomplete", petrol: "Oxygen sensor heaters", diesel: "Particulate filter" },
  { supported: "egr_supported", incomplete: "egr_incomplete", petrol: "Exhaust gas recirculation", diesel: "EGR / variable valve timing" },
];

interface Reading {
  flags: Record<string, boolean>;
  dtcCount: number | null;
  distanceSinceCleared: number | null;
  distanceUnit: string | null;
  warmupsSinceCleared: number | null;
  milOn: boolean;
}

function parseValues(values: DecodedValue[]): Reading | null {
  const byId = new Map(values.map((v: DecodedValue) => [v.signal_id, v]));
  const status = byId.get("monitor_status");
  if (!status || status.value.type !== "flags") return null;

  const flags: Record<string, boolean> = {};
  for (const f of status.value.value) flags[f.id] = f.set;

  const num = (id: string) => {
    const v = byId.get(id);
    return v && (v.value.type === "number" || v.value.type === "integer") ? v.value.value : null;
  };

  return {
    flags,
    milOn: flags.mil_on === true,
    dtcCount: num("dtc_count"),
    distanceSinceCleared: num("distance_since_cleared"),
    distanceUnit: byId.get("distance_since_cleared")?.unit ?? null,
    warmupsSinceCleared: num("warmups_since_cleared"),
  };
}

export function ReadinessCard({
  moduleKey,
  onEvidence,
}: {
  moduleKey: string | null;
  onEvidence: (ref: number) => void;
}) {
  const [result, setResult] = useState<ToolResult<ReadinessData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  const read = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      // Every module that keeps readiness, not just the selected one.
      //
      // Measured on a real truck: the engine and transmission controllers both
      // answer and disagree, and the card used to show whichever module happened
      // to be selected as though it were the vehicle's answer.
      setResult(await api.readiness());
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    setResult(null);
    void read();
  }, [read]);

  const modules = result?.success ? (result.data?.modules ?? []) : [];

  if (!moduleKey) return null;

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div>
          <strong>Emissions readiness</strong>
          <div className="faint">
            Whether the car has finished its own self-tests — and whether the codes were
            cleared recently.
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
          <span>
            This vehicle did not report its readiness monitors. That is unusual but not
            alarming on older or non-compliant vehicles.
          </span>
        </div>
      )}

      {result && <Warnings warnings={result.warnings} />}

      {/* One block per module that keeps readiness. Where they disagree the
          numbers are simply different, and that is shown rather than resolved:
          each module clears its own counters on its own schedule, so there is
          no single right answer to average toward. */}
      {modules.map((m) => {
        const reading = parseValues(m.values);
        if (!reading) return null;
        return (
          <div key={m.module} style={{ marginTop: 14 }}>
            {modules.length > 1 && (
              <div
                className="faint"
                style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 6 }}
              >
                {m.name ?? `Module at ${m.address}`}
              </div>
            )}
            <ReadinessBody reading={reading} values={m.values} onEvidence={onEvidence} />
          </div>
        );
      })}
    </div>
  );
}

function ReadinessBody({
  reading,
  values,
  onEvidence,
}: {
  reading: Reading;
  values: DecodedValue[];
  onEvidence: (ref: number) => void;
}) {
  const evidenceRef = values.find((v) => v.provenance.evidence_ref != null)?.provenance.evidence_ref ?? null;
  const diesel = reading.flags.compression_ignition === true;
  const all = [...CONTINUOUS, ...NON_CONTINUOUS];

  const rows = all
    .map((m) => ({
      name: diesel ? m.diesel : m.petrol,
      supported: reading.flags[m.supported] === true,
      incomplete: reading.flags[m.incomplete] === true,
    }))
    // A monitor the engine does not have is not a gap, so it is not listed.
    .filter((r) => r.name && r.supported);

  const notReady = rows.filter((r) => r.incomplete);
  const ready = rows.length - notReady.length;

  // The interpretation. Everything below is derived from measured bits, but the
  // conclusion drawn from them is the useful part.
  const km = reading.distanceSinceCleared;
  const recentlyCleared = km != null && km < 80;
  const suspicious = notReady.length > 0 && !reading.milOn && (reading.dtcCount ?? 0) === 0;

  return (
    <>
      <div className="row" style={{ marginTop: 12, gap: 20 }}>
        <div>
          <div className="faint" style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}>
            Warning lamp
          </div>
          <div className="value-big" style={{ fontSize: 16, color: reading.milOn ? "var(--serious)" : "var(--ok)" }}>
            {reading.milOn ? "ON" : "off"}
          </div>
        </div>
        <div>
          <div className="faint" style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}>
            Self-tests complete
          </div>
          <div
            className="value-big"
            style={{ fontSize: 16, color: notReady.length ? "var(--caution)" : "var(--ok)" }}
          >
            {ready} of {rows.length}
          </div>
        </div>
        {km != null && (
          <div>
            <div className="faint" style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}>
              Since codes cleared
            </div>
            <div className="value-big" style={{ fontSize: 16, color: recentlyCleared ? "var(--serious)" : undefined }}>
              {Math.round(km)}
              <span className="value-unit">{reading.distanceUnit ?? "km"}</span>
            </div>
          </div>
        )}
      </div>

      {suspicious && (
        <div className="banner serious" style={{ marginTop: 12, marginBottom: 0 }}>
          <span className="b-code">worth asking</span>
          <div>
            <strong>
              No stored codes, but {notReady.length} self-test
              {notReady.length === 1 ? " has" : "s have"} not finished
              {km != null ? `, and the codes were cleared ${Math.round(km)} ${reading.distanceUnit ?? "km"} ago` : ""}.
            </strong>
            <div style={{ marginTop: 6 }}>
              Clearing the trouble codes resets these self-tests, and they only finish again
              after the car has been driven properly — usually 50 to 100 miles. A car showing
              this pattern was very likely cleared shortly before you saw it. A recently
              disconnected or flat battery does the same thing, so it is not proof of anything
              — but it is worth asking the seller about, and worth re-checking after a good
              drive before you buy.
            </div>
          </div>
        </div>
      )}

      {!suspicious && notReady.length > 0 && (
        <div className="banner caution" style={{ marginTop: 12, marginBottom: 0 }}>
          <span className="b-code">not ready</span>
          <span>
            {notReady.length} self-test{notReady.length === 1 ? "" : "s"} not finished. In most
            regions this alone fails an emissions test, whatever else is wrong with the car.
          </span>
        </div>
      )}

      {!notReady.length && !reading.milOn && (
        <div className="banner info" style={{ marginTop: 12, marginBottom: 0 }}>
          <span className="b-code">ready</span>
          <span>
            Every self-test this engine supports has completed and passed. The car has been
            driven enough for its own diagnostics to have checked themselves — which also
            means the codes have not been cleared recently.
          </span>
        </div>
      )}

      <details style={{ marginTop: 10 }}>
        <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
          each self-test ({diesel ? "diesel" : "petrol"} engine)
        </summary>
        <table style={{ marginTop: 6 }}>
          <tbody>
            {rows.map((r) => (
              <tr key={r.name}>
                <td>{r.name}</td>
                <td style={{ width: 120 }}>
                  <span className={r.incomplete ? "tag pending" : "tag"} style={!r.incomplete ? { color: "var(--ok)", borderColor: "#1e4a26" } : undefined}>
                    {r.incomplete ? "not finished" : "complete"}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>

      {/* Evidence for this module's own reading, so a number and the exchange
          that produced it stay attached even when several modules answered. */}
      {evidenceRef != null && (
        <div className="provenance">
          <button className="ev" onClick={() => onEvidence(evidenceRef)}>
            show me where this came from
          </button>
        </div>
      )}
    </>
  );
}
