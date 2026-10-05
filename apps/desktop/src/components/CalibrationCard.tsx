// Which software a module runs, and whether a file on this computer is that
// software.
//
// Two buttons, in the order the question is asked. "Read software identity"
// asks the module, and only reads. "Find calibration file" looks through a
// folder on this computer and judges what is there against what the module
// said; it does not touch the vehicle and it does not fetch anything.
//
// The answer to the second is usually "none found", and that is shown as an
// answer. Calibration files are the manufacturer's and mostly reach nobody
// outside a dealership, so a screen that treated their absence as a failure
// would be failing on nearly every car.
//
// Nothing on this card can write to a module. There is no button for it and
// no route behind one.

import { useEffect, useRef, useState } from "react";
import {
  api,
  describeError,
  type CalibrationFound,
  type CalibrationIdentity,
  type Evaluated,
} from "../api/client";
import type { ToolResult } from "../api/types";
import { ErrorBanner, Spinner, Warnings } from "./primitives";

/** Field names as a person reads them. The core's names, said plainly. */
const LABELS: Record<string, string> = {
  vin: "VIN",
  manufacturer: "Manufacturer",
  make: "Make",
  model: "Model",
  model_year: "Model year",
  engine: "Engine",
  transmission: "Transmission",
  module_name: "Module",
  hardware_number: "Hardware number",
  hardware_version: "Hardware version",
  part_number: "Part number",
  software_number: "Software number",
  software_version: "Software version",
  calibration_id: "Calibration ID",
  calibration_verification_number: "Calibration verification number",
  program_id: "Program ID",
  strategy_id: "Strategy ID",
  rom_id: "ROM ID",
  boot_software_id: "Boot software ID",
  application_data_id: "Application data ID",
  supplier: "Supplier",
};

const VEHICLE_FIELDS = ["vin", "make", "model", "model_year", "engine", "transmission"];

function label(field: string): string {
  return LABELS[field] ?? field;
}

/** How an identifier was asked for, in words. */
function how(source: { kind: string; info_type?: number; did?: number; name?: string }): string {
  switch (source.kind) {
    case "obd_info_type":
      return `OBD-II service 09, type ${(source.info_type ?? 0).toString(16).toUpperCase().padStart(2, "0")}`;
    case "uds_did":
      return `UDS identifier ${(source.did ?? 0).toString(16).toUpperCase().padStart(4, "0")}`;
    case "vin_structure":
      return "from the VIN";
    case "lookup":
      return `looked up: ${source.name ?? "an outside source"}`;
    default:
      return source.kind;
  }
}

const STATUS_WORDS: Record<string, { text: string; color: string }> = {
  EXACT_MATCH: { text: "Exact match", color: "var(--ok)" },
  PARTIAL_MATCH: { text: "Partial match", color: "var(--warn, #d6a100)" },
  NO_MATCH: { text: "Not this module's", color: "var(--bad, #d9534f)" },
  UNKNOWN: { text: "Cannot tell", color: "var(--text-faint)" },
};

const VALIDATION_WORDS: Record<string, string> = {
  VALID: "File intact: matches the hash its source stated",
  INVALID: "File NOT valid",
  PARTIALLY_VALIDATED: "File unvouched for: nobody stated what its hash should be",
  UNKNOWN: "Not a kind of file this app treats as a calibration",
};

const VERDICT_WORDS: Record<string, string> = {
  confirmed: "YES",
  conflict: "CONFLICT",
  name_agrees: "name only",
  name_differs: "name differs",
  unknown: "UNKNOWN",
};

export function CalibrationCard({
  moduleKey,
  onEvidence,
}: {
  moduleKey: string | null;
  onEvidence: (ref: number) => void;
}) {
  const [result, setResult] = useState<ToolResult<{ identity: CalibrationIdentity }> | null>(null);
  const [found, setFound] = useState<CalibrationFound | null>(null);
  const [busy, setBusy] = useState<"reading" | "finding" | null>(null);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  // What is shown belongs to the module it was asked of.
  const current = useRef(moduleKey);
  useEffect(() => {
    current.current = moduleKey;
    setResult(null);
    setFound(null);
    setError(null);
  }, [moduleKey]);

  if (!moduleKey) return null;

  async function read(key: string) {
    setBusy("reading");
    setError(null);
    setFound(null);
    try {
      const r = await api.calibrationIdentity(key);
      if (current.current === key) setResult(r);
    } catch (e) {
      if (current.current === key) setError(describeError(e));
    } finally {
      setBusy(null);
    }
  }

  async function find(identity: CalibrationIdentity) {
    setBusy("finding");
    setError(null);
    try {
      const r = await api.findCalibration(identity);
      if (current.current === identity.module_key) setFound(r);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  }

  const identity = result?.success ? (result.data?.identity ?? null) : null;
  const fields = identity ? Object.entries(identity.fields) : [];
  const aboutVehicle = fields.filter(([f]) => VEHICLE_FIELDS.includes(f));
  const aboutModule = fields.filter(([f]) => !VEHICLE_FIELDS.includes(f));
  const namesSoftware = aboutModule.some(([f]) =>
    ["calibration_id", "program_id", "software_number", "rom_id", "strategy_id"].includes(f),
  );

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <div>
          <strong>Software on this module</strong>
          <div className="faint">
            Which calibration it says it runs, and whether a file on this computer is that
            calibration. Reads only: this app never writes software to a vehicle.
          </div>
        </div>
        <button onClick={() => void read(moduleKey)} disabled={busy !== null}>
          {busy === "reading" ? (
            <Spinner label="Reading" />
          ) : result ? (
            "Read again"
          ) : (
            "Read software identity"
          )}
        </button>
      </div>

      <ErrorBanner error={error} />

      {result && !result.success && (
        <div className="banner caution" style={{ marginTop: 10, marginBottom: 0 }}>
          <span className="b-code">{result.error?.code ?? "unavailable"}</span>
          <span>{result.error?.message ?? "This module did not answer."}</span>
        </div>
      )}

      {result && <Warnings warnings={result.warnings} />}

      {identity && (
        <div style={{ marginTop: 10 }}>
          <IdentityTable title="Vehicle" rows={aboutVehicle} onEvidence={onEvidence} />
          <IdentityTable
            title={`Module ${identity.module_key} at ${identity.address}`}
            rows={aboutModule}
            onEvidence={onEvidence}
            empty="This module reported nothing about itself."
          />

          {identity.unanswered.length > 0 && (
            <details style={{ marginTop: 8 }}>
              <summary className="faint" style={{ cursor: "pointer" }}>
                {identity.unanswered.length} identifiers asked for and not given
              </summary>
              <table style={{ marginTop: 6 }}>
                <thead>
                  <tr><th>Asked for</th><th>How</th><th>What happened</th><th>Reply</th></tr>
                </thead>
                <tbody>
                  {identity.unanswered.map((u, i) => (
                    <tr key={i}>
                      <td>{u.field ? label(u.field) : <span className="faint">-</span>}</td>
                      <td className="faint">{how(u.source)}</td>
                      <td>{u.reason}</td>
                      <td className="mono faint">
                        {u.raw_hex ?? ""}
                        {u.evidence_ref != null && (
                          <button
                            className="ev"
                            style={{ marginLeft: 6 }}
                            onClick={() => onEvidence(u.evidence_ref as number)}
                          >
                            evidence #{u.evidence_ref}
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </details>
          )}

          <div className="row" style={{ gap: 10, marginTop: 12, alignItems: "center" }}>
            <button onClick={() => void find(identity)} disabled={busy !== null}>
              {busy === "finding" ? <Spinner label="Looking" /> : "Find calibration file"}
            </button>
            <span className="faint" style={{ fontSize: 12 }}>
              {namesSoftware
                ? "Looks in a folder on this computer. Nothing is downloaded."
                : "This module named no software, so no file can be matched exactly."}
            </span>
          </div>
        </div>
      )}

      {found && <Found found={found} />}
    </div>
  );
}

function IdentityTable({
  title,
  rows,
  onEvidence,
  empty,
}: {
  title: string;
  rows: [string, CalibrationIdentity["fields"][string]][];
  onEvidence: (ref: number) => void;
  empty?: string;
}) {
  if (!rows.length && !empty) return null;
  return (
    <div style={{ marginTop: 8 }}>
      <div className="faint" style={{ marginBottom: 4 }}>{title}</div>
      {!rows.length ? (
        <div className="faint">{empty}</div>
      ) : (
        <table>
          <thead>
            <tr><th>What</th><th>Value</th><th>Where it came from</th></tr>
          </thead>
          <tbody>
            {rows.flatMap(([field, values]) =>
              values.map((v, i) => (
                <tr key={`${field}-${i}`}>
                  <td>{label(field)}</td>
                  <td className="mono" style={{ fontWeight: 600 }}>{v.value}</td>
                  <td className="faint">
                    {how(v.source)}
                    {v.evidence_ref != null && (
                      <button
                        className="ev"
                        style={{ marginLeft: 6 }}
                        title={v.raw_hex ? `raw: ${v.raw_hex}` : undefined}
                        onClick={() => onEvidence(v.evidence_ref as number)}
                      >
                        evidence #{v.evidence_ref}
                      </button>
                    )}
                  </td>
                </tr>
              )),
            )}
          </tbody>
        </table>
      )}
    </div>
  );
}

function Found({ found }: { found: CalibrationFound }) {
  const r = found.resolution;
  const looked = r.sources
    .map((s) =>
      s.searched
        ? `${s.source.name.toLowerCase()} (${s.offered} ${s.offered === 1 ? "file" : "files"}${s.error ? `, could not be read: ${s.error}` : ""})`
        : `${s.source.name.toLowerCase()} (switched off)`,
    )
    .join("; ");

  return (
    <div style={{ marginTop: 12 }}>
      {r.outcome === "NO_ARTIFACT_FOUND" && (
        <div className="banner info" style={{ marginBottom: 8 }}>
          <span className="b-code">none found</span>
          <span>
            No file here is this module's calibration. That is the usual answer, not a fault:
            calibration files are the manufacturer's, and this app does not download them. If
            you have one you are entitled to, put it in{" "}
            <span className="mono">{found.folder}</span> and look again. The README there says
            how to state what a file is.
          </span>
        </div>
      )}
      <div className="faint" style={{ fontSize: 12 }}>Looked in: {looked}.</div>

      {r.matches.map((e) => <Artifact key={e.artifact.sha256} e={e} />)}

      {r.set_aside.length > 0 && (
        <details style={{ marginTop: 8 }}>
          <summary className="faint" style={{ cursor: "pointer" }}>
            {r.set_aside.length} other {r.set_aside.length === 1 ? "file" : "files"} looked at and
            set aside
          </summary>
          {r.set_aside.map((e) => <Artifact key={e.artifact.sha256} e={e} />)}
        </details>
      )}
    </div>
  );
}

function Artifact({ e }: { e: Evaluated }) {
  const status = STATUS_WORDS[e.matching.status] ?? STATUS_WORDS.UNKNOWN;
  return (
    <div className="card" style={{ marginTop: 8 }}>
      <div className="row" style={{ gap: 10, flexWrap: "wrap", alignItems: "baseline" }}>
        <span className="tag" style={{ color: status.color, fontWeight: 600 }}>{status.text}</span>
        <strong className="mono">{e.artifact.filename}</strong>
        <span className="faint">{e.artifact.format.toUpperCase()} · {e.artifact.size.toLocaleString()} bytes</span>
      </div>
      <div style={{ marginTop: 6 }}>{e.matching.reason}</div>

      <table style={{ marginTop: 8 }}>
        <thead>
          <tr><th>Evidence</th><th>Module reports</th><th>File is said to be</th><th></th></tr>
        </thead>
        <tbody>
          {e.matching.checks.map((c) => (
            <tr key={c.field}>
              <td>{label(c.field)}</td>
              <td className="mono">{c.reported.join(", ") || <span className="faint">not reported</span>}</td>
              <td className="mono">
                {c.claimed.length
                  ? c.claimed.map(([value, basis]) => `${value}${basis === "filename" ? " (its name)" : ""}`).join(", ")
                  : <span className="faint">not stated</span>}
              </td>
              <td title={c.note} style={{ fontWeight: 600, whiteSpace: "nowrap" }}>
                {VERDICT_WORDS[c.verdict] ?? c.verdict}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <div style={{ marginTop: 8 }}>
        <span className="tag">{VALIDATION_WORDS[e.validation.status] ?? e.validation.status}</span>
        {e.validation.checks
          .filter((c) => c.passed === false)
          .map((c) => (
            <div key={c.what} className="faint" style={{ marginTop: 4 }}>{c.what}: {c.detail}</div>
          ))}
      </div>
      <div className="faint mono" style={{ fontSize: 11, marginTop: 6, wordBreak: "break-all" }}>
        SHA-256 {e.artifact.sha256}
      </div>
      <div className="faint" style={{ fontSize: 12, marginTop: 2 }}>
        From: {e.artifact.sources.map((s) => `${s.source} (${s.reference})`).join("; ")}
        {e.cached ? " · kept by this app" : ""}
      </div>
    </div>
  );
}
