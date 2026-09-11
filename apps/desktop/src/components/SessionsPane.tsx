// Session history. Readable whether or not an adapter is connected - and only
// persisted at all when the server was started with --db, which is worth
// saying out loud rather than showing an empty list.

import { useCallback, useEffect, useState } from "react";
import { api, describeError, type SessionDetail } from "../api/client";
import type { Health, Measurement, SessionSummary } from "../api/types";
import { ErrorBanner, Spinner, localTime } from "./primitives";
import { saveFile, scanFilename, toCsv } from "./exportFile";
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

  useEffect(() => {
    if (!selected) { setDetail(null); setMeasurements(null); setSavedTo(null); return; }
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
              onClick={() =>
                void saveFile(
                  scanFilename("session", detail.vehicle?.vin ?? null, "csv"),
                  sessionCsv(detail, measurements ?? []),
                )
                  .then((r) => setSavedTo(r.path))
                  .catch(() => setSavedTo("could not save"))
              }
            >
              Export everything
            </button>
          </div>
        </div>
        {savedTo && (
          <div className="faint" style={{ marginBottom: 10 }}>
            {savedTo === "could not save" ? savedTo : `Saved to ${savedTo}`}
          </div>
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
        <button onClick={() => void load()} disabled={busy}>{busy ? <Spinner /> : "Refresh"}</button>
      </div>

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
