// Clearing the codes, and being honest about what it costs.
//
// This is the one write the app performs, and it is the one every cheap code
// reader performs too — which is exactly why it needs a better dialog than they
// give it. Those tools present it as "erase codes", which sounds like tidying
// up. It is not:
//
//   * The readiness monitors reset. Until the vehicle has been driven 50 to 100
//     miles through the right conditions, it fails an emissions test outright,
//     whatever else is right with it.
//   * The freeze frames go. Those are the snapshots of what the engine was
//     doing when each fault was recorded, and they are usually the most useful
//     thing on the whole scan.
//   * Nothing is repaired. If the fault is still there the code comes back, and
//     the only thing that changed is that the evidence is gone.
//
// So the dialog states all three, requires the word CLEAR to be typed rather
// than a button to be clicked twice, and names who authorised it in the audit
// trail. The agent cannot reach this path at all.

import { useState } from "react";
import { api, describeError } from "../api/client";
import type { Dtc, ToolResult } from "../api/types";
import { ErrorBanner, Spinner } from "./primitives";

const PHRASE = "CLEAR";

export function ClearCodesDialog({
  moduleKey,
  codes,
  onClose,
  onCleared,
}: {
  /** Module to clear, or null for every module that answers. */
  moduleKey: string | null;
  /** What is about to be destroyed, so it can be listed. */
  codes: Dtc[];
  onClose: () => void;
  onCleared: (result: ToolResult<unknown>) => void;
}) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  const armed = typed.trim().toUpperCase() === PHRASE;

  async function clear() {
    if (!armed) return;
    setBusy(true);
    setError(null);
    try {
      // The confirmation string is recorded verbatim in the audit trail, so it
      // says who authorised it rather than merely that something did.
      const r = await api.clearDtcs(`user typed ${PHRASE} in the desktop app`, moduleKey ?? undefined);
      onCleared(r);
      onClose();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-labelledby="clear-title">
      <div className="dialog" style={{ width: "min(560px, 100%)" }}>
        <div className="row" style={{ justifyContent: "space-between" }}>
          <h1 id="clear-title">Clear the stored codes?</h1>
          <button onClick={onClose} disabled={busy}>Cancel</button>
        </div>

        <p className="muted" style={{ marginTop: 0 }}>
          This erases what the vehicle has recorded. It does not repair anything.
        </p>

        <div className="banner serious">
          <span className="b-code">this is not undoable</span>
          <div>
            <strong>Three things go, and one of them matters more than people expect.</strong>
            <ul style={{ margin: "8px 0 0", paddingLeft: 18 }}>
              <li style={{ marginBottom: 6 }}>
                <strong>The self-test results reset.</strong> Until the vehicle has been driven
                properly again — usually 50 to 100 miles of mixed driving — it will fail an
                emissions test on that alone, however healthy it is.
              </li>
              <li style={{ marginBottom: 6 }}>
                <strong>The freeze frames are destroyed.</strong> Those are the snapshots of what
                the engine was doing at the moment each fault was recorded, and they are usually
                the most useful evidence on the whole scan.
              </li>
              <li>
                <strong>Nothing is fixed.</strong> If the fault is still present the code returns
                within a few drives — only now without the history that explained it.
              </li>
            </ul>
          </div>
        </div>

        {codes.length > 0 && (
          <div className="section" style={{ marginTop: 4 }}>
            <h2>About to be erased ({codes.length})</h2>
            <div className="mono" style={{ fontSize: 12, lineHeight: 1.8 }}>
              {codes.map((d) => (
                <span key={`${d.code}-${d.status}`} style={{ marginRight: 12 }}>
                  {d.code}
                  <span className="faint"> {d.status}</span>
                </span>
              ))}
            </div>
          </div>
        )}

        <div className="banner info">
          <span className="b-code">already saved</span>
          <span>
            Everything read this session — the codes, the freeze frames, the raw exchange — stays
            in this app's history under Sessions, whatever the vehicle forgets. Clearing does not
            erase that.
          </span>
        </div>

        <div className="field">
          <label htmlFor="clear-confirm">
            Type {PHRASE} to confirm
          </label>
          <input
            id="clear-confirm"
            type="text"
            value={typed}
            autoFocus
            spellCheck={false}
            autoComplete="off"
            placeholder={PHRASE}
            onChange={(e) => setTyped(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && armed) void clear(); }}
          />
          <span className="faint" style={{ fontSize: 12 }}>
            The engine must be off and the vehicle stationary. The app checks before sending.
          </span>
        </div>

        <ErrorBanner error={error} />

        <div className="row" style={{ justifyContent: "flex-end", marginTop: 4 }}>
          <button onClick={onClose} disabled={busy}>Keep the codes</button>
          <button className="danger" onClick={() => void clear()} disabled={!armed || busy}>
            {busy ? <Spinner label="Clearing" /> : `Clear ${moduleKey ? "this module" : "everything"}`}
          </button>
        </div>
      </div>
    </div>
  );
}
