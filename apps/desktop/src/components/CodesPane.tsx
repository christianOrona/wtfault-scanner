// Trouble codes for the selected module.
//
// Two contract rules shape this pane:
//  * the same code can appear twice with different statuses (confirmed and
//    permanent are two distinct observations), so rows are keyed by
//    (code, status) and never collapsed by code alone.
//  * `description` is null when the code is not in the catalog. That is the
//    answer. Nothing is synthesised client-side to fill the gap.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { Dtc, DtcData, FreezeFrameData, ToolResult } from "../api/types";
import { VehicleMap } from "./VehicleMap";
import { ErrorBanner, FailedResult, Spinner, Value, Warnings } from "./primitives";
import { PaneIntro, useExplain } from "../explain";
import { saveFile, scanFilename, toCsv } from "./exportFile";
import { ClearCodesDialog } from "./ClearCodesDialog";

export function CodesPane({
  moduleKey,
  onEvidence,
}: {
  moduleKey: string | null;
  onEvidence: (ref: number) => void;
}) {
  const [result, setResult] = useState<ToolResult<DtcData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [frame, setFrame] = useState<ToolResult<FreezeFrameData> | null>(null);
  const [frameBusy, setFrameBusy] = useState(false);
  /** Where the last export landed, or why it did not. Shown, never silent. */
  const [savedTo, setSavedTo] = useState<string | null>(null);

  const read = useCallback(async () => {
    if (!moduleKey) return;
    setBusy(true);
    setError(null);
    setFrame(null);
    try {
      setResult(await api.dtcs(moduleKey));
    } catch (e) {
      setError(describeError(e));
      setResult(null);
    } finally {
      setBusy(false);
    }
  }, [moduleKey]);

  useEffect(() => {
    setResult(null);
    setFrame(null);
    void read();
  }, [read]);

  async function readFreezeFrame() {
    if (!moduleKey) return;
    setFrameBusy(true);
    try {
      setFrame(await api.freezeFrame(moduleKey, 0));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setFrameBusy(false);
    }
  }

  const { easy } = useExplain();
  const [clearing, setClearing] = useState(false);
  const [cleared, setCleared] = useState<ToolResult<unknown> | null>(null);
  if (!moduleKey) return <div className="empty">Select a module.</div>;

  const dtcs = result?.data?.dtcs ?? [];

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="dtc" />
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <div className="row">
          <strong>Trouble codes</strong>
          {!easy && <span className="faint mono">{moduleKey}</span>}
        </div>
        <div className="row">
          <button onClick={() => void readFreezeFrame()} disabled={frameBusy || !result?.success}>
            {frameBusy ? <Spinner label="Reading" /> : "Read freeze frame"}
          </button>
          <button onClick={() => void read()} disabled={busy}>
            {busy ? <Spinner label="Reading" /> : "Re-read"}
          </button>
          <button
            disabled={!dtcs.length}
            onClick={() =>
              void saveFile(
                scanFilename("codes", null, "csv"),
                toCsv(
                  ["code", "status", "module", "description", "source", "structural"],
                  dtcs.map((d) => [
                    d.code,
                    d.status,
                    d.module,
                    d.description ?? "",
                    d.verification ?? "",
                    d.structural_summary ?? "",
                  ]),
                ),
              ).then((r) => setSavedTo(r.path)).catch(() => setSavedTo("could not save"))
            }
          >
            Export
          </button>
          {/* Deliberately last and styled as destructive. It is the one thing
              on this screen that changes the vehicle. */}
          <button
            className="danger"
            onClick={() => setClearing(true)}
            disabled={busy || !result?.success}
            title="Erase stored codes, freeze frames and self-test results. Cannot be undone."
          >
            Clear codes
          </button>
        </div>
      </div>

      {cleared && (
        <div className={`banner ${cleared.success ? "info" : "serious"}`}>
          <span className="b-code">{cleared.success ? "cleared" : "not cleared"}</span>
          <div>
            {cleared.success ? (
              <>
                The vehicle has erased its stored codes. Its self-tests have reset and will take
                50 to 100 miles of driving to finish again — until then it will not pass an
                emissions test. Everything read before this is still in Sessions.
              </>
            ) : (
              <>{cleared.error?.message ?? "The vehicle refused."}</>
            )}
          </div>
        </div>
      )}

      {clearing && (
        <ClearCodesDialog
          moduleKey={moduleKey}
          codes={dtcs}
          onClose={() => setClearing(false)}
          onCleared={(r) => {
            setCleared(r);
            void read();
          }}
        />
      )}

      {savedTo && (
        <div className="banner info">
          <span className="b-code">saved</span>
          <span className="mono" style={{ fontSize: 12, wordBreak: "break-all" }}>{savedTo}</span>
        </div>
      )}
      <ErrorBanner error={error} />
      {result && !result.success && <FailedResult result={result} onEvidence={onEvidence} />}
      {result && <Warnings warnings={result.warnings} />}

      {result?.success && (
        <>
          <div className="section">
            <h2>
              {dtcs.length} code{dtcs.length === 1 ? "" : "s"}
              {result.data ? ` - ${result.data.confirmed_count} confirmed` : ""}
            </h2>
            {dtcs.length === 0 ? (
              <div className="card muted">
                {easy ? "This computer has no fault codes stored. Nothing has gone wrong that it thought was worth writing down." : "No stored, pending or permanent codes on this module."}
              </div>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>Code</th>
                    <th>Status</th>
                    <th>Description</th>
                    <th>Source</th>
                  </tr>
                </thead>
                <tbody>
                  {dtcs.map((d) => <DtcRow key={`${d.code}-${d.status}`} dtc={d} />)}
                </tbody>
              </table>
            )}
            {result.raw_evidence_ref != null && (
              <div className="provenance">
                <button className="ev" onClick={() => onEvidence(result.raw_evidence_ref!)}>
                  evidence #{result.raw_evidence_ref}
                </button>
                <span>read from {result.data?.modules_read.join(", ")}</span>
                <span>{result.execution_time_ms} ms</span>
              </div>
            )}
          </div>

          {frame && (
            <div className="section">
              <h2>{easy ? "Snapshot from when it broke" : `Freeze frame ${frame.data?.frame ?? 0}`}</h2>
              <PaneIntro kind="concept" id="freeze_frame" />
              {!frame.success ? (
                <FailedResult result={frame} onEvidence={onEvidence} />
              ) : (
                <>
                  <div className="card">
                    <div className="row">
                      <span className="tag">{frame.data?.dtc ?? "no code recorded"}</span>
                      <span className="muted">
                        {frame.data?.dtc_description ?? "no catalog description for this code"}
                      </span>
                    </div>
                    <div className="faint" style={{ marginTop: 6 }}>
                      A snapshot from when the fault was recorded. This is historical data - it is
                      deliberately kept out of the live series, because the difference is the whole
                      point of a freeze frame.
                    </div>
                  </div>
                  <Warnings warnings={frame.warnings} />
                  <div className="grid-2" style={{ marginTop: 10 }}>
                    {frame.values.map((v) => (
                      <Value key={v.signal_id} value={v} onEvidence={onEvidence} />
                    ))}
                  </div>
                  {!frame.values.length && <div className="card muted">The frame held no parameters.</div>}
                </>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function DtcRow({ dtc }: { dtc: Dtc }) {
  // "Where is it?" is the question that follows "what is wrong?", and the app
  // used to answer it with a part name and nothing else. Behind a toggle
  // because it is a zone rather than a location, and a diagram that opened
  // itself would be claiming more than it can support.
  const [showWhere, setShowWhere] = useState(false);
  const placeable = dtc.region !== "unknown";

  return (
    <>
      <tr>
        <td className="mono" style={{ fontWeight: 600 }}>{dtc.code}</td>
        <td><span className={`tag ${dtc.status}`}>{dtc.status}</span></td>
        <td>
          {dtc.description ?? (
            <span className="faint">
              Not in the catalog. It decoded structurally as: {dtc.structural_summary ?? "unknown"}
            </span>
          )}
          {/* Offered only where the standard actually places the subsystem.
              Most manufacturer-specific codes get no button, because nobody
              published what they are about. */}
          {placeable && (
            <button
              className="mini"
              style={{ marginLeft: 8 }}
              onClick={() => setShowWhere((s) => !s)}
            >
              {showWhere ? "Hide" : "Where is it?"}
            </button>
          )}
        </td>
        <td className="faint">
          {dtc.is_generic ? "SAE generic" : "manufacturer-specific"}
          {dtc.verification !== "verified" && <span className="tag unverified" style={{ marginLeft: 6 }}>unverified</span>}
        </td>
      </tr>
      {showWhere && placeable && (
        <tr>
          <td colSpan={4} style={{ paddingTop: 0 }}>
            <VehicleMap region={dtc.region} what={dtc.description ?? dtc.code} />
          </td>
        </tr>
      )}
    </>
  );
}
