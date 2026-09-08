// Live data over `GET /api/v1/live`.
//
// Two rules from docs/API.md drive the shape of this hook:
//
//  * the server clamps interval_ms, and the `subscribed` frame reports the
//    interval actually used - so the UI must display that, not what it asked for.
//  * losing the adapter mid-stream sends an `error` frame and stops sampling,
//    but leaves the socket open so the user can see why. So an error here is
//    state to render, not a reason to tear the connection down.

import { useCallback, useEffect, useRef, useState } from "react";
import { wsUrl } from "../api/client";
import type { ApiError, DecodedValue, LiveFrame, ToolResult, Warning } from "../api/types";

export interface Sample {
  at: number;
  values: Record<string, DecodedValue>;
  execution_time_ms: number | null;
}

export interface LiveState {
  socket: "closed" | "connecting" | "open";
  subscribed: { module: string; signals: string[]; interval_ms: number } | null;
  limits: { min_interval_ms: number; max_signals: number } | null;
  latest: Sample | null;
  history: Sample[];
  warnings: Warning[];
  error: ApiError | null;
}

const HISTORY = 240; // ~2 minutes at 500 ms; enough to see a trend, bounded.

export function useLive(enabled: boolean) {
  const ws = useRef<WebSocket | null>(null);
  const pending = useRef<string | null>(null);
  const [state, setState] = useState<LiveState>({
    socket: "closed",
    subscribed: null,
    limits: null,
    latest: null,
    history: [],
    warnings: [],
    error: null,
  });

  useEffect(() => {
    if (!enabled) return;
    let closed = false;
    setState((s) => ({ ...s, socket: "connecting" }));

    const sock = new WebSocket(wsUrl("/live"));
    ws.current = sock;

    sock.onopen = () => {
      setState((s) => ({ ...s, socket: "open" }));
      if (pending.current) {
        sock.send(pending.current);
        pending.current = null;
      }
    };

    sock.onmessage = (ev) => {
      let frame: LiveFrame;
      try {
        frame = JSON.parse(ev.data as string) as LiveFrame;
      } catch {
        return;
      }
      setState((s) => reduce(s, frame));
    };

    sock.onclose = () => {
      if (!closed) setState((s) => ({ ...s, socket: "closed", subscribed: null }));
    };
    sock.onerror = () => {
      /* onclose carries the outcome; nothing useful is in the error event. */
    };

    return () => {
      closed = true;
      ws.current = null;
      sock.close();
    };
  }, [enabled]);

  const subscribe = useCallback((module: string, signals: string[], interval_ms: number) => {
    const msg = JSON.stringify({ type: "subscribe", module, signals, interval_ms });
    // Clear history: a new signal set makes the old series meaningless.
    setState((s) => ({ ...s, history: [], latest: null, error: null, warnings: [] }));
    if (ws.current?.readyState === WebSocket.OPEN) ws.current.send(msg);
    else pending.current = msg;
  }, []);

  const unsubscribe = useCallback(() => {
    if (ws.current?.readyState === WebSocket.OPEN) {
      ws.current.send(JSON.stringify({ type: "unsubscribe" }));
    }
    pending.current = null;
  }, []);

  return { ...state, subscribe, unsubscribe };
}

function reduce(s: LiveState, frame: LiveFrame): LiveState {
  switch (frame.type) {
    case "hello":
      return {
        ...s,
        limits: { min_interval_ms: frame.min_interval_ms, max_signals: frame.max_signals },
      };
    case "subscribed":
      return {
        ...s,
        subscribed: { module: frame.module, signals: frame.signals, interval_ms: frame.interval_ms },
        error: null,
      };
    case "unsubscribed":
      return { ...s, subscribed: null };
    case "sample": {
      const sample = toSample(frame.result);
      // A sample whose ToolResult failed means nothing could be read. Surface
      // it rather than letting a graph flatline silently.
      if (!frame.result.success) {
        return { ...s, error: frame.result.error, warnings: frame.result.warnings };
      }
      const history = [...s.history, sample];
      if (history.length > HISTORY) history.splice(0, history.length - HISTORY);
      return { ...s, latest: sample, history, warnings: frame.result.warnings, error: null };
    }
    case "error":
      return { ...s, error: frame.error };
    default:
      return s;
  }
}

function toSample(result: ToolResult): Sample {
  const values: Record<string, DecodedValue> = {};
  for (const v of result.values) values[v.signal_id] = v;
  return { at: Date.parse(result.timestamp) || Date.now(), values, execution_time_ms: result.execution_time_ms };
}
