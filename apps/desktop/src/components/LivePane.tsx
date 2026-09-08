// Live data: pick signals from what the module actually reports as supported,
// then stream them over the websocket.
//
// The picker is built from `GET /modules/{key}/signals`, which distinguishes
// "the vehicle supports this PID" from "this build can decode it". A PID with
// decoder_available: false is shown and disabled, so the gap is visible rather
// than hidden.

import { useCallback, useEffect, useMemo, useState } from "react";
import { api, describeError } from "../api/client";
import { useLive } from "../hooks/useLive";
import type { SignalsData, SupportedPid, ToolResult } from "../api/types";
import { ErrorBanner, FailedResult, Spinner, Value, Warnings } from "./primitives";
import { Explain, PaneIntro, useExplain } from "../explain";
import { Sparkline } from "./Sparkline";

const MAX_SIGNALS = 32;

export function LivePane({
  moduleKey,
  onEvidence,
}: {
  moduleKey: string | null;
  onEvidence: (ref: number) => void;
}) {
  const [signals, setSignals] = useState<ToolResult<SignalsData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [interval, setIntervalMs] = useState(500);

  const live = useLive(!!moduleKey);

  const load = useCallback(async () => {
    if (!moduleKey) return;
    setBusy(true);
    setError(null);
    try {
      const res = await api.signals(moduleKey);
      setSignals(res);
      // Default to a couple of decodable signals so the first run shows something.
      const decodable = (res.data?.pids ?? []).filter((p) => p.decoder_available && p.signal_id);
      const preferred = ["engine_rpm", "coolant_temp", "vehicle_speed"];
      const pick = decodable
        .filter((p) => preferred.includes(p.signal_id!))
        .map((p) => p.signal_id!);
      setSelected(pick.length ? pick : decodable.slice(0, 3).map((p) => p.signal_id!));
    } catch (e) {
      setError(describeError(e));
      setSignals(null);
    } finally {
      setBusy(false);
    }
  }, [moduleKey]);

  // Changing module must stop the stream. The socket is per-connection, not
  // per-module, so a subscription left running would keep sampling the module
  // the user just navigated away from while the page describes a different one.
  useEffect(() => {
    setSignals(null);
    setSelected([]);
    live.unsubscribe();
    void load();
    // `live.unsubscribe` is stable; re-running this when it changes identity
    // would cancel the stream the user just started.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load]);

  const decodable = useMemo(
    () => (signals?.data?.pids ?? []).filter((p) => p.decoder_available && p.signal_id),
    [signals],
  );
  const undecodable = useMemo(
    () => (signals?.data?.pids ?? []).filter((p) => !p.decoder_available),
    [signals],
  );

  const streaming = !!live.subscribed;
  const minInterval = live.limits?.min_interval_ms ?? 50;
  const maxSignals = live.limits?.max_signals ?? MAX_SIGNALS;
  const { easy } = useExplain();

  function toggle(signalId: string) {
    setSelected((s) =>
      s.includes(signalId) ? s.filter((x) => x !== signalId)
        : s.length >= maxSignals ? s
        : [...s, signalId],
    );
  }

  /**
   * Select everything the socket will accept.
   *
   * The cap is the server's, not a preference: it is what the adapter can
   * actually sample inside one interval. Taking the first N in the order the
   * vehicle advertised them is arbitrary but predictable, and the count is
   * shown so a truncated selection is never a surprise.
   */
  function selectAll() {
    setSelected(decodable.slice(0, maxSignals).map((p) => p.signal_id!));
  }

  const allSelected = decodable.length > 0 && selected.length >= Math.min(decodable.length, maxSignals);
  const overCap = decodable.length > maxSignals;

  function start() {
    if (moduleKey && selected.length) live.subscribe(moduleKey, selected, interval);
  }

  if (!moduleKey) return <div className="empty">Select a module.</div>;

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="live_data" />

      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <div className="row">
          <strong>Live data</strong>
          {!easy && <span className="faint mono">{moduleKey}</span>}
          {signals?.data && (
            <span className="faint">
              {easy
                ? `${decodable.length} readings available`
                : `${signals.data.count} supported PIDs, ${decodable.length} decodable`}
            </span>
          )}
        </div>
        <div className="row">
          <label className="faint" htmlFor="interval">
            {easy ? "refresh" : "interval"}
          </label>
          <select
            id="interval"
            value={interval}
            onChange={(e) => setIntervalMs(Number(e.target.value))}
            disabled={streaming}
          >
            {[100, 250, 500, 1000, 2000].map((ms) => (
              <option key={ms} value={ms} disabled={ms < minInterval}>{ms} ms</option>
            ))}
          </select>
          {streaming ? (
            <button className="danger" onClick={live.unsubscribe}>Stop</button>
          ) : (
            <button className="primary" onClick={start} disabled={!selected.length || live.socket !== "open"}>
              Start
            </button>
          )}
          <button onClick={() => void load()} disabled={busy}>
            {busy ? <Spinner /> : easy ? "Refresh list" : "Reload PIDs"}
          </button>
        </div>
      </div>

      <ErrorBanner error={error} />
      {signals && !signals.success && <FailedResult result={signals} onEvidence={onEvidence} />}
      {signals && <Warnings warnings={signals.warnings} />}
      {live.error && (
        <div className="banner error">
          <span className="b-code">{live.error.code}</span>
          <span>{live.error.message}</span>
        </div>
      )}
      <Warnings warnings={live.warnings} />

      {live.subscribed && (
        <div className="row faint" style={{ marginBottom: 10 }}>
          <span className="pill"><span className="dot ready" />streaming</span>
          <span>
            sampling {live.subscribed.signals.length} signal
            {live.subscribed.signals.length === 1 ? "" : "s"} every{" "}
            <strong>{live.subscribed.interval_ms} ms</strong>
            {live.subscribed.interval_ms !== interval && ` (clamped from ${interval} ms)`}
          </span>
          {live.latest?.execution_time_ms != null && (
            <span className={live.latest.execution_time_ms > live.subscribed.interval_ms ? "" : "faint"}>
              last round trip {live.latest.execution_time_ms} ms
              {live.latest.execution_time_ms > live.subscribed.interval_ms &&
                " - the adapter cannot keep up with this set; drop a signal or slow down"}
            </span>
          )}
        </div>
      )}

      <div style={{ display: "grid", gridTemplateColumns: "minmax(0,1fr) 280px", gap: 16 }}>
        <div>
          {live.latest ? (
            <div className="grid-2">
              {live.subscribed?.signals.map((id) => {
                const v = live.latest!.values[id];
                if (!v) {
                  return (
                    <div className="card muted" key={id}>
                      <div className="mono faint" style={{ fontSize: 11 }}>{id}</div>
                      not returned in the last sample
                    </div>
                  );
                }
                const series = live.history
                  .map((s) => {
                    const val = s.values[id]?.value;
                    return val && (val.type === "number" || val.type === "integer")
                      ? { at: s.at, y: val.value }
                      : null;
                  })
                  .filter((p): p is { at: number; y: number } => p !== null);
                return (
                  <div key={id}>
                    <Value value={v} onEvidence={onEvidence} animate />
                    {series.length > 1 && <Sparkline points={series} unit={v.unit} />}
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="empty">
              {streaming ? <Spinner label="Waiting for the first sample" />
                : selected.length ? "Press Start."
                : "Choose at least one signal."}
            </div>
          )}
        </div>

        <div>
          <div className="section">
            <div className="row" style={{ justifyContent: "space-between", marginBottom: 8 }}>
              <h2 style={{ margin: 0 }}>
                {easy ? "What to watch" : "Signals"} ({selected.length}/{maxSignals})
              </h2>
              <div className="row" style={{ gap: 4 }}>
                <button
                  className="mini"
                  onClick={selectAll}
                  disabled={streaming || allSelected || !decodable.length}
                  title={
                    overCap
                      ? `The adapter can sample ${maxSignals} at once; the first ${maxSignals} are selected.`
                      : "Select every reading this car offers."
                  }
                >
                  All{overCap ? ` (${maxSignals})` : ""}
                </button>
                <button
                  className="mini"
                  onClick={() => setSelected([])}
                  disabled={streaming || !selected.length}
                >
                  None
                </button>
              </div>
            </div>
            <Explain kind="concept" id="live_data" />
            <div className="checks">
              {decodable.map((p) => (
                <PidCheck
                  key={p.pid}
                  pid={p}
                  checked={selected.includes(p.signal_id!)}
                  disabled={streaming}
                  onToggle={() => toggle(p.signal_id!)}
                />
              ))}
              {!decodable.length && !busy && (
                <span className="faint">This module reported nothing this build can read.</span>
              )}
            </div>
          </div>

          {undecodable.length > 0 && (
            <div className="section">
              <h2>
                {easy ? "Offered, but we can't read these" : "Supported, not decodable"} (
                {undecodable.length})
              </h2>
              <Explain kind="concept" id="supported_not_decodable" />
              {/* Shown as bare identifiers on purpose: naming them would mean
                  guessing what they measure, which is the one thing this
                  project will not do. The list is the honest form of "we do
                  not know yet". */}
              <div className="mono faint" style={{ fontSize: 11, lineHeight: 1.7, marginTop: 6 }}>
                {undecodable.map((p) => p.hex).join("  ")}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function PidCheck({
  pid,
  checked,
  disabled,
  onToggle,
}: {
  pid: SupportedPid;
  checked: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  const unverified = pid.verification && pid.verification !== "verified";
  const { easy, text } = useExplain();
  const explanation = pid.signal_id ? text("signal", pid.signal_id) : null;
  return (
    <label
      className={`check${disabled ? " disabled" : ""}`}
      // The full explanation on hover, at whichever level is selected. This is
      // the cheapest place to answer "what even is this?" without the picker
      // turning into a wall of prose.
      title={explanation ?? undefined}
    >
      <input type="checkbox" checked={checked} disabled={disabled} onChange={onToggle} />
      <span style={{ flex: 1, minWidth: 0 }}>
        <span style={{ display: "block", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {pid.name ?? pid.signal_id}
        </span>
        <span className="mono faint" style={{ fontSize: 11 }}>
          {easy
            ? (pid.unit ?? "")
            : `${pid.hex} ${pid.signal_id}${pid.unit ? ` (${pid.unit})` : ""}`}
        </span>
      </span>
      {unverified && <span className="tag unverified">{easy ? "unchecked" : "unver"}</span>}
    </label>
  );
}
