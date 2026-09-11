// Ask the agent anything, and let it read the vehicle to answer.
//
// Only prose is sent back to the server. The tool traffic from previous turns
// is deliberately not replayed from here: the client is not the authority on
// what the truck said, and letting it post tool results would be a way to put
// words in the diagnostic core's mouth.

import { useEffect, useRef, useState } from "react";
import { api, describeError } from "../api/client";
import type { AgentStatus, TraceEntry } from "../api/types";
import { ErrorBanner, Spinner } from "./primitives";
import { PaneIntro } from "../explain";
import { TraceList } from "./InspectPane";

interface Turn {
  role: "user" | "assistant";
  content: string;
  trace?: TraceEntry[];
}

/**
 * Render `**bold**` and `` `code` `` and leave everything else alone.
 *
 * Models write markdown whether or not you ask them to, and showing the raw
 * asterisks looks broken. This handles the two markers that actually appear in
 * practice rather than pulling in a markdown library and its sanitiser — the
 * text is inserted as React children, never as HTML, so there is no injection
 * surface to defend.
 */
function renderInline(text: string): React.ReactNode[] {
  const parts: React.ReactNode[] = [];
  const pattern = /\*\*(.+?)\*\*|`([^`]+)`/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let key = 0;

  while ((m = pattern.exec(text)) !== null) {
    if (m.index > last) parts.push(text.slice(last, m.index));
    if (m[1] !== undefined) parts.push(<strong key={key++}>{m[1]}</strong>);
    else parts.push(<code key={key++}>{m[2]}</code>);
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

const SUGGESTIONS = [
  "What is wrong with this vehicle?",
  "Is it safe to drive home?",
  "Explain the codes like I know nothing about cars",
  "What would it cost to fix?",
];

export function AskPane({
  agent,
  connected,
  onEvidence,
  onOpenSettings,
  onFinished,
}: {
  agent: AgentStatus | null;
  connected: boolean;
  onEvidence: (ref: number) => void;
  onOpenSettings: () => void;
  /** A chat turn can scan the vehicle too, so the sidebar may be stale. */
  onFinished?: () => void;
}) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [turns.length, busy]);

  async function send(text: string) {
    const question = text.trim();
    if (!question || busy) return;

    const next: Turn[] = [...turns, { role: "user", content: question }];
    setTurns(next);
    setInput("");
    setBusy(true);
    setError(null);

    try {
      const res = await api.ask(next.map((t) => ({ role: t.role, content: t.content })));
      setTurns([
        ...next,
        {
          role: "assistant",
          content: res.text || "(the model returned nothing)",
          trace: res.trace,
        },
      ]);
      onFinished?.();
    } catch (e) {
      setError(describeError(e));
      // Keep the question on screen: retyping it after a provider hiccup is
      // pure friction.
      setTurns(next);
    } finally {
      setBusy(false);
    }
  }

  if (!agent?.ready) {
    return (
      <div className="pane">
        <div className="card">
          <strong>No model is set up yet.</strong>
          <p className="muted">{agent?.reason ?? "Add a provider to ask questions."}</p>
          <button className="primary" onClick={onOpenSettings}>Set one up</button>
        </div>
      </div>
    );
  }

  return (
    <div className="pane" style={{ display: "flex", flexDirection: "column", padding: 0 }}>
      <div style={{ flex: 1, overflowY: "auto", padding: 16, minHeight: 0 }}>
        <PaneIntro kind="concept" id="ask" />
        {!turns.length && (
          <div className="card">
            <strong>Ask about this vehicle</strong>
            <p className="muted">
              The agent will read whatever it needs to answer. It only reports what it actually
              measured, and says so when it is going on general knowledge instead.
            </p>
            <div className="row">
              {SUGGESTIONS.map((s) => (
                <button key={s} onClick={() => void send(s)} disabled={!connected}>{s}</button>
              ))}
            </div>
            {!connected && (
              <div className="faint" style={{ marginTop: 8 }}>
                Connect to a vehicle first.
              </div>
            )}
          </div>
        )}

        {turns.map((t, i) => (
          <div key={i} className="card" style={{
            borderLeft: t.role === "user" ? "3px solid var(--accent)" : "3px solid var(--line)",
          }}>
            <div className="faint" style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em", marginBottom: 4 }}>
              {t.role === "user" ? "you" : "ai mechanic"}
            </div>
            <div style={{ whiteSpace: "pre-wrap" }}>{renderInline(t.content)}</div>
            {!!t.trace?.length && (
              <details style={{ marginTop: 8 }}>
                <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
                  what it read to answer this ({t.trace.filter((e) => e.type === "tool").length} reads)
                </summary>
                <TraceList trace={t.trace} onEvidence={onEvidence} />
              </details>
            )}
          </div>
        ))}

        {busy && (
          <div className="card">
            <Spinner label="Reading the vehicle and thinking" />
          </div>
        )}

        {/* Under the conversation rather than beside the box: these are things
            to ask next, and they belong where the last answer ended. */}
        {!busy && <Suggestions turns={turns} connected={connected} busy={busy} onPick={send} />}

        <ErrorBanner error={error} />
        <div ref={endRef} />
      </div>

      <div style={{ borderTop: "1px solid var(--line)", padding: 12, flex: "0 0 auto" }}>
        <div className="row">
          <input
            type="text"
            style={{ flex: 1 }}
            value={input}
            placeholder={connected ? "Ask anything about this vehicle…" : "Connect to a vehicle first"}
            disabled={busy || !connected}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send(input);
              }
            }}
          />
          <button
            className="primary"
            onClick={() => void send(input)}
            disabled={busy || !connected || !input.trim()}
          >
            Ask
          </button>
          {turns.length > 0 && (
            <button onClick={() => { setTurns([]); setError(null); }} disabled={busy}>
              Clear
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** One thing worth asking next, and the question that asks it. */
interface Suggestion {
  label: string;
  prompt: string;
}

/**
 * Follow-ups that have not been done yet in this conversation.
 *
 * Deliberately derived from the **trace** — which tools actually ran — rather
 * than from the reply text. Reading the model's prose to guess what it meant
 * would make these buttons a second, worse interpretation of an answer that is
 * already on screen, and they would be wrong exactly when the answer was
 * surprising.
 *
 * What they are instead is a statement of fact: this has not been read yet, and
 * here is the question that reads it. Nothing here can do anything the person
 * could not already do from the tabs — each button sends an ordinary question
 * through the ordinary path, so every gate, confirmation and refusal along the
 * way still applies. A button is a shortcut, never a privilege.
 */
function suggest(turns: Turn[], connected: boolean): Suggestion[] {
  if (!connected) return [];

  const ran = new Set<string>();
  for (const t of turns) {
    for (const e of t.trace ?? []) {
      if (e.type === "tool") ran.add(e.name);
    }
  }

  const all: (Suggestion & { done: boolean })[] = [
    {
      label: "What vehicle is this?",
      prompt: "Identify this vehicle and tell me what you can read from the VIN.",
      done: ran.has("identify_vehicle"),
    },
    {
      label: "Any stored codes?",
      prompt: "Read the stored trouble codes and explain what each one means in plain English.",
      done: ran.has("read_dtcs"),
    },
    {
      label: "What's wearing out?",
      prompt:
        "Read the on-board self-test results and tell me which parts are passing but close to " +
        "their limits, before they set a code.",
      done: ran.has("read_monitor_tests"),
    },
    {
      label: "Ready for an emissions test?",
      prompt: "Check the emissions readiness monitors and tell me whether this would pass a test.",
      done: ran.has("read_readiness"),
    },
    {
      label: "Everything on the bus",
      prompt:
        "Scan every module on the vehicle, including the ones the emissions services do not " +
        "reach, and tell me what you found.",
      done: ran.has("scan_all_modules"),
    },
  ];

  // Only what has not been done. Offering to re-read something just read is
  // noise, and on a long conversation it would be most of the row.
  return all.filter((s) => !s.done).map(({ label, prompt }) => ({ label, prompt }));
}

/** The follow-up row. Hidden entirely when there is nothing useful left. */
function Suggestions({
  turns,
  connected,
  busy,
  onPick,
}: {
  turns: Turn[];
  connected: boolean;
  busy: boolean;
  onPick: (prompt: string) => void;
}) {
  const items = suggest(turns, connected);
  if (!items.length) return null;

  return (
    <div className="suggestions">
      <span className="faint">Or ask:</span>
      {items.map((s) => (
        <button
          key={s.label}
          className="mini"
          disabled={busy}
          title={s.prompt}
          onClick={() => onPick(s.prompt)}
        >
          {s.label}
        </button>
      ))}
    </div>
  );
}
