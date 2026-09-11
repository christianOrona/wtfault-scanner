// The manufacturer's record of how this vehicle was built, and how to get it.
//
// # Why this is on screen unprompted
//
// An as-built file covers the *whole* vehicle. Measured on a 2019 F-250: the
// file held 29 modules while six were awake on the bus. Configuration for a
// module that is asleep, or on a bus this adapter cannot reach, is in the file
// regardless — and no amount of scanning gets at it.
//
// The friction is not the account, which is free. It is that most people have
// never heard of the file. So the app says so, with the VIN ready to paste and
// the download page one click away.
//
// # What this deliberately does not do
//
// Fetch it. Measured 2026-09-10: the as-built endpoint answers 302 to a login
// page, for everybody. An "attempt online first" step would fail for every
// user and cost only latency; getting past it would mean handling somebody's
// manufacturer credentials, which this project does not do, and automated
// retrieval from a login-gated service is against its terms besides.
//
// Notice, guide, import. Not scrape.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, describeError } from "../api/client";
import type { AsBuiltImport, AsBuiltStatus, ToolResult } from "../api/types";
import { ErrorBanner, Spinner } from "./primitives";

/** Where Ford publishes it. Opened by the person, never fetched by the app. */
const MOTORCRAFT = "https://www.motorcraftservice.com/AsBuilt";

export function AsBuiltPanel({ connected }: { connected: boolean }) {
  const [status, setStatus] = useState<ToolResult<AsBuiltStatus> | null>(null);
  const [imported, setImported] = useState<AsBuiltImport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [copied, setCopied] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    try {
      setStatus(await api.asBuiltStatus());
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  useEffect(() => {
    if (connected) void load();
  }, [connected, load]);

  const data = status?.data;
  if (!connected || !data) return null;

  // No VIN, no file to look for. Say which of the two reasons it is — a
  // disagreement between two sources is a different problem from not having
  // read one yet, and the server distinguishes them.
  if (!data.vin) {
    return (
      <div className="banner info">
        <span className="b-code">as-built</span>
        <span>{data.why_not_yet ?? "Identify the vehicle first."}</span>
      </div>
    );
  }

  async function importFile(file: File) {
    setBusy(true);
    setError(null);
    setImported(null);
    try {
      const text = await file.text();
      const r = await api.importAsBuilt(text, file.name);
      if (r.success && r.data) {
        setImported(r.data);
        await load();
      } else if (r.error) {
        // The refusal that matters most is the VIN mismatch, and it arrives
        // here as an ordinary error with the two VINs named in it. Shown
        // verbatim: the server's sentence is the honest one.
        setError({ code: r.error.code, message: r.error.message });
      }
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
      if (fileRef.current) fileRef.current.value = "";
    }
  }

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between", alignItems: "flex-start" }}>
        <div style={{ minWidth: 0 }}>
          <div className="row" style={{ gap: 8 }}>
            <strong>How this vehicle left the factory</strong>
            <span className="tag" style={{ color: data.held ? "var(--ok)" : "var(--caution)" }}>
              {data.held ? "imported" : "not imported"}
            </span>
          </div>
          <div className="explain">{data.what_it_would_add}</div>
        </div>
        {data.held && (
          <button
            className="mini"
            disabled={busy}
            onClick={() => void api.forgetAsBuilt().then(load).catch((e) => setError(describeError(e)))}
          >
            Forget it
          </button>
        )}
      </div>

      <ErrorBanner error={error} />

      {data.held ? (
        <>
          <div className="faint" style={{ marginTop: 8 }}>
            Imported {data.imported_at?.slice(0, 10)}
            {data.source ? ` from ${data.source}` : ""}. It stays on this machine.
          </div>
          <div className="banner caution" style={{ marginTop: 10, marginBottom: 0 }}>
            <span className="b-code">a snapshot</span>
            <span>{data.what_it_is_not}</span>
          </div>
        </>
      ) : (
        <>
          {/* The guide, with the work already done. Copying a VIN off a
              dashboard is the step people give up on. */}
          <ol style={{ margin: "10px 0 0", paddingLeft: 18 }}>
            {(data.how_to_get_one ?? []).map((step, i) => (
              <li key={i} style={{ marginBottom: 4 }}>
                {step}
              </li>
            ))}
          </ol>
          <div className="row" style={{ marginTop: 10, gap: 6 }}>
            <button
              className="mini"
              onClick={() => {
                void navigator.clipboard.writeText(data.vin!).then(() => {
                  setCopied(true);
                  setTimeout(() => setCopied(false), 2000);
                });
              }}
            >
              {copied ? "Copied" : `Copy VIN ${data.vin}`}
            </button>
            <a className="mini button" href={MOTORCRAFT} target="_blank" rel="noreferrer">
              Open the download page
            </a>
          </div>
        </>
      )}

      <div className="row" style={{ marginTop: 10, gap: 6 }}>
        <input
          ref={fileRef}
          type="file"
          accept=".ab,.xml,text/xml"
          style={{ display: "none" }}
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) void importFile(f);
          }}
        />
        <button className="mini" disabled={busy} onClick={() => fileRef.current?.click()}>
          {busy ? <Spinner label="Reading the file" /> : data.held ? "Import a newer file" : "Import a file"}
        </button>
        <span className="faint">
          Checked against your VIN before anything is kept. Never uploaded.
        </span>
      </div>

      {imported && (
        <div className="banner info" style={{ marginTop: 10, marginBottom: 0 }}>
          <span className="b-code">imported</span>
          <span>
            {imported.modules} modules, {imported.lines} configuration lines, for{" "}
            <span className="mono">{imported.vin}</span>.
            {imported.modules_not_answering_on_the_bus > 0 && (
              <>
                {" "}
                <strong>{imported.modules_not_answering_on_the_bus}</strong> of those are not
                answering on the bus right now — asleep, or on a bus this adapter cannot reach.
                Those are the ones you could not have read any other way.
              </>
            )}
          </span>
        </div>
      )}
    </div>
  );
}
