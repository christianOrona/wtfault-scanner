// Every module on the vehicle, and every fault it is holding.
//
// The Codes tab reads what the law requires a car to tell you: emissions
// faults, from the engine and transmission controllers. That is a small corner
// of a modern vehicle. Brakes, airbag, body, steering and the rest are on the
// same wires, answering the same standard, and nothing was asking them — which
// is why a car with a dead wheel speed sensor and a healthy engine scans clean
// on a code reader.
//
// This sweeps the whole diagnostic address range and asks everything that
// answers for its fault memory. It takes longer than reading codes, so it is a
// deliberate action rather than something that happens on connect.
//
// Two presentation rules that matter more here than elsewhere:
//
//  * "Failing now" and "stored" are separated. A confirmed code that is not
//    currently failing is the commonest and most confusing result — something
//    went wrong once and is not wrong now — and collapsing the two into a
//    single red list makes an old fault look like an emergency.
//  * A module with no fault service is not a module with no faults. Both show
//    zero, and they mean entirely different things.

import { useCallback, useState } from "react";
import { api, describeError } from "../api/client";
import type { FullScanData, ScannedModule, ToolResult, UdsFault } from "../api/types";
import { ErrorBanner, FailedResult, Spinner, Warnings } from "./primitives";
import { PaneIntro, useExplain } from "../explain";
import { saveFile, scanFilename, toCsv } from "./exportFile";

export function FullScanPane({ connected }: { connected: boolean }) {
  const [result, setResult] = useState<ToolResult<FullScanData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const { easy } = useExplain();

  const run = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setResult(await api.scanAllModules());
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const data = result?.success ? result.data : null;
  const modules = data?.modules ?? [];
  const withFaults = modules.filter((m) => m.fault_count > 0);
  const failingNow = modules.flatMap((m) => m.faults.filter((f) => f.failing_now));

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="full_scan" />

      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <div>
          <strong>{easy ? "Check every computer" : "Full vehicle scan"}</strong>
          <div className="faint">
            Asks every control module on the vehicle for its faults, not just the two the
            emissions rules cover.
          </div>
        </div>
        <div className="row">
          {data && (
            <button
              onClick={() =>
                void saveFile(
                  scanFilename("full-scan", null, "csv"),
                  toCsv(
                    ["module", "address", "code", "description", "status", "failing_now"],
                    modules.flatMap((m) =>
                      m.faults.map((f) => [
                        m.name,
                        m.address,
                        f.code,
                        f.description ?? "",
                        f.status_summary,
                        f.failing_now,
                      ]),
                    ),
                  ),
                )
                  .then((r) => setSavedTo(r.path))
                  .catch((e) => setSavedTo(`could not save: ${describeError(e).message}`))
              }
            >
              Export
            </button>
          )}
          <button className="primary" onClick={() => void run()} disabled={busy || !connected}>
            {busy ? <Spinner label="Scanning every module…" /> : "Scan the whole vehicle"}
          </button>
        </div>
      </div>

      {!connected && (
        <div className="banner caution">
          <span className="b-code">not connected</span>
          <span>Connect to a vehicle first.</span>
        </div>
      )}

      <ErrorBanner error={error} />
      {savedTo && (
        <div className="banner info">
          <span className="b-code">saved</span>
          <span className="mono" style={{ fontSize: 12, wordBreak: "break-all" }}>{savedTo}</span>
        </div>
      )}
      {result && !result.success && <FailedResult result={result} />}
      {result && <Warnings warnings={result.warnings} />}

      {busy && (
        <div className="banner info">
          <span className="b-code">working</span>
          <span>
            Knocking on every diagnostic address in turn and asking whoever answers what is
            wrong. This takes longer than reading codes because most addresses have nothing
            behind them.
          </span>
        </div>
      )}

      {data && (
        <>
          <div className="row" style={{ gap: 24, marginBottom: 14 }}>
            <Stat label="Modules found" value={String(data.module_count)} />
            <Stat
              label="Faults stored"
              value={String(data.fault_count)}
              tone={data.fault_count ? "var(--caution)" : "var(--ok)"}
            />
            <Stat
              label="Failing right now"
              value={String(failingNow.length)}
              tone={failingNow.length ? "var(--serious)" : "var(--ok)"}
            />
            {!easy && <Stat label="Addresses probed" value={String(data.addresses_probed)} />}
          </div>

          {data.fault_count === 0 ? (
            <div className="banner info">
              <span className="b-code">all clear</span>
              <span>
                {data.module_count} modules answered and none of them is holding a fault. That
                is a stronger result than a clean code scan, because it covers systems the
                emissions rules never look at.
              </span>
            </div>
          ) : (
            <div className={`banner ${failingNow.length ? "serious" : "caution"}`}>
              <span className="b-code">{failingNow.length ? "active faults" : "stored faults"}</span>
              <div>
                {failingNow.length > 0 ? (
                  <>
                    <strong>
                      {failingNow.length} fault{failingNow.length === 1 ? " is" : "s are"} happening
                      right now.
                    </strong>
                    <div style={{ marginTop: 6 }}>
                      These are not history — the module reports them as failing at this moment.
                    </div>
                  </>
                ) : (
                  <>
                    <strong>Nothing is failing at this moment.</strong>
                    <div style={{ marginTop: 6 }}>
                      Every fault found is stored from an earlier drive. Worth understanding, not
                      worth panicking about.
                    </div>
                  </>
                )}
              </div>
            </div>
          )}

          {withFaults.map((m) => (
            <ModuleCard key={m.address} module={m} easy={easy} />
          ))}

          <details style={{ marginTop: 12 }}>
            <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
              modules with nothing to report ({modules.length - withFaults.length})
            </summary>
            <table style={{ marginTop: 8 }}>
              <tbody>
                {modules
                  .filter((m) => m.fault_count === 0)
                  .map((m) => (
                    <tr key={m.address}>
                      <td>{m.name}</td>
                      <td className="faint">
                        {/* The distinction that a bare zero would lose. */}
                        {m.note ?? "answered, no faults stored"}
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </details>
        </>
      )}

      {!data && !busy && connected && (
        <div className="empty">
          Nothing scanned yet. This is the check that finds faults a code reader cannot.
        </div>
      )}
    </div>
  );
}

function ModuleCard({ module: m, easy }: { module: ScannedModule; easy: boolean }) {
  return (
    <div className="card">
      <div className="row" style={{ gap: 8 }}>
        <strong>{m.name}</strong>
        {!m.in_legislated_range && (
          <span
            className="tag"
            title="Outside the emissions block a code reader can reach. This module is only visible to a full scan."
          >
            {easy ? "beyond a code reader" : "non-legislated"}
          </span>
        )}
        {!easy && <span className="faint mono" style={{ fontSize: 11 }}>{m.address}</span>}
      </div>

      <table style={{ marginTop: 8 }}>
        <tbody>
          {m.faults.map((f) => (
            <FaultRow key={f.code} fault={f} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function FaultRow({ fault: f }: { fault: UdsFault }) {
  return (
    <tr>
      <td style={{ width: 110 }}>
        <span className="mono" style={{ fontWeight: 600 }}>{f.code}</span>
      </td>
      <td>
        <div>
          {/* Never invented. A module outside the emissions system is exactly
              where this build is most likely to have no description, and the
              structural decoding is what it can honestly say instead. */}
          {f.description ?? f.structural_summary ?? "no description for this code in this build"}
        </div>
        <div
          className="faint"
          style={{ fontSize: 12, color: f.failing_now ? "var(--serious)" : undefined }}
        >
          {f.status_summary}
          {f.warning_lamp ? " · the module is asking for a warning light" : ""}
        </div>
      </td>
    </tr>
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
      <div className="value-big" style={{ fontSize: 18, color: tone }}>{value}</div>
    </div>
  );
}
