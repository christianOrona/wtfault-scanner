// The Flight Recorder. Every adapter request, reply, decision and reading, in
// order, with no gaps.
//
// This is where `raw_evidence_ref` lands: a value elsewhere in the UI links
// here, and the row it selects is the literal adapter exchange behind it. That
// round trip is the whole point of the recorder, so the highlighted row is
// scrolled to and marked rather than merely filtered for.

import { useEffect, useMemo, useRef, useState } from "react";
import type { SessionEvent } from "../api/types";
import { saveFile, scanFilename, toCsv } from "./exportFile";
import { Spinner, localTime } from "./primitives";

const KINDS = [
  "all", "adapter", "dtc", "measurement", "module", "safety", "tool", "session",
] as const;
type Filter = (typeof KINDS)[number];

export function RecorderPane({
  events,
  live,
  socket,
  recovering,
  highlight,
  onHighlightShown,
}: {
  events: SessionEvent[];
  live: boolean;
  socket: "closed" | "connecting" | "open";
  recovering: boolean;
  highlight: number | null;
  onHighlightShown: () => void;
}) {
  const [filter, setFilter] = useState<Filter>("all");
  const [follow, setFollow] = useState(true);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef(new Map<number, HTMLDivElement>());

  const shown = useMemo(
    () => (filter === "all" ? events : events.filter((e) => group(e) === filter)),
    [events, filter],
  );

  // Jumping to an evidence ref wins over following the tail.
  useEffect(() => {
    if (highlight == null) return;
    const row = rowRefs.current.get(highlight);
    if (row) {
      setFollow(false);
      row.scrollIntoView({ block: "center", behavior: "smooth" });
    }
    const t = setTimeout(onHighlightShown, 4000);
    return () => clearTimeout(t);
  }, [highlight, onHighlightShown, shown.length]);

  useEffect(() => {
    if (follow && listRef.current) listRef.current.scrollTop = listRef.current.scrollHeight;
  }, [shown.length, follow]);

  return (
    <div className="pane" style={{ display: "flex", flexDirection: "column", padding: 0 }}>
      <div className="row" style={{ padding: "10px 16px", borderBottom: "1px solid var(--line)", justifyContent: "space-between" }}>
        <div className="row">
          <strong>Flight recorder</strong>
          <span className="term" title="Every question we asked the car and every answer it gave, in order, exactly as it happened. This is the proof behind every number in the app.">what is this?</span>
          <span className="faint">{events.length} events</span>
          {socket === "connecting" && <Spinner label="connecting" />}
          {socket === "open" && !live && <Spinner label="replaying backlog" />}
          {live && <span className="pill"><span className="dot ready" />live</span>}
          {socket === "closed" && <span className="pill"><span className="dot" />stream closed</span>}
          {recovering && <Spinner label="re-reading a gap" />}
        </div>
        <div className="row">
          <select value={filter} onChange={(e) => setFilter(e.target.value as Filter)}>
            {KINDS.map((k) => <option key={k} value={k}>{k}</option>)}
          </select>
          <label className="check" style={{ padding: 0 }}>
            <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
            follow
          </label>
          <button
            disabled={!shown.length}
            title="Write these events to a file, including the raw adapter lines"
            onClick={() =>
              void saveFile(
                // The filter is in the name. A file holding a filtered view and
                // called "recorder" would read as the whole recording, which is
                // the sort of quiet lie this pane exists to prevent.
                scanFilename(filter === "all" ? "recorder" : `recorder-${filter}`, null, "csv"),
                toCsv(
                  ["seq", "timestamp", "kind", "detail", "raw"],
                  shown.map((e) => [e.seq, e.timestamp, e.kind.kind, plain(e), raw(e)]),
                ),
              )
                .then((r) => setSavedTo(r.path))
                .catch(() => setSavedTo("could not save"))
            }
          >
            Export
          </button>
        </div>
      </div>
      {savedTo && (
        <div className="faint" style={{ padding: "6px 16px" }}>
          {savedTo === "could not save" ? savedTo : `Saved to ${savedTo}`}
          {filter !== "all" && savedTo !== "could not save" && ` — ${filter} events only`}
        </div>
      )}

      <div className="events" ref={listRef} style={{ flex: 1, overflowY: "auto", minHeight: 0 }}>
        {!shown.length && <div className="empty">No events yet.</div>}
        {shown.map((e) => (
          <div
            key={e.seq}
            ref={(el) => {
              if (el) rowRefs.current.set(e.id, el);
              else rowRefs.current.delete(e.id);
            }}
            className={`ev-row${highlight === e.id ? " highlight" : ""}`}
          >
            <span className="seq">{e.seq}</span>
            <span className="time">{localTime(e.timestamp)}</span>
            <span className="kind">{e.kind.kind}</span>
            <span className="detail">{describe(e)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function group(e: SessionEvent): Filter {
  const k = e.kind.kind;
  if (k.startsWith("adapter")) return "adapter";
  if (k === "dtc_read") return "dtc";
  if (k === "measurement_recorded") return "measurement";
  if (k === "module_discovered") return "module";
  if (k === "safety_decision") return "safety";
  if (k.startsWith("tool_")) return "tool";
  return "session";
}

function describe(e: SessionEvent): React.ReactNode {
  const k = e.kind as Record<string, unknown> & { kind: string };
  switch (k.kind) {
    case "adapter_request":
      return <span>&gt; {String(k.command)}</span>;
    case "adapter_response": {
      const cls = String(k.classification);
      const lines = (k.lines as string[] | undefined) ?? [];
      return (
        <span>
          &lt; {String(k.command)}{"  "}
          <span className={`cls-${cls}`}>[{cls}]</span>{"  "}
          {lines.join(" | ")}
          <span className="faint">  {String(k.elapsed_ms)} ms</span>
        </span>
      );
    }
    case "adapter_failure": {
      const err = k.error as { code?: string; message?: string } | undefined;
      return <span className="cls-bus_error">! {String(k.command)} - {err?.code}: {err?.message}</span>;
    }
    case "connection_state_changed": {
      const to = k.to as { state?: string } | undefined;
      return <span>{String(k.from)} -&gt; {to?.state}</span>;
    }
    case "adapter_identified":
      return <span>adapter capabilities recorded</span>;
    case "session_started":
      return <span>{k.label ? String(k.label) : "session started"}</span>;
    case "session_ended":
      return <span>session ended</span>;
    case "vehicle_identified":
      return <span>VIN {String(k.vin)}</span>;
    case "module_discovered":
      return <span>{String(k.module_key)} at {String(k.address)}</span>;
    case "dtc_read":
      return <span>{String(k.module_key)}  {String(k.code)}  {String(k.status)}</span>;
    case "measurement_recorded":
      return (
        <span>
          {String(k.signal_id)} = {k.value == null ? "null" : String(k.value)} {k.unit ? String(k.unit) : ""}
          {k.raw_hex ? <span className="faint">  raw {String(k.raw_hex)}</span> : null}
        </span>
      );
    case "safety_decision":
      return (
        <span className={k.allowed ? "" : "cls-bus_error"}>
          {String(k.operation)} [{String(k.level)}] {k.allowed ? "allowed" : "REFUSED"} for{" "}
          {String(k.initiator)}
          {k.reason ? ` - ${String(k.reason)}` : ""}
        </span>
      );
    case "tool_invoked":
      return <span>{String(k.tool)} by {String(k.initiator)} {JSON.stringify(k.arguments)}</span>;
    case "tool_completed": {
      const warns = (k.warnings as unknown[] | undefined)?.length ?? 0;
      return (
        <span>
          {String(k.tool)} {k.success ? "ok" : "failed"} in {String(k.execution_time_ms)} ms
          {warns > 0 && <span className="cls-no_data">  {warns} warning{warns === 1 ? "" : "s"}</span>}
        </span>
      );
    }
    case "user_note":
      return <span>{String(k.text)}</span>;
    default:
      return <span className="faint">{JSON.stringify(k)}</span>;
  }
}

/** The same content as `describe`, as text rather than as elements.
 *
 * Deliberately a second function rather than a stringified render: `describe`
 * returns React nodes for the screen, and coaxing text out of those would give
 * a file whose content depended on how the row happened to be marked up. */
function plain(e: SessionEvent): string {
  const k = e.kind as Record<string, unknown> & { kind: string };
  switch (k.kind) {
    case "adapter_request":
      return `> ${String(k.command)}`;
    case "adapter_response":
      return `< ${String(k.command)} [${String(k.classification)}]`;
    default:
      return Object.entries(k)
        .filter(([key]) => key !== "kind")
        .map(([key, v]) => `${key}=${typeof v === "object" ? JSON.stringify(v) : String(v)}`)
        .join(" ");
  }
}

/** The literal adapter lines, when the event has any.
 *
 * Kept in their own column and unsummarised. These lines are the evidence every
 * `raw_evidence_ref` in the application points at, and a file that paraphrased
 * them would not be worth exporting. */
function raw(e: SessionEvent): string {
  const k = e.kind as Record<string, unknown> & { kind: string };
  const lines = k.lines as string[] | undefined;
  return lines?.length ? lines.join(" | ") : "";
}
