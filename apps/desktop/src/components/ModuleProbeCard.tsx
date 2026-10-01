// What one module answers: its sessions, whether it has security access, and
// which standard identifiers it holds.
//
// The core has had this probe since before the second manufacturer work, and
// only the assistant and the API could run it. That mattered more once the
// probe started skipping ranges on purpose: a Honda's modules are not asked
// for Ford's configuration range, and the probe says so in a sentence. Nobody
// using the app could read that sentence, so an empty result looked the same
// whether a range was asked and empty or never asked.
//
// Run on a button, never on its own. It reads only, but it opens and closes
// diagnostic sessions and asks for a security seed, and that is a different
// thing from reading a value somebody can see on screen.

import { useEffect, useRef, useState } from "react";
import { api, describeError } from "../api/client";
import type { ModuleProbeData, ToolResult } from "../api/types";
import { ErrorBanner, Spinner, Warnings } from "./primitives";

export function ModuleProbeCard({ moduleKey }: { moduleKey: string | null }) {
  const [result, setResult] = useState<ToolResult<ModuleProbeData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  // A result belongs to the module it was asked of.
  const current = useRef(moduleKey);
  useEffect(() => {
    current.current = moduleKey;
    setResult(null);
    setError(null);
  }, [moduleKey]);

  if (!moduleKey) return null;

  async function probe(key: string) {
    setBusy(true);
    setError(null);
    try {
      const r = await api.probeModule(key);
      // Dropped if somebody picked another module while it ran.
      if (current.current === key) setResult(r);
    } catch (e) {
      if (current.current === key) setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const data = result?.success ? result.data : null;

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div>
          <strong>What this module answers</strong>
          <div className="faint">
            Which diagnostic sessions it grants, whether it has security access, and which
            standard identifiers it holds. Reads only: nothing is written.
          </div>
        </div>
        <button onClick={() => void probe(moduleKey)} disabled={busy}>
          {busy ? <Spinner label="Probing" /> : result ? "Probe again" : "Probe"}
        </button>
      </div>

      <ErrorBanner error={error} />

      {result && !result.success && (
        <div className="banner caution" style={{ marginTop: 10, marginBottom: 0 }}>
          <span className="b-code">{result.error?.code ?? "unavailable"}</span>
          <span>{result.error?.message ?? "This module did not answer."}</span>
        </div>
      )}

      {data && (
        <div style={{ marginTop: 10 }}>
          <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
            {data.sessions.map((s) => (
              <span
                key={s.session}
                className="tag"
                title={s.detail ?? undefined}
                style={{ color: s.granted ? "var(--ok)" : "var(--text-faint)" }}
              >
                {s.session} session {s.granted ? "granted" : "not granted"}
              </span>
            ))}
            {data.security && (
              <span
                className="tag"
                title={[data.security.detail, data.security.note].filter(Boolean).join(" ")}
              >
                security access {data.security.implements_security_access ? "offered" : "not offered"}
              </span>
            )}
          </div>

          <table style={{ marginTop: 10 }}>
            <thead>
              <tr>
                <th>Range</th>
                <th>For</th>
                <th>Found</th>
              </tr>
            </thead>
            <tbody>
              {data.ranges_probed.map((r) => (
                <tr key={r.from}>
                  <td className="mono" style={{ whiteSpace: "nowrap" }}>
                    {r.from}–{r.to}
                  </td>
                  <td>{r.purpose}</td>
                  <td>
                    {r.skipped_because ? (
                      <span className="faint">not asked: {r.skipped_because}</span>
                    ) : r.stopped_early_at ? (
                      <>
                        {r.found}{" "}
                        <span className="faint">(stopped at {r.stopped_early_at}: it went quiet)</span>
                      </>
                    ) : (
                      r.found
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

          {data.identifiers.length > 0 && (
            <table style={{ marginTop: 10 }}>
              <thead>
                <tr>
                  <th>Identifier</th>
                  <th>Value</th>
                </tr>
              </thead>
              <tbody>
                {data.identifiers.map((i) => (
                  <tr key={i.did}>
                    <td className="mono">{i.did}</td>
                    <td className="mono" style={{ wordBreak: "break-all" }}>
                      {i.text ?? i.bytes}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          <Warnings warnings={result!.warnings} />
        </div>
      )}
    </div>
  );
}
