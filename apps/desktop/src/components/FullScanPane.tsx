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
//  * An empty fault memory is not an engine with no codes. This scan asks each
//    module for its own list. The trouble codes the Codes tab reads are a
//    separate read and are not asked for here, so a module that may keep some
//    is never called clean on the strength of this scan: it is listed with
//    what was read and a way to the read that was not made.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { FullScanData, ScannedModule, ToolResult, UdsFault } from "../api/types";
import { ErrorBanner, FailedResult, Spinner, Warnings, localTime } from "./primitives";
import { PaneIntro, useExplain } from "../explain";
import { saveFile, scanFilename, toCsv, whereSaved } from "./exportFile";

export function FullScanPane({
  connected,
  active,
  onReadCodes,
}: {
  connected: boolean;
  active: boolean;
  /** Open the Codes tab on one module. */
  onReadCodes: (moduleKey: string) => void;
}) {
  const [result, setResult] = useState<ToolResult<FullScanData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  // Who ran the scan on screen. The assistant's inspection runs one too, and
  // it is the same scan: shown here rather than run a second time.
  const [ranBy, setRanBy] = useState<"assistant" | "person" | null>(null);
  const { easy } = useExplain();

  const run = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setResult(await api.scanAllModules());
      setRanBy("person");
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, []);

  // A scan is done once per sitting and seen everywhere. Each time this tab
  // is opened it asks the core for the session's last one, so a scan the
  // assistant ran during an inspection is here without pressing anything, and
  // one that was still running when the tab was left arrives when it ends.
  useEffect(() => {
    if (!active || !connected) return;
    let stale = false;
    api
      .lastFullScan()
      .then(({ scan }) => {
        if (stale || !scan) return;
        setResult((shown) =>
          // Never replace a newer scan with an older one.
          shown && shown.timestamp >= scan.result.timestamp ? shown : scan.result,
        );
        setRanBy((who) => who ?? scan.by);
      })
      .catch(() => {
        // Nothing connected, or the core went away: the pane says so itself.
      });
    return () => { stale = true; };
  }, [active, connected]);

  // A new vehicle is a new scan.
  useEffect(() => {
    if (!connected) { setResult(null); setRanBy(null); }
  }, [connected]);

  const data = result?.success ? result.data : null;
  const modules = data?.modules ?? [];
  // A module whose list was cut off is shown with the faulty ones even when
  // none of what arrived was a fault: "nothing to report" would be a claim
  // about the part that never came.
  const withFaults = modules.filter((m) => m.fault_count > 0 || m.cut_off);
  const cutOff = modules.filter((m) => m.cut_off);
  // A module with a note never handed over its fault list. Its zero is
  // unknown, not clean, and "all clear" may only speak for the rest.
  const unread = modules.filter((m) => m.note);
  const read = modules.length - unread.length;
  // Modules that may keep trouble codes this scan did not ask for. Seen on the
  // simulated F-250 (2026-10-05): an engine module holding three codes was
  // listed as "answered, no faults stored", because its own fault memory was
  // empty and that was all it had been asked for.
  const codesElsewhere = modules.filter(keepsCodesElsewhere);
  const quiet = modules.filter((m) => m.fault_count === 0 && !m.cut_off);
  const quietWithCodesElsewhere = quiet.filter(keepsCodesElsewhere).length;
  const failingNow = modules.flatMap((m) => m.faults.filter((f) => f.failing_now));
  // Faults whose module's protocol does not say whether they are failing now.
  // "Nothing is failing" may only be said when every fault was asked.
  const notReported = modules.flatMap((m) => m.faults.filter((f) => f.failing_now === null));

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="full_scan" />

      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <div>
          <strong>{easy ? "Check every computer" : "Full vehicle scan"}</strong>
          <div className="faint">
            Asks every control module on the vehicle for its faults, not just the two the
            emissions rules cover. The app does this itself: no AI model is involved and
            nothing is spent.
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
                  .then((r) => setSavedTo(whereSaved(r)))
                  .catch((e) => setSavedTo(`could not save: ${describeError(e).message}`))
              }
            >
              Export
            </button>
          )}
          <button className="primary" onClick={() => void run()} disabled={busy || !connected}>
            {busy ? <Spinner label="Scanning every module…" /> : result ? "Scan again" : "Scan the whole vehicle"}
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

      {result?.success && ranBy && (
        <div className="faint" style={{ marginBottom: 10 }}>
          {ranBy === "assistant" ? "This is the scan the AI inspection ran" : "Scanned"}{" "}
          at {localTime(result.timestamp)}. It is one scan, used everywhere: for the next half
          hour the AI inspection reads this one instead of running its own.
        </div>
      )}
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
            {/* With no fault list read, a zero is not known to be zero. */}
            <Stat
              label="Faults stored"
              value={read === 0 ? "unknown" : String(data.fault_count)}
              tone={
                data.fault_count
                  ? "var(--caution)"
                  : read === 0 || codesElsewhere.length
                    ? undefined
                    : "var(--ok)"
              }
            />
            <Stat
              label="Failing right now"
              value={
                read === 0 || (notReported.length > 0 && failingNow.length === 0)
                  ? "unknown"
                  : String(failingNow.length)
              }
              tone={
                failingNow.length
                  ? "var(--serious)"
                  : read === 0 || notReported.length || codesElsewhere.length
                    ? undefined
                    : "var(--ok)"
              }
            />
            {!easy && <Stat label="Addresses probed" value={String(data.addresses_probed)} />}
          </div>

          {data.fault_count === 0 && modules.length > 0 && read === 0 ? (
            <div className="banner caution">
              <span className="b-code">nothing read</span>
              <span>
                {modules.length} modules answered, but none of them handed over its list of faults,
                so whether any faults are stored is not known. This is not a clean result.
              </span>
            </div>
          ) : data.fault_count === 0 && cutOff.length > 0 ? (
            <div className="banner caution">
              <span className="b-code">incomplete</span>
              <span>
                None of the faults that arrived is active or stored, but{" "}
                {cutOff.length === 1 ? "one module" : `${cutOff.length} modules`} stopped partway
                through {cutOff.length === 1 ? "its" : "their"} fault list, so this is not a clean
                bill of health. Scanning again may get the rest.
              </span>
            </div>
          ) : data.fault_count === 0 && codesElsewhere.length > 0 ? (
            <div className="banner info">
              <span className="b-code">no faults here</span>
              <span>
                {read} module{read === 1 ? "" : "s"} handed over {read === 1 ? "its" : "their"}{" "}
                fault list and none of them is holding a fault. Trouble codes are not part of
                that. They are a separate read, and this scan did not make it for{" "}
                {named(codesElsewhere)}. {codesElsewhere.length === 1 ? "It is" : "Each is"} in
                the list below, with a button that does.
                {unread.length > 0 &&
                  ` ${unread.length} more answered but would not hand one over, so nothing is known about them.`}
              </span>
            </div>
          ) : data.fault_count === 0 ? (
            <div className="banner info">
              <span className="b-code">all clear</span>
              <span>
                {read} module{read === 1 ? "" : "s"} handed over {read === 1 ? "its" : "their"}{" "}
                fault list and none of them is holding a fault. That is a stronger result than a
                clean code scan, because it covers systems the emissions rules never look at.
                {unread.length > 0 &&
                  ` ${unread.length} more answered but would not hand one over, so nothing is known about them.`}
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
                ) : notReported.length > 0 ? (
                  <>
                    <strong>
                      {data.fault_count} fault{data.fault_count === 1 ? " is" : "s are"} stored.
                    </strong>
                    <div style={{ marginTop: 6 }}>
                      Whether any is failing right now is not something this vehicle reports, so
                      it is not known. A fault that is still present usually keeps the warning
                      lamp on.
                    </div>
                  </>
                ) : (
                  <>
                    <strong>
                      {/* "Nothing" would speak for codes this scan did not read. */}
                      {codesElsewhere.length > 0
                        ? "None of these faults is failing at this moment."
                        : "Nothing is failing at this moment."}
                    </strong>
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
            <ModuleCard key={m.address} module={m} easy={easy} onReadCodes={onReadCodes} />
          ))}

          {/* Open when it is where the banner above sends the reader. */}
          <details
            style={{ marginTop: 12 }}
            open={data.fault_count === 0 && quietWithCodesElsewhere > 0}
          >
            <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
              {quietWithCodesElsewhere > 0
                ? `modules with no faults in this scan${unread.length > 0 ? " or no fault list" : ""} (${quiet.length}) · trouble codes of ${quietWithCodesElsewhere} are read separately`
                : unread.length > 0
                  ? `modules with no faults or no fault list (${quiet.length})`
                  : `modules with nothing to report (${quiet.length})`}
            </summary>
            <table style={{ marginTop: 8 }}>
              <tbody>
                {quiet.map((m) => (
                  <tr key={m.address}>
                    <td>{m.name}</td>
                    <td className="faint">
                      {/* The distinctions that a bare zero would lose. */}
                      {m.note ??
                        (keepsCodesElsewhere(m)
                          ? "nothing in its own fault list"
                          : "answered, no faults stored")}
                      {keepsCodesElsewhere(m) && (
                        <>
                          {". "}
                          <CodesElsewhere module={m} onReadCodes={onReadCodes} />
                        </>
                      )}
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

/** Whether a module may keep trouble codes this scan did not ask for. */
function keepsCodesElsewhere(m: ScannedModule): boolean {
  return m.emissions_codes_read === false;
}

/** "A", "A and B", "A, B and C", or a count when that would be a paragraph. */
function named(modules: ScannedModule[]): string {
  const names = modules.map((m) => m.name);
  if (names.length > 3) return `${names.length} of these modules`;
  if (names.length === 1) return names[0];
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/**
 * What this scan did not read from a module, and the way to read it.
 *
 * The button changes which module is selected and opens the Codes tab, which
 * reads on arrival. It asks the vehicle nothing from here.
 */
function CodesElsewhere({
  module: m,
  onReadCodes,
}: {
  module: ScannedModule;
  onReadCodes: (moduleKey: string) => void;
}) {
  const key = m.module_key;
  return (
    <>
      Its trouble codes are a separate read, which this scan did not make.
      {key && (
        <button className="mini" style={{ marginLeft: 8 }} onClick={() => onReadCodes(key)}>
          Read its codes
        </button>
      )}
    </>
  );
}

function ModuleCard({
  module: m,
  easy,
  onReadCodes,
}: {
  module: ScannedModule;
  easy: boolean;
  onReadCodes: (moduleKey: string) => void;
}) {
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

      {m.cut_off && (
        <div className="banner caution" style={{ marginTop: 8 }}>
          <span className="b-code">list cut off</span>
          <span>{m.cut_off}</span>
        </div>
      )}

      {m.faults.length > 0 && (
        <table style={{ marginTop: 8 }}>
          <tbody>
            {m.faults.map((f) => (
              <FaultRow key={f.code} fault={f} />
            ))}
          </tbody>
        </table>
      )}

      {keepsCodesElsewhere(m) && (
        <div className="faint" style={{ marginTop: 8, fontSize: 12 }}>
          <CodesElsewhere module={m} onReadCodes={onReadCodes} />
        </div>
      )}
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
