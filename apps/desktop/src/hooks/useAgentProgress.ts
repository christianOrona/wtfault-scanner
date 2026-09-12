// What the agent is doing, while it is doing it.
//
// An inspection takes minutes, and until now the UI showed a spinner and a
// clock. The honest question that provokes — "is this stuck?" — was asked twice
// during development by the person who wrote the thing, which is a good sign it
// was unanswerable.
//
// Nothing new is needed on the server: every tool call is already written to the
// flight recorder as it happens. This reads that log while the run is in flight
// and turns it into a running commentary.

import { useEffect, useRef, useState } from "react";
import { api } from "../api/client";
import type { SessionEvent } from "../api/types";

export interface ProgressLine {
  /** Event row id, unique and ordered. */
  seq: number;
  /** Wall-clock time it happened. */
  at: string;
  /** What the agent did, in plain words. */
  text: string;
  /** True once the corresponding read came back. */
  done: boolean;
  /** False when the vehicle refused or the arguments were wrong. */
  ok: boolean;
}

/** Tool names rendered for someone who does not know the API. */
const PLAIN: Record<string, string> = {
  identify_vehicle: "Reading the VIN",
  scan_modules: "Looking for control modules",
  read_dtcs: "Reading trouble codes",
  read_freeze_frame: "Reading the freeze frame",
  read_live_data: "Reading live sensor data",
  read_supported_pids: "Checking what this module can report",
  get_module_identity: "Asking a module to identify itself",
  adapter_health: "Checking the adapter",
  read_monitor_tests: "Reading the self-test results",
  read_readiness: "Checking which self-tests have finished",
  list_vehicle_features: "Looking up configurable settings",
  preview_configuration_change: "Working out what a change would involve",
  list_features: "Looking up configurable settings",
  clear_dtcs: "Clearing stored codes",
  // These reach the log too and had no entry, so they rendered as raw
  // identifiers in the middle of plain English — `scan_all_modules` appeared
  // exactly like that on a real inspection.
  scan_all_modules: "Asking every computer in the car for its faults",
  as_built_status: "Checking for a factory configuration file",
  forget_as_built: "Forgetting the factory configuration file",
  list_captures: "Looking at what has been recorded",
  list_catalog_signals: "Looking up what this vehicle can report",
  connect: "Connecting to the vehicle",
  disconnect: "Disconnecting",
  // Found by the test that compares this list against the tools that actually
  // reach the recorder. Every one of these would have appeared as a raw
  // identifier the first time it ran.
  read_pid: "Reading one sensor",
  read_feature: "Looking up one setting",
  read_catalog_signal: "Reading a value from the catalogue",
  check_procedure: "Checking whether the car is in the right state",
  run_procedure: "Taking a measurement under held conditions",
  capture_configuration: "Recording a module's current configuration",
  apply_configuration_change: "Writing a setting to the car",
  import_as_built: "Reading the factory configuration file",
  probe_module_capabilities: "Asking a module what it supports",
  probe_write_gate: "Checking whether this module accepts changes",
};

const describe = (tool: string, args: unknown): string => {
  const base = PLAIN[tool] ?? tool;
  const module = (args as { module?: string } | null)?.module;
  return module ? `${base} from ${module}` : base;
};

/**
 * Follow a running agent task by polling the session's event log.
 *
 * Polling rather than the websocket: the flight-recorder stream exists and works,
 * but it replays the entire session backlog first, and this only ever wants the
 * tail. A three-second poll costs one small request and cannot fall behind in a
 * way that needs recovering from.
 */
export function useAgentProgress(sessionId: string | null, active: boolean) {
  const [lines, setLines] = useState<ProgressLine[]>([]);
  const seen = useRef(0);
  /** False until the first poll of a run has skipped past existing history. */
  const primed = useRef(false);

  useEffect(() => {
    if (!active || !sessionId) {
      seen.current = 0;
      primed.current = false;
      setLines([]);
      return;
    }

    let cancelled = false;
    let timer: number | undefined;

    const tick = async () => {
      try {
        const page = await api.events(sessionId, seen.current, 500);
        if (cancelled) return;
        for (const e of page.events) seen.current = Math.max(seen.current, e.seq);

        // The first poll of a run only moves the cursor to the end of the log.
        //
        // Without this a second inspection in the same session replays the
        // first one: `seen` resets to zero on activation, so the opening poll
        // returns every tool call the session has ever recorded and the
        // progress list showed the whole run twice. The log is per-session and
        // the run is not, so "everything so far" is the wrong starting point —
        // "everything from now" is the right one.
        if (!primed.current) {
          primed.current = true;
          if (!cancelled) timer = window.setTimeout(tick, 800);
          return;
        }

        const fresh = toLines(page.events);
        if (fresh.length) {
          setLines((prev) => merge(prev, fresh));
        }
      } catch {
        // A failed poll is not worth reporting: the inspection itself is the
        // thing being watched, and it reports its own failures.
      }
      if (!cancelled) timer = window.setTimeout(tick, 3000);
    };

    void tick();
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
  }, [sessionId, active]);

  return lines;
}

function toLines(events: SessionEvent[]): ProgressLine[] {
  const out: ProgressLine[] = [];
  for (const e of events) {
    const k = e.kind as Record<string, unknown> & { kind: string };

    // Only the agent's own work. A reading the user clicked for is not progress
    // on the inspection, and mixing them would misrepresent what it did.
    if (k.kind === "tool_invoked" && String(k.initiator ?? "").startsWith("agent")) {
      out.push({
        seq: e.seq,
        at: e.timestamp,
        text: describe(String(k.tool), k.arguments),
        done: false,
        ok: true,
      });
    }
    if (k.kind === "tool_completed") {
      out.push({
        seq: e.seq,
        at: e.timestamp,
        text: `__done__${String(k.tool)}`,
        done: true,
        ok: k.success === true,
      });
    }
  }
  return out;
}

/**
 * Fold completions into the call they belong to, so one read is one line rather
 * than two.
 */
function merge(prev: ProgressLine[], fresh: ProgressLine[]): ProgressLine[] {
  const out = [...prev];
  for (const line of fresh) {
    if (line.done && line.text.startsWith("__done__")) {
      const tool = line.text.slice("__done__".length);
      // Close the most recent open line for this tool.
      for (let i = out.length - 1; i >= 0; i--) {
        if (!out[i].done && PLAIN[tool] && out[i].text.startsWith(PLAIN[tool])) {
          out[i] = { ...out[i], done: true, ok: line.ok };
          break;
        }
        if (!out[i].done && out[i].text.startsWith(tool)) {
          out[i] = { ...out[i], done: true, ok: line.ok };
          break;
        }
      }
      continue;
    }
    if (!out.some((l) => l.seq === line.seq)) out.push(line);
  }
  return out;
}
