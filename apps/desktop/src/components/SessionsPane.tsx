// Session history. Readable whether or not an adapter is connected - and only
// persisted at all when the server was started with --db, which is worth
// saying out loud rather than showing an empty list.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, describeError, type ImportSummary, type SessionDetail } from "../api/client";
import type { Health, Measurement, SessionSummary } from "../api/types";
import { ErrorBanner, Spinner, localTime } from "./primitives";
import { saveFile, savedNote, scanFilename, toCsv } from "./exportFile";
import { PaneIntro } from "../explain";
import { CompareSessions } from "./CompareSessions";

export function SessionsPane({ health }: { health: Health | null }) {
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.sessions(50);
      setSessions(res.sessions);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const [measurements, setMeasurements] = useState<Measurement[] | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);

  // Bringing in what another device recorded. Two steps on purpose: the file
  // is read and the person is told exactly what it would add before any of it
  // is kept, because a merge into the history cannot be taken back out.
  const filePicker = useRef<HTMLInputElement>(null);
  const [incoming, setIncoming] = useState<{ file: File; preview: ImportSummary } | null>(null);
  const [importing, setImporting] = useState(false);

  const previewImport = useCallback(async (file: File) => {
    setImporting(true);
    setSavedTo(null);
    setIncoming(null);
    try {
      setIncoming({ file, preview: await api.importDatabase(file, true) });
    } catch (e) {
      setSavedTo(`Could not read ${file.name}: ${describeError(e).message}`);
    } finally {
      setImporting(false);
    }
  }, []);

  const confirmImport = useCallback(async () => {
    if (!incoming) return;
    setImporting(true);
    try {
      const done = await api.importDatabase(incoming.file, false);
      setSavedTo(`Imported ${incoming.file.name}: ${importLines(done).join("; ")}.`);
      setIncoming(null);
      await load();
    } catch (e) {
      setSavedTo(`Nothing was imported: ${describeError(e).message}`);
    } finally {
      setImporting(false);
    }
  }, [incoming, load]);

  // The whole database, not one session: it is the one file another install
  // opens as-is, and what was learned about the vehicle travels with it.
  const exportDatabase = useCallback(async () => {
    setCopying(true);
    setSavedTo(null);
    try {
      const r = await api.exportDatabase(scanFilename("database", null, "sqlite"));
      setSavedTo(`${savedNote(r)} (${megabytes(r.bytes)})`);
    } catch (e) {
      setSavedTo(`Could not export the database: ${describeError(e).message}`);
    } finally {
      setCopying(false);
    }
  }, []);

  useEffect(() => {
    setSavedTo(null);
    if (!selected) { setDetail(null); setMeasurements(null); return; }
    api.session(selected).then(setDetail).catch((e) => setError(describeError(e)));
    // Readings were never shown at all - the old view reported a count of
    // events and left it there, which is most of why it read as a summary.
    api.measurements(selected).then((r) => setMeasurements(r.measurements)).catch(() => {});
  }, [selected]);

  // Opening a session takes over the pane rather than appending below the
  // list. The old arrangement put everything under the fold, and the only
  // signal that it was there was the scrollbar changing size - which is not a
  // signal, it is an accident somebody has to notice.
  if (selected && detail) {
    return (
      <div className="pane">
        <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
          <div className="row" style={{ gap: 10 }}>
            <button onClick={() => setSelected(null)}>← All sessions</button>
            <strong>
              {detail.vehicle?.vin ? `VIN ${detail.vehicle.vin}` : "Session"}
            </strong>
            <span className="faint">{localTime(detail.session.started_at)}</span>
          </div>
          <div className="row" style={{ gap: 8 }}>
            <button
              title="Every command sent to the adapter and every reply, with the VIN replaced. Replays without the vehicle."
              onClick={() =>
                // No VIN in the name: the point of this file is that it has none.
                void api
                  .exportTranscript(detail.session.id, scanFilename("replay", null, "transcript"))
                  .then((r) => setSavedTo(savedNote(r)))
                  .catch((e) => setSavedTo(`Could not export the transcript: ${describeError(e).message}`))
              }
            >
              Export transcript
            </button>
            <button
              onClick={() =>
                void saveFile(
                  scanFilename("session", detail.vehicle?.vin ?? null, "csv"),
                  sessionCsv(detail, measurements ?? []),
                )
                  .then((r) => setSavedTo(savedNote(r)))
                  .catch(() => setSavedTo("could not save"))
              }
            >
              Export everything
            </button>
          </div>
        </div>
        {savedTo && (
          <div className="faint" style={{ marginBottom: 10 }}>{savedTo}</div>
        )}
        <ErrorBanner error={error} />
        <SessionDetailView detail={detail} measurements={measurements} />
      </div>
    );
  }

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="session" />
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <strong>Sessions</strong>
        <div className="row" style={{ gap: 8 }}>
          <input
            ref={filePicker}
            type="file"
            accept=".sqlite,application/octet-stream"
            style={{ display: "none" }}
            onChange={(e) => {
              const file = e.target.files?.[0];
              // Cleared so that choosing the same file again still fires.
              e.target.value = "";
              if (file) void previewImport(file);
            }}
          />
          <button
            title="Bring in a database exported from another device. You are shown what it would add first."
            onClick={() => filePicker.current?.click()}
            disabled={importing}
          >
            {importing && !incoming ? <Spinner /> : "Import database"}
          </button>
          <button
            title="One file holding every session here, with timestamps, readings and the raw exchanges. Another install can open it."
            onClick={() => void exportDatabase()}
            disabled={copying || !sessions?.length}
          >
            {copying ? <Spinner /> : "Export database"}
          </button>
          <button onClick={() => void load()} disabled={busy}>{busy ? <Spinner /> : "Refresh"}</button>
        </div>
      </div>
      {savedTo && (
        <div className="faint" style={{ marginBottom: 10 }}>
          {savedTo}
        </div>
      )}
      {incoming && (
        <div className="card">
          <strong>{incoming.file.name}</strong>
          {isNothingNew(incoming.preview) ? (
            <div style={{ marginTop: 6 }}>Nothing in this file is new here.</div>
          ) : (
            <>
              <div style={{ marginTop: 6 }}>Importing it would add:</div>
              <ul style={{ margin: "6px 0 0 18px" }}>
                {importLines(incoming.preview).map((line) => <li key={line}>{line}</li>)}
              </ul>
            </>
          )}
          {incoming.preview.sessions_skipped.map((s) => (
            <div key={s.id} className="faint" style={{ marginTop: 6 }}>
              Left out, <span className="mono">{s.id}</span>: {s.reason}.
            </div>
          ))}
          <div className="faint" style={{ fontSize: 12, marginTop: 8 }}>
            Nothing already here is replaced by anything older, and importing the same file twice
            adds nothing the second time. An import cannot be undone.
          </div>
          <div className="row" style={{ gap: 8, marginTop: 10 }}>
            {!isNothingNew(incoming.preview) && (
              <button className="primary" onClick={() => void confirmImport()} disabled={importing}>
                {importing ? <Spinner /> : "Add to this device"}
              </button>
            )}
            <button onClick={() => setIncoming(null)} disabled={importing}>
              {isNothingNew(incoming.preview) ? "Close" : "Cancel"}
            </button>
          </div>
        </div>
      )}
      {!!sessions?.length && (
        <div className="faint" style={{ fontSize: 12, marginBottom: 10 }}>
          The database export is everything recorded on this device, VINs included. Keep it to
          yourself; a session's transcript is the one made to pass on.
        </div>
      )}

      <ErrorBanner error={error} />

      {health && !health.database && (
        <div className="banner info">
          <span className="b-code">in-memory</span>
          <span>
            This server was started without <code>--db</code>, so nothing is written to disk and
            history disappears when it stops. Restart with <code>--db ./sessions.sqlite</code> to keep it.
          </span>
        </div>
      )}

      {sessions && !sessions.length && <div className="empty">No sessions recorded yet.</div>}

      {!!sessions?.length && (
        <table>
          <thead>
            <tr>
              <th>Started</th><th>Label</th><th>VIN</th>
              <th>Modules</th><th>Codes</th><th>Readings</th><th>Events</th>
            </tr>
          </thead>
          <tbody>
            {sessions.map((s) => (
              <tr
                key={s.session.id}
                onClick={() => setSelected(s.session.id === selected ? null : s.session.id)}
                style={{ cursor: "pointer" }}
              >
                <td>
                  {localTime(s.session.started_at)}
                  {!s.session.ended_at && <span className="tag" style={{ marginLeft: 6 }}>open</span>}
                </td>
                <td>{s.session.label ?? <span className="faint">none</span>}</td>
                <td className="mono">{s.vehicle?.vin ?? <span className="faint">not identified</span>}</td>
                <td className="num">{s.module_count}</td>
                <td className="num">{s.dtc_count}</td>
                <td className="num">{s.measurement_count}</td>
                <td className="num">{s.event_count}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {sessions && sessions.length > 1 && <CompareSessions sessions={sessions} />}

    </div>
  );
}

/** `3 sessions`, `1 session`. */
function some(n: number, one: string): string {
  return `${n.toLocaleString()} ${one}${n === 1 ? "" : "s"}`;
}

/** True when a file holds nothing this device lacks. The same test the core
 *  applies; sessions already here and findings kept are not news. */
function isNothingNew(s: ImportSummary): boolean {
  return (
    !s.sessions_added && !s.sessions_extended && !s.vehicles_added && !s.captures_added &&
    !s.findings_added && !s.findings_updated && !s.vehicle_records_taken
  );
}

/** What an import adds, as a list a person can agree to. */
function importLines(s: ImportSummary): string[] {
  const out: string[] = [];
  const vins = s.vehicles.map((v) => v ?? "a vehicle that gave no VIN").join(", ");
  if (s.sessions_added) out.push(`${some(s.sessions_added, "session")} this device does not have`);
  if (s.sessions_extended) {
    out.push(`the rest of ${some(s.sessions_extended, "session")} it has the start of`);
  }
  if (s.sessions_added || s.sessions_extended) {
    out.push(
      `with ${some(s.events_added, "recorded exchange")}, ${some(s.measurements_added, "reading")}` +
        ` and ${some(s.dtcs_added, "fault code record")}, for ${vins}`,
    );
  }
  if (s.vehicles_added) out.push(`${some(s.vehicles_added, "vehicle")} it has never seen`);
  if (s.findings_added) out.push(`${some(s.findings_added, "finding")} about a vehicle`);
  if (s.findings_updated) {
    out.push(`${some(s.findings_updated, "finding")} newer than the one here, which it replaces`);
  }
  if (s.captures_added) out.push(some(s.captures_added, "configuration capture"));
  if (s.vehicle_records_taken) {
    out.push(`${some(s.vehicle_records_taken, "vehicle record")} (as-built file or VIN lookup)`);
  }
  if (s.sessions_already_here) {
    out.push(`${some(s.sessions_already_here, "session")} already here, left as they are`);
  }
  if (s.findings_kept) {
    out.push(`${some(s.findings_kept, "finding")} here that are as new as the file's, kept`);
  }
  return out.length ? out : ["nothing new"];
}

/** A file size a person can weigh against their data plan. */
function megabytes(bytes: number): string {
  return bytes < 100_000 ? `${Math.max(1, Math.round(bytes / 1000))} kB` : `${(bytes / 1_000_000).toFixed(1)} MB`;
}

function Field({ k, v }: { k: string; v: string | null }) {
  return (
    <div>
      <div className="faint" style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}>{k}</div>
      <div className="mono">{v ?? <span className="faint">not reported</span>}</div>
    </div>
  );
}

/** Everything recorded for one session.
 *
 * Split out of the list because it now gets the whole pane. It previously
 * lived at the bottom of the sessions table and reported "N recorded events"
 * as its last word, which is a count standing in for the content.
 */
function SessionDetailView({
  detail,
  measurements,
}: {
  detail: SessionDetail;
  measurements: Measurement[] | null;
}) {
  return (
    <div className="section">
  <h2>Session {detail.session.id}</h2>

  {detail.vehicle && (
    <div className="card">
      <div className="row" style={{ gap: 24 }}>
        <Field k="VIN" v={detail.vehicle.vin} />
        <Field k="Make" v={detail.vehicle.make} />
        <Field k="Year" v={detail.vehicle.year != null ? String(detail.vehicle.year) : null} />
        <Field k="Model" v={detail.vehicle.model} />
      </div>
      <div className="faint" style={{ marginTop: 8 }}>
        Only what the VIN standard encodes is filled in. Model, trim and engine stay empty
        because this build has no licensed VIN database and will not guess.
      </div>
    </div>
  )}

  {!!detail.connections.length && (
    <div className="card">
      <div className="faint" style={{ marginBottom: 6 }}>Connections</div>
      <table>
        <thead><tr><th>Adapter</th><th>Transport</th><th>Firmware</th><th>Connected</th><th>Closed</th></tr></thead>
        <tbody>
          {detail.connections.map((c) => (
            <tr key={c.id}>
              <td className="mono">{c.adapter_id}</td>
              <td>{c.transport}</td>
              <td className="mono">{c.firmware ?? <span className="faint">-</span>}</td>
              <td>{localTime(c.connected_at)}</td>
              <td>{c.disconnected_at ? localTime(c.disconnected_at) : <span className="faint">open</span>}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )}

  {!!detail.dtcs.length && (
    <div className="card">
      <div className="faint" style={{ marginBottom: 6 }}>Codes recorded</div>
      <table>
        <thead><tr><th>Code</th><th>Status</th><th>Seen</th><th>Description</th><th>Read at</th></tr></thead>
        <tbody>
          {detail.dtcs.map((d, i) => (
            <tr key={`${d.code}-${d.status}-${i}`}>
              <td className="mono" style={{ fontWeight: 600 }}>{d.code}</td>
              <td><span className={`tag ${d.status}`}>{d.status}</span></td>
              <td className="num">{d.occurrence}x</td>
              <td className="faint">{d.description ?? "not in the catalog"}</td>
              <td className="faint">{localTime(d.read_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )}

  {!!detail.modules.length && (
    <div className="card">
      <div className="faint" style={{ marginBottom: 6 }}>Modules</div>
      <table>
        <thead><tr><th>Key</th><th>Address</th><th>Name</th><th>Protocol</th></tr></thead>
        <tbody>
          {detail.modules.map((m) => (
            <tr key={m.id}>
              <td className="mono">{m.module_key}</td>
              <td className="mono">{m.address}</td>
              <td>{m.name ?? <span className="faint">did not report a name</span>}</td>
              <td className="faint mono">{m.protocol}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )}


      {/* Readings were never shown here at all. The old view reported a count
          of events and stopped, which is most of why this read as a summary of
          a session rather than the session. */}
      {!!measurements?.length && (
        <div className="card">
          <div className="faint" style={{ marginBottom: 6 }}>
            Readings recorded ({measurements.length})
          </div>
          <table>
            <thead>
              <tr><th>Signal</th><th>Value</th><th>Unit</th><th>Raw</th><th>When</th></tr>
            </thead>
            <tbody>
              {measurements.map((m, i) => (
                <tr key={`${m.signal_id}-${i}`}>
                  <td className="mono">{m.signal_id}</td>
                  <td className="num">{m.value ?? m.text_value ?? <span className="faint">-</span>}</td>
                  <td className="faint">{m.unit ?? ""}</td>
                  {/* The bytes the value was decoded from. This is what makes a
                      reading checkable rather than merely readable. */}
                  <td className="mono faint">{m.raw_value}</td>
                  <td className="faint">{localTime(m.timestamp)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="card faint">
        {detail.event_count} recorded events. The full exchange behind every number above is in
        the flight recorder; tests, diagnoses and agent traces are empty in this build.
      </div>
    </div>
  );
}

/** One file holding everything recorded for a session.
 *
 * A single CSV with a `section` column rather than several files: somebody
 * sending this to a mechanic should attach one thing, and a spreadsheet can
 * filter a column. */
function sessionCsv(detail: SessionDetail, measurements: Measurement[]): string {
  const rows: unknown[][] = [];
  rows.push(["session", "id", detail.session.id, "", ""]);
  rows.push(["session", "started", detail.session.started_at, "", ""]);
  if (detail.vehicle) {
    rows.push(["vehicle", "vin", detail.vehicle.vin ?? "", "", ""]);
    rows.push(["vehicle", "make", detail.vehicle.make ?? "", "", ""]);
    rows.push(["vehicle", "year", detail.vehicle.year ?? "", "", ""]);
  }
  for (const c of detail.connections) {
    rows.push(["connection", c.adapter_id, c.transport, c.firmware ?? "", c.connected_at]);
  }
  for (const m of detail.modules) {
    rows.push(["module", m.module_key, m.address, m.name ?? "", m.protocol]);
  }
  for (const d of detail.dtcs) {
    rows.push(["code", d.code, d.status, d.description ?? "", d.read_at]);
  }
  for (const m of measurements) {
    rows.push(["reading", m.signal_id, m.value ?? m.text_value ?? "", m.unit ?? "", m.raw_value]);
  }
  return toCsv(["section", "a", "b", "c", "d"], rows);
}
