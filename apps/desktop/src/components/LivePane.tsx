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
import type { DecodedValue, SignalsData, SupportedPid, ToolResult } from "../api/types";
import type { Sample } from "../hooks/useLive";
import { ErrorBanner, FailedResult, Spinner, Value, Warnings } from "./primitives";
import { Explain, PaneIntro, useExplain } from "../explain";
import { Sparkline } from "./Sparkline";
import { download, scanFilename, toCsv } from "./exportFile";

const MAX_SIGNALS = 32;

/**
 * Roughly how long one PID costs on an ELM327-class adapter, round trip.
 *
 * Measured on the development truck at 500000 baud: a 32-signal set on a 100 ms
 * interval took 2286 ms, which is about 71 ms per signal. Used to stop the app
 * from letting someone request a set it cannot possibly deliver on time.
 */
const ROUND_TRIP_PER_SIGNAL_MS = 75;

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
      // Start on a handful of real gauges, not on whatever came first in the
      // mask — which is how "PIDs supported [01-20]" ended up as the opening
      // card of the dashboard.
      const gauges = (res.data?.pids ?? []).filter(
        (p) => p.decoder_available && p.signal_id && p.is_measurement !== false,
      );
      const preferred = ["engine_rpm", "coolant_temp", "vehicle_speed", "engine_load"];
      const pick = gauges
        .filter((p) => preferred.includes(p.signal_id!))
        .map((p) => p.signal_id!);
      setSelected(pick.length ? pick : gauges.slice(0, 4).map((p) => p.signal_id!));
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
   * The signals that are actually gauges.
   *
   * "Supported PIDs [01-20]" is a list of which questions the car will answer.
   * "Monitor status" is a set of flags. Both are real and worth reading — they
   * are on the Inspect screen, where they mean something — but neither belongs
   * on a strip chart, and selecting All used to put four of them there.
   */
  const measurements = useMemo(
    () => decodable.filter((p) => p.is_measurement !== false),
    [decodable],
  );
  const notGauges = useMemo(
    () => decodable.filter((p) => p.is_measurement === false),
    [decodable],
  );

  /**
   * Select everything worth watching, within what the adapter can keep up with.
   *
   * Two caps, and the tighter one wins. The socket's own limit is a hard
   * maximum. The softer one is arithmetic: an ELM327 needs roughly 60-80 ms per
   * PID, so asking for thirty of them on a 100 ms interval guarantees every
   * sample arrives incomplete. That is exactly what happened — 32 signals, a
   * 2286 ms round trip, and a dashboard that blinked.
   */
  function selectAll() {
    setSelected(measurements.slice(0, affordable).map((p) => p.signal_id!));
  }

  /** How many signals this interval can realistically carry. */
  const affordable = Math.max(
    3,
    Math.min(maxSignals, Math.floor(interval / ROUND_TRIP_PER_SIGNAL_MS)),
  );
  const allSelected =
    measurements.length > 0 && selected.length >= Math.min(measurements.length, affordable);
  const overCap = measurements.length > affordable;

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
          {/* Every sample still in the buffer, not just the one on screen. A
              strip chart you cannot take away with you is a strip chart you
              have to sit and watch. */}
          <button
            disabled={!live.history.length}
            title="Save every reading captured so far as a spreadsheet."
            onClick={() =>
              download(
                scanFilename("live", null, "csv"),
                toCsv(
                  ["time", "signal", "value", "unit", "verification"],
                  live.history.flatMap((s) =>
                    Object.entries(s.values).map(([id, v]) => [
                      new Date(s.at).toISOString(),
                      id,
                      v.value.type === "number" || v.value.type === "integer"
                        ? v.value.value
                        : JSON.stringify(v.value),
                      v.unit ?? "",
                      v.provenance.verification,
                    ]),
                  ),
                ),
                "text/csv",
              )
            }
          >
            Export
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
                // The last value we ever saw, not just the one in this sample.
                //
                // At 32 signals the adapter cannot answer them all inside one
                // interval, so each sample returns a different subset and every
                // card blanked to "not returned in the last sample" and back
                // again. That is the flicker: the data was fine, the rendering
                // threw it away. A reading that is two seconds old is still a
                // reading — it just has to say so.
                const v = live.latest!.values[id] ?? lastSeen(live.history, id);
                const fresh = !!live.latest!.values[id];
                if (!v) {
                  return (
                    <div className="card muted" key={id}>
                      <div style={{ fontWeight: 500 }}>{friendlyName(id, decodable)}</div>
                      <div className="faint" style={{ fontSize: 12, marginTop: 4 }}>
                        waiting for the first reading
                      </div>
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
                    <Value value={v} onEvidence={onEvidence} animate={fresh} stale={!fresh} compact />
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
                {easy ? "What to watch" : "Signals"} ({selected.length}/{affordable})
              </h2>
              <div className="row" style={{ gap: 4 }}>
                <button
                  className="mini"
                  onClick={selectAll}
                  disabled={streaming || allSelected || !measurements.length}
                  title={
                    overCap
                      ? `At ${interval} ms the adapter can carry about ${affordable} readings. ` +
                        `Selecting more than that guarantees every sample arrives incomplete.`
                      : "Watch every reading this car offers."
                  }
                >
                  All{overCap ? ` (${affordable})` : ""}
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
              {measurements.map((p) => (
                <PidCheck
                  key={p.pid}
                  pid={p}
                  checked={selected.includes(p.signal_id!)}
                  disabled={streaming}
                  onToggle={() => toggle(p.signal_id!)}
                />
              ))}
              {!measurements.length && !busy && (
                <span className="faint">This module reported nothing this build can read.</span>
              )}
            </div>
          </div>

          {/* Real readings, deliberately not offered as gauges. Listed rather
              than hidden: a person who ticked "Monitor status" last time and
              cannot find it now deserves to know where it went, and where it
              actually lives. */}
          {notGauges.length > 0 && (
            <div className="section">
              <h2>{easy ? "Not gauges" : "Status, not measurements"} ({notGauges.length})</h2>
              <div className="explain">
                These are lists and flags rather than numbers, so they do not plot on a
                graph. You can see them on the Inspect screen, where they are read once and
                explained properly.
              </div>
              <div className="faint" style={{ fontSize: 12, lineHeight: 1.8, marginTop: 6 }}>
                {notGauges.map((p) => p.name ?? p.signal_id).join(" · ")}
              </div>
            </div>
          )}

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

/**
 * The most recent value for a signal anywhere in the history buffer.
 *
 * Needed because a sample is not a snapshot of everything. The adapter answers
 * what it can inside the interval, so any given sample carries a subset, and
 * reading only the latest one makes two thirds of the dashboard blink.
 */
function lastSeen(history: Sample[], id: string): DecodedValue | null {
  for (let i = history.length - 1; i >= 0; i--) {
    const v = history[i].values[id];
    if (v) return v;
  }
  return null;
}

/** A signal's human name, falling back to its id when the list has not loaded. */
function friendlyName(id: string, decodable: SupportedPid[]): string {
  return decodable.find((p) => p.signal_id === id)?.name ?? id;
}
