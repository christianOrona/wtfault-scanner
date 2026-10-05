// Which software a module runs, and whether a file on this computer is that
// software.
//
// Two buttons, in the order the question is asked. "Read software identity"
// asks the module, and only reads. "Find calibration file" looks through the
// calibration files on this computer and judges what is there against what
// the module said; it does not touch the vehicle and it does not fetch
// anything. "Add a file" copies a file the person has into their folder.
//
// The answer to the second is usually "none found", and that is shown as an
// answer. Calibration files are the manufacturer's and mostly reach nobody
// outside a dealership, so a screen that treated their absence as a failure
// would be failing on nearly every car.
//
// Three things are kept apart on every result, because running them together
// is how a file gets believed for the wrong reason: whether it matches the
// module, whose word that match is, and who made the file. The last has one
// answer here, "not established", and it is shown on every file.
//
// Nothing on this card can write to a module. There is no button for it and
// no route behind one.

import { useEffect, useRef, useState } from "react";
import {
  api,
  describeError,
  type Availability,
  type Basis,
  type CalibrationFound,
  type CalibrationIdentity,
  type CalibrationRead,
  type Evaluated,
  type NotGiven,
  type SourceKind,
  type SourceStatus,
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

/** Why an identifier was not given. Different facts, so different words. */
const NOT_GIVEN_WORDS: Record<NotGiven, string> = {
  not_supported: "Not supported",
  refused: "Refused",
  no_answer: "No answer",
  unreadable: "Could not be read",
  empty: "Empty answer",
  not_asked: "Not asked",
  unspecified: "Not given",
};

/** Where a field stands, in the order worth reading. */
const AVAILABILITY_WORDS: [Availability, string][] = [
  ["NOT_SUPPORTED", "The module says it has none"],
  ["REFUSED", "The module refused"],
  ["READ_FAILED", "Asked for, and the reading failed"],
  ["NOT_READ", "Not read: nothing in this app asks for it"],
];

const STATUS_WORDS: Record<string, { text: string; color: string }> = {
  EXACT_MATCH: { text: "Exact match", color: "var(--ok)" },
  PARTIAL_MATCH: { text: "Partial match", color: "var(--warn, #d6a100)" },
  CONFLICTING_EVIDENCE: { text: "Conflicting evidence", color: "var(--caution, #d6a100)" },
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
  conflicting_evidence: "IN DOUBT",
  in_file_header: "in the file",
  name_agrees: "name only",
  name_differs: "name differs",
  unknown: "UNKNOWN",
};

/** Whose word something said about a file is. */
const BASIS_WORDS: Record<Basis, string> = {
  filename: "its name",
  user_declared: "declared by you",
  source_declared: "declared by its source",
  file_header: "in its own header",
};

/** What a match rests on, as the sentence under the status. */
const RESTS_ON_WORDS: Record<Basis, string> = {
  filename: "its file name, which anyone can type",
  user_declared: "what you declared about the file",
  source_declared: "what the file's source declared about it",
  file_header: "an identifier written in the file's own header",
};

const KIND_WORDS: Record<SourceKind, string> = {
  user_folder: "your folder",
  tool_installation: "a service tool's folder",
  kept: "kept by this app",
  other: "another source",
};

const SOURCE_STATUS_WORDS: Record<SourceStatus, string> = {
  matched: "searched",
  no_match: "searched, nothing for this module",
  failed: "COULD NOT BE SEARCHED",
  unavailable: "not there, so not searched",
  not_configured: "not installed, so not searched",
  switched_off: "switched off, so not searched",
};

const CONTENT_WORDS: Record<string, string> = {
  empty: "nothing",
  gzip: "gzip",
  rwd: "a Honda RWD package",
  intel_hex: "Intel HEX",
  s_record: "Motorola S-records",
  opaque: "bytes with no recognisable form",
};

export function CalibrationCard({
  moduleKey,
  onEvidence,
}: {
  moduleKey: string | null;
  onEvidence: (ref: number) => void;
}) {
  const [result, setResult] = useState<ToolResult<CalibrationRead> | null>(null);
  const [found, setFound] = useState<CalibrationFound | null>(null);
  const [added, setAdded] = useState<string | null>(null);
  const [busy, setBusy] = useState<"reading" | "finding" | "adding" | null>(null);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const picker = useRef<HTMLInputElement>(null);

  // What is shown belongs to the module it was asked of.
  const current = useRef(moduleKey);
  useEffect(() => {
    current.current = moduleKey;
    setResult(null);
    setFound(null);
    setAdded(null);
    setError(null);
  }, [moduleKey]);

  if (!moduleKey) return null;

  async function read(key: string) {
    setBusy("reading");
    setError(null);
    setFound(null);
    setAdded(null);
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

  /** Copy a file the person has into their folder, then look again. */
  async function add(file: File, identity: CalibrationIdentity) {
    setBusy("adding");
    setError(null);
    setAdded(null);
    try {
      const a = await api.addCalibrationFile(file);
      setAdded(
        a.already_there
          ? `${a.filename} was already in your folder.`
          : `${a.filename} was copied into your folder. Adding it says nothing about what it is.`,
      );
      const r = await api.findCalibration(identity);
      if (current.current === identity.module_key) setFound(r);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  }

  const identity = result?.success ? (result.data?.identity ?? null) : null;
  const availability = result?.success ? (result.data?.availability ?? {}) : {};
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

          <NotAvailable availability={availability} />

          {identity.unanswered.length > 0 && (
            <details style={{ marginTop: 8 }}>
              <summary className="faint" style={{ cursor: "pointer" }}>
                {identity.unanswered.length} requests that gave nothing, each with what happened
              </summary>
              <table style={{ marginTop: 6 }}>
                <thead>
                  <tr><th>Asked for</th><th>How</th><th>Result</th><th>What happened</th><th>Reply</th></tr>
                </thead>
                <tbody>
                  {identity.unanswered.map((u, i) => (
                    <tr key={i}>
                      <td>{u.field ? label(u.field) : <span className="faint">-</span>}</td>
                      <td className="faint">{how(u.source)}</td>
                      <td style={{ whiteSpace: "nowrap" }}>{NOT_GIVEN_WORDS[u.state] ?? u.state}</td>
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

          <div className="row" style={{ gap: 10, marginTop: 12, alignItems: "center", flexWrap: "wrap" }}>
            <button onClick={() => void find(identity)} disabled={busy !== null}>
              {busy === "finding" ? <Spinner label="Looking" /> : "Find calibration file"}
            </button>
            <input
              ref={picker}
              type="file"
              accept=".rwd,.gz,.bin,.hex,.s19"
              style={{ display: "none" }}
              onChange={(e) => {
                const file = e.target.files?.[0];
                // Cleared so that choosing the same file again still fires.
                e.target.value = "";
                if (file) void add(file, identity);
              }}
            />
            <button onClick={() => picker.current?.click()} disabled={busy !== null}>
              {busy === "adding" ? <Spinner label="Adding" /> : "Add a file you have"}
            </button>
            <span className="faint" style={{ fontSize: 12 }}>
              {namesSoftware
                ? "Looks on this computer only. Nothing is downloaded."
                : "This module named no software, so no file can be matched exactly."}
            </span>
          </div>
          {added && <div className="faint" style={{ marginTop: 6 }}>{added}</div>}
        </div>
      )}

      {found && <Found found={found} />}
    </div>
  );
}

/** What the module did not give, grouped by why. Shown after what it did
 *  give, so the screen leads with what is known. */
function NotAvailable({ availability }: { availability: Record<string, Availability> }) {
  const groups = AVAILABILITY_WORDS.map(([state, words]) => ({
    words,
    fields: Object.entries(availability)
      .filter(([, s]) => s === state)
      .map(([f]) => label(f)),
  })).filter((g) => g.fields.length > 0);
  if (!groups.length) return null;
  return (
    <div style={{ marginTop: 8 }}>
      <div className="faint" style={{ marginBottom: 4 }}>Not known for this module</div>
      {groups.map((g) => (
        <div key={g.words} className="faint" style={{ fontSize: 12 }}>
          <span style={{ color: "var(--text)" }}>{g.words}:</span> {g.fields.join(", ")}
        </div>
      ))}
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
  const inDoubt = r.set_aside.filter((e) => e.matching.status === "CONFLICTING_EVIDENCE");
  const ruledOut = r.set_aside.filter((e) => e.matching.status !== "CONFLICTING_EVIDENCE");

  return (
    <div style={{ marginTop: 12 }}>
      {r.incomplete && (
        <div className="banner caution" style={{ marginBottom: 8 }}>
          <span className="b-code">search incomplete</span>
          <span>
            A place calibration files are kept could not be searched, so a file may be there
            that was not looked at. The list below says which.
          </span>
        </div>
      )}
      {r.outcome === "NO_ARTIFACT_FOUND" && (
        <div className="banner info" style={{ marginBottom: 8 }}>
          <span className="b-code">none found</span>
          <span>
            No file here is this module's calibration. That is the usual answer, not a fault:
            calibration files are the manufacturer's, and this app does not download them. If
            you have one you are entitled to, use <strong>Add a file you have</strong>, or put
            it in <span className="mono">{found.folder}</span>. The README there says how to
            state what a file is.
          </span>
        </div>
      )}
      {r.outcome === "CONFLICTING_EVIDENCE" && (
        <div className="banner caution" style={{ marginBottom: 8 }}>
          <span className="b-code">not judged</span>
          <span>
            A file here may be this module's calibration, and what is said about it disagrees
            with itself, so it is neither accepted nor ruled out. Each piece of evidence is
            listed below.
          </span>
        </div>
      )}

      <table>
        <thead>
          <tr><th>Looked in</th><th>What happened</th></tr>
        </thead>
        <tbody>
          {r.sources.map((s) => (
            <tr key={s.source.id}>
              <td>
                {s.source.name}
                {s.source.location && (
                  <div className="faint mono" style={{ fontSize: 11, wordBreak: "break-all" }}>
                    {s.source.location}
                  </div>
                )}
              </td>
              <td>
                <span style={{ fontWeight: s.status === "failed" ? 600 : 400 }}>
                  {SOURCE_STATUS_WORDS[s.status] ?? s.status}
                </span>
                {s.searched && (
                  <span className="faint">
                    {" "}· {s.offered} {s.offered === 1 ? "file" : "files"} looked at
                    {s.matched > 0 ? `, ${s.matched} matched` : ""}
                    {s.passed_over > 0 ? `, ${s.passed_over} passed over` : ""}
                  </span>
                )}
                {s.error && <div className="faint" style={{ fontSize: 12 }}>{s.error}</div>}
                {s.note && <div className="faint" style={{ fontSize: 12 }}>{s.note}</div>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      {r.matches.map((e) => <Artifact key={e.artifact.sha256} e={e} />)}
      {/* In doubt is not ruled out, so it is not folded away with what is. */}
      {inDoubt.map((e) => <Artifact key={e.artifact.sha256} e={e} />)}

      {ruledOut.length > 0 && (
        <details style={{ marginTop: 8 }}>
          <summary className="faint" style={{ cursor: "pointer" }}>
            {ruledOut.length} other {ruledOut.length === 1 ? "file" : "files"} looked at and
            set aside
          </summary>
          {ruledOut.map((e) => <Artifact key={e.artifact.sha256} e={e} />)}
        </details>
      )}
    </div>
  );
}

/** What a file's name says it is, both layers. */
function namedAs(a: Evaluated["artifact"]): string {
  const inner = a.format === "unknown" ? "" : a.format.toUpperCase();
  if (a.compression === "gzip") return inner ? `gzip holding ${inner}` : "gzip";
  return inner || "unknown kind";
}

function Artifact({ e }: { e: Evaluated }) {
  const status = STATUS_WORDS[e.matching.status] ?? STATUS_WORDS.UNKNOWN;
  const seen = e.inspection;
  const headerTexts = seen.rwd?.headers_read ? seen.rwd.groups.flatMap((g) => g.texts) : [];
  const hidden = seen.rwd?.groups.reduce((n, g) => n + g.not_shown, 0) ?? 0;
  return (
    <div className="card" style={{ marginTop: 8 }}>
      <div className="row" style={{ gap: 10, flexWrap: "wrap", alignItems: "baseline" }}>
        <span className="tag" style={{ color: status.color, fontWeight: 600 }}>{status.text}</span>
        {/* Beside the match, not under it: a match on a file that is not
            what it says must not be read without this. */}
        {e.validation.status === "INVALID" && (
          <span className="tag" style={{ color: "var(--bad, #d9534f)", fontWeight: 600 }}>
            File NOT valid
          </span>
        )}
        <strong className="mono">{e.artifact.filename}</strong>
        <span className="faint">{namedAs(e.artifact)} · {e.artifact.size.toLocaleString()} bytes</span>
      </div>
      <div style={{ marginTop: 6 }}>{e.matching.reason}</div>

      {/* Three answers, never one: the match, whose word it is, who made it. */}
      <table style={{ marginTop: 8 }}>
        <tbody>
          <tr>
            <td className="faint">This match rests on</td>
            <td>
              {e.matching.rests_on
                ? RESTS_ON_WORDS[e.matching.rests_on]
                : "nothing that ties the file to this module's calibration"}
            </td>
          </tr>
          <tr>
            <td className="faint">Made by the manufacturer</td>
            <td>
              <strong>Not established.</strong> <span className="faint">{e.origin.reason}</span>
            </td>
          </tr>
          <tr>
            <td className="faint">Software inside the file</td>
            <td>
              {seen.software === "PAYLOAD_OPAQUE"
                ? "Not read. This app does not open or interpret it."
                : "Not reached: the file's packing did not open."}
            </td>
          </tr>
        </tbody>
      </table>

      {e.matching.warnings.map((w) => (
        <div key={w} className="banner caution" style={{ marginTop: 8, marginBottom: 0 }}>
          <span>{w}</span>
        </div>
      ))}

      <table style={{ marginTop: 8 }}>
        <thead>
          <tr><th>Evidence</th><th>Module reports</th><th>Said of the file, and by whom</th><th></th></tr>
        </thead>
        <tbody>
          {e.matching.checks.map((c) => (
            <tr key={c.field}>
              <td>{label(c.field)}</td>
              <td className="mono">{c.reported.join(", ") || <span className="faint">not reported</span>}</td>
              <td>
                {c.claimed.length
                  ? c.claimed.map(([value, basis], i) => (
                      <div key={`${value}-${basis}-${i}`}>
                        <span className="mono">{value}</span>{" "}
                        <span className="faint">({BASIS_WORDS[basis] ?? basis})</span>
                      </div>
                    ))
                  : <span className="faint">not stated</span>}
              </td>
              <td title={c.note} style={{ fontWeight: 600, whiteSpace: "nowrap" }}>
                {VERDICT_WORDS[c.verdict] ?? c.verdict}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {/* A conflict's own words. Evidence in doubt is already spelled out in
          the reason above. */}
      {e.matching.checks
        .filter((c) => c.verdict === "conflict")
        .map((c) => (
          <div key={c.field} className="faint" style={{ marginTop: 4 }}>{c.note}.</div>
        ))}

      <div style={{ marginTop: 8 }}>
        <span className="tag">{VALIDATION_WORDS[e.validation.status] ?? e.validation.status}</span>
        {e.validation.checks
          .filter((c) => c.passed === false)
          .map((c) => (
            <div key={c.what} className="faint" style={{ marginTop: 4 }}>{c.what}: {c.detail}</div>
          ))}
      </div>

      <details style={{ marginTop: 8 }}>
        <summary className="faint" style={{ cursor: "pointer" }}>What is in the file</summary>
        <div className="faint" style={{ fontSize: 12, marginTop: 4 }}>
          {seen.compression === "gzip"
            ? seen.unpack_problem
              ? `Packed with gzip, and ${seen.unpack_problem}.`
              : `Packed with gzip. Inside: ${CONTENT_WORDS[seen.content] ?? seen.content}${
                  seen.payload_size != null ? `, ${seen.payload_size.toLocaleString()} bytes` : ""
                }.`
            : `Not packed. It is ${CONTENT_WORDS[seen.content] ?? seen.content}.`}
        </div>
        {seen.rwd && (
          <div className="faint" style={{ fontSize: 12, marginTop: 4 }}>
            {seen.rwd.headers_read ? (
              <>
                Header read ({seen.rwd.header_bytes?.toLocaleString()} bytes). Text in it:{" "}
                <span className="mono" style={{ color: "var(--text)" }}>
                  {headerTexts.join(", ") || "none"}
                </span>
                .{hidden > 0 ? ` ${hidden} other values are not text or are not read out.` : ""}{" "}
                What each value means is not described anywhere public, so none is taken as
                the calibration the file contains.
              </>
            ) : (
              <>Header not read: {seen.rwd.problem ?? "it is not shaped as described"}.</>
            )}
          </div>
        )}
        {seen.payload_sha256 && (
          <div className="faint mono" style={{ fontSize: 11, marginTop: 4, wordBreak: "break-all" }}>
            SHA-256 of what is inside the packing {seen.payload_sha256}
          </div>
        )}
      </details>

      <div className="faint mono" style={{ fontSize: 11, marginTop: 6, wordBreak: "break-all" }}>
        SHA-256 {e.artifact.sha256}
      </div>
      <div className="faint" style={{ fontSize: 12, marginTop: 2 }}>
        Found in: {e.artifact.sources
          .map((s) => `${KIND_WORDS[s.kind] ?? s.kind} (${s.reference})`)
          .join("; ")}
        {e.cached ? " · kept by this app" : ""}
      </div>
    </div>
  );
}
