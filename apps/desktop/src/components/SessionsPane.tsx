// Session history. Readable whether or not an adapter is connected - and only
// persisted at all when the server was started with --db, which is worth
// saying out loud rather than showing an empty list.

import { useCallback, useEffect, useState } from "react";
import { api, describeError, type SessionDetail } from "../api/client";
import type { Health, SessionSummary } from "../api/types";
import { ErrorBanner, Spinner, localTime } from "./primitives";
import { PaneIntro } from "../explain";

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

  useEffect(() => {
    if (!selected) { setDetail(null); return; }
    api.session(selected).then(setDetail).catch((e) => setError(describeError(e)));
  }, [selected]);

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

      {detail && (
        <div className="section" style={{ marginTop: 20 }}>
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

          <div className="card faint">
            {detail.event_count} recorded events. Tests, diagnoses and agent traces are empty in
            this build - the tables exist as the seam for later phases.
          </div>
        </div>
      )}
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
