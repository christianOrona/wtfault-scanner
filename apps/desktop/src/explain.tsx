// Easy versus Advanced: one switch that changes what the whole app says.
//
// The problem this solves: every screen was written for someone who already
// knows what a PID is. Someone who does not gets numbers, hex, and identifiers,
// and no way in. So every explainable thing in the product carries two
// descriptions, and this decides which one the reader sees.
//
// The two modes are not "less information" and "more information". They are
// different readers:
//
//   Easy      - plain sentences, no identifiers, no provenance chrome. The
//               proof is still one click away, because hiding it would make the
//               app less honest, not simpler.
//   Advanced  - everything that was always there, plus the technical
//               explanation for anyone who wants the precise version.
//
// The explanations themselves come from the core, from a versioned data file,
// so an identifier with nothing written about it shows up as a visible gap
// rather than a sentence somebody invented in a component.

import {
  createContext, useCallback, useContext, useEffect, useMemo, useState,
  type ReactNode,
} from "react";
import { api } from "./api/client";
import type { ExplainKind, Explanation, ExplanationsResponse } from "./api/types";

export type ExplainMode = "easy" | "advanced";

const STORAGE_KEY = "wrenchgpt.explainMode";

interface ExplainContextValue {
  mode: ExplainMode;
  setMode: (m: ExplainMode) => void;
  easy: boolean;
  /** Look up one explanation, or null when this build has nothing to say. */
  lookup: (kind: ExplainKind, id: string) => Explanation | null;
  /** The text for the current mode, or null. */
  text: (kind: ExplainKind, id: string) => string | null;
  loaded: boolean;
}

const Ctx = createContext<ExplainContextValue | null>(null);

function initialMode(): ExplainMode {
  // Easy is the default. Someone who needs Advanced knows to go and find it;
  // someone who needs Easy does not know that they need it.
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return v === "advanced" ? "advanced" : "easy";
  } catch {
    return "easy";
  }
}

export function ExplainProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<ExplainMode>(initialMode);
  const [data, setData] = useState<ExplanationsResponse | null>(null);

  // Fetched once. The set never changes while the core is running, and every
  // screen needs it, so paying for it per component would be waste.
  useEffect(() => {
    let cancelled = false;
    const attempt = (tries: number) => {
      api
        .explanations()
        .then((r) => { if (!cancelled) setData(r); })
        .catch(() => {
          // The core may still be starting. Explanations are not worth an
          // error banner: the app is fully usable without them, just terser.
          if (!cancelled && tries > 0) setTimeout(() => attempt(tries - 1), 1500);
        });
    };
    attempt(5);
    return () => { cancelled = true; };
  }, []);

  const setMode = useCallback((m: ExplainMode) => {
    setModeState(m);
    try { localStorage.setItem(STORAGE_KEY, m); } catch { /* private window */ }
  }, []);

  const index = useMemo(() => {
    const build = (list: Explanation[] | undefined) =>
      new Map((list ?? []).map((e) => [e.id, e]));
    return {
      signal: build(data?.signals),
      code: build(data?.codes),
      concept: build(data?.concepts),
    };
  }, [data]);

  const lookup = useCallback(
    (kind: ExplainKind, id: string) => index[kind].get(id) ?? null,
    [index],
  );

  const value = useMemo<ExplainContextValue>(() => {
    const easy = mode === "easy";
    return {
      mode,
      setMode,
      easy,
      lookup,
      text: (kind, id) => {
        const e = lookup(kind, id);
        return e ? (easy ? e.easy : e.technical) : null;
      },
      loaded: data !== null,
    };
  }, [mode, setMode, lookup, data]);

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useExplain(): ExplainContextValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useExplain outside ExplainProvider");
  return v;
}

/** The header switch. */
export function ExplainToggle() {
  const { mode, setMode } = useExplain();
  return (
    <div className="seg mode-toggle" role="group" aria-label="Explanation detail">
      <button
        aria-pressed={mode === "easy"}
        onClick={() => setMode("easy")}
        title="Plain language. Identifiers and raw bytes are tucked away."
      >
        Easy
      </button>
      <button
        aria-pressed={mode === "advanced"}
        onClick={() => setMode("advanced")}
        title="Full technical detail: identifiers, raw bytes, decoder provenance."
      >
        Advanced
      </button>
    </div>
  );
}

/**
 * An explanation rendered in place.
 *
 * Renders nothing when this build has no entry for the id — that is the honest
 * outcome, and it keeps a missing explanation visible as an absence rather than
 * as filler text.
 */
export function Explain({
  kind,
  id,
  className,
  as = "div",
}: {
  kind: ExplainKind;
  id: string;
  className?: string;
  as?: "div" | "span";
}) {
  const { text } = useExplain();
  const t = text(kind, id);
  if (!t) return null;
  const Tag = as;
  return <Tag className={`explain${className ? ` ${className}` : ""}`}>{t}</Tag>;
}

/**
 * A word with its explanation available on hover and, in Easy mode, spelled out
 * underneath rather than hidden behind an interaction a novice will not try.
 */
export function Term({
  kind,
  id,
  children,
}: {
  kind: ExplainKind;
  id: string;
  children: ReactNode;
}) {
  const { text } = useExplain();
  const t = text(kind, id);
  if (!t) return <>{children}</>;
  return (
    <span className="term" title={t} tabIndex={0}>
      {children}
    </span>
  );
}

/**
 * The strip at the top of a pane saying what this screen is for.
 *
 * Only in Easy mode: in Advanced it is a line of text between the reader and
 * the instrument they came for.
 */
export function PaneIntro({ kind, id }: { kind: ExplainKind; id: string }) {
  const { easy, text } = useExplain();
  const t = text(kind, id);
  if (!easy || !t) return null;
  return (
    <div className="pane-intro">
      <span className="pane-intro-mark" aria-hidden="true" />
      <span>{t}</span>
    </div>
  );
}
