// The Flight Recorder stream: `GET /api/v1/sessions/{id}/stream?after_seq=N`.
//
// The server replays the backlog, sends `hello` to mark the boundary, then
// follows live. If the client falls behind it gets a `lagged` frame naming the
// sequence to resume from - the server tells you rather than dropping events
// silently, so the gap is re-read over HTTP and the log stays gap-free.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, wsUrl } from "../api/client";
import type { ApiError, SessionEvent, StreamFrame } from "../api/types";

const MAX_EVENTS = 5000;

export interface RecorderState {
  socket: "closed" | "connecting" | "open";
  events: SessionEvent[];
  /** False while the backlog is still replaying (before the `hello` frame). */
  live: boolean;
  error: ApiError | null;
  /** Set when the stream reported a gap we are re-reading over HTTP. */
  recovering: boolean;
}

export function useFlightRecorder(sessionId: string | null, enabled: boolean) {
  const [state, setState] = useState<RecorderState>({
    socket: "closed",
    events: [],
    live: false,
    error: null,
    recovering: false,
  });
  const lastSeq = useRef(0);

  useEffect(() => {
    if (!sessionId || !enabled) {
      setState({ socket: "closed", events: [], live: false, error: null, recovering: false });
      lastSeq.current = 0;
      return;
    }

    let disposed = false;
    lastSeq.current = 0;
    setState({ socket: "connecting", events: [], live: false, error: null, recovering: false });

    const sock = new WebSocket(wsUrl(`/sessions/${encodeURIComponent(sessionId)}/stream?after_seq=0`));

    sock.onopen = () => setState((s) => ({ ...s, socket: "open" }));

    sock.onmessage = (ev) => {
      let frame: StreamFrame;
      try {
        frame = JSON.parse(ev.data as string) as StreamFrame;
      } catch {
        return;
      }

      if (frame.type === "event") {
        lastSeq.current = Math.max(lastSeq.current, frame.event.seq);
        setState((s) => ({ ...s, events: append(s.events, [frame.event]) }));
      } else if (frame.type === "hello") {
        setState((s) => ({ ...s, live: true }));
      } else if (frame.type === "error") {
        setState((s) => ({ ...s, error: frame.error }));
      } else if (frame.type === "lagged") {
        // Re-read the gap over HTTP from where the server says to resume.
        setState((s) => ({ ...s, recovering: true }));
        void api
          .events(sessionId, frame.resume_after_seq, 1000)
          .then((page) => {
            if (disposed) return;
            for (const e of page.events) lastSeq.current = Math.max(lastSeq.current, e.seq);
            setState((s) => ({ ...s, events: append(s.events, page.events), recovering: false }));
          })
          .catch(() => {
            if (!disposed) setState((s) => ({ ...s, recovering: false }));
          });
      }
    };

    sock.onclose = () => {
      if (!disposed) setState((s) => ({ ...s, socket: "closed", live: false }));
    };

    return () => {
      disposed = true;
      sock.close();
    };
  }, [sessionId, enabled]);

  /** Pull one event by its row id, for resolving a `raw_evidence_ref`. */
  const findEvent = useCallback(
    (ref: number) => state.events.find((e) => e.id === ref) ?? null,
    [state.events],
  );

  return { ...state, findEvent };
}

/** Merge by seq, keeping the log ordered and free of duplicates. */
function append(existing: SessionEvent[], incoming: SessionEvent[]): SessionEvent[] {
  if (!incoming.length) return existing;
  const bySeq = new Map(existing.map((e) => [e.seq, e]));
  for (const e of incoming) bySeq.set(e.seq, e);
  const merged = [...bySeq.values()].sort((a, b) => a.seq - b.seq);
  return merged.length > MAX_EVENTS ? merged.slice(merged.length - MAX_EVENTS) : merged;
}
