// Changing a setting on the vehicle, from the preview to the read-back.
//
// # Why this exists
//
// Until this, no screen in the application could make a change. The Settings
// screen could preview one — every check, and later the bytes — and stopped
// there. The assistant could preview one and was told, correctly, that it may
// not apply it. So the two writes this app has made to a real truck, AutoLock
// and the double horn chirp, were both made by typing requests at the API by
// hand, and an owner who asked to "try it myself from the UI" could not.
//
// One flow, used in both places: under an assistant reply that previewed a
// change, and on a feature's card in Settings. Two copies of the path that
// writes to somebody's vehicle would be two places for the safety of it to
// drift apart.
//
// # What the person sees before anything is sent
//
// The record as it is and as it would be, with the byte that moves picked out;
// every check, failures first; and a confirmation that says in one sentence
// exactly what will be sent. Nothing is written until the word is typed.
//
// # The write gate, done for them
//
// A verified mapping on a module nobody has asked about writes is blocked by
// one thing that is the application's own homework, not the person's: asking
// the module whether it accepts writes, which is a request to an identifier the
// module has just said it does not have, so nothing can land. The core marks a
// plan where that is the only blocker, and only then does this offer to ask and
// then change under the one confirmation — and the confirmation says both.
//
// # What counts as done
//
// What the module reports afterwards, read back, and nothing less. A positive
// response is not a change; a read-back that matches is a change *to the
// module*; whether the vehicle behaves differently is only known after the key
// has been cycled and somebody has looked, and the result says so.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { ApplyResult, ChangePlan, ToolResult } from "../api/types";
import { ErrorBanner, FailedResult, Spinner, Warnings } from "./primitives";

const PHRASE = "CHANGE";

type Step =
  | { kind: "idle" }
  | { kind: "previewing" }
  | { kind: "planned" }
  | { kind: "confirming" }
  | { kind: "working"; label: string }
  | { kind: "done"; result: ToolResult<ApplyResult> }
  | { kind: "stopped"; result: ToolResult<unknown>; why: string };

export function ChangeFlow({
  featureId,
  desired,
  autoPreview = false,
  onChanged,
}: {
  featureId: string;
  desired: "on" | "off";
  /** Preview as soon as this appears. True under an assistant reply, where the
   *  person asked for the change in words and should not have to ask again. */
  autoPreview?: boolean;
  /** Called after a write that the module read back, so a list can refresh. */
  onChanged?: () => void;
}) {
  const [plan, setPlan] = useState<ToolResult<ChangePlan> | null>(null);
  const [step, setStep] = useState<Step>({ kind: "idle" });
  const [typed, setTyped] = useState("");
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  const preview = useCallback(async () => {
    setStep({ kind: "previewing" });
    setError(null);
    try {
      setPlan(await api.previewFeature(featureId, desired));
      setStep({ kind: "planned" });
    } catch (e) {
      setError(describeError(e));
      setStep({ kind: "idle" });
    }
  }, [featureId, desired]);

  useEffect(() => {
    if (autoPreview) void preview();
  }, [autoPreview, preview]);

  const p = plan?.data ?? null;
  const armed = typed.trim().toUpperCase() === PHRASE;
  const alreadySet = !!p?.bytes?.already_as_asked;
  const offerable = !!p && !alreadySet && (p.can_apply || !!p.needs_write_gate_probe);

  async function go() {
    if (!p || !armed) return;
    // Recorded verbatim in the audit trail, so it says who authorised what
    // rather than merely that something did.
    const confirmation = `user typed ${PHRASE} in the desktop app to turn ${featureId} ${desired}`;
    setError(null);
    try {
      if (!p.can_apply && p.needs_write_gate_probe) {
        setStep({ kind: "working", label: "Asking the module whether it accepts changes" });
        const gate = await api.probeFeatureGate(featureId, confirmation);
        if (!gate.success || !gate.data?.writes_open_without_security) {
          setStep({
            kind: "stopped",
            result: gate,
            why:
              gate.data?.verdict ??
              "The module did not say it accepts changes, so nothing was written.",
          });
          return;
        }
        // The plan again, from the vehicle, now that the module has answered.
        // Written against what it says now, never against what it said before.
        setStep({ kind: "working", label: "Checking again" });
        const fresh = await api.previewFeature(featureId, desired);
        setPlan(fresh);
        if (!fresh.data?.can_apply) {
          setTyped("");
          setStep({ kind: "planned" });
          return;
        }
      }
      setStep({ kind: "working", label: "Writing, then reading it back" });
      const result = await api.applyFeature(featureId, desired, confirmation);
      setTyped("");
      setStep({ kind: "done", result });
      if (result.success && result.data?.changed) onChanged?.();
    } catch (e) {
      setError(describeError(e));
      setStep({ kind: "planned" });
    }
  }

  return (
    <div className="change-flow">
      {step.kind === "idle" && !autoPreview && (
        <button className="mini" onClick={() => void preview()}>
          {desired === "on" ? "What would turning it on involve?" : "Turning it off?"}
        </button>
      )}

      {step.kind === "previewing" && <Spinner label="Reading the setting from the vehicle" />}

      {plan && !plan.success && step.kind !== "previewing" && <FailedResult result={plan} />}

      {p && step.kind !== "previewing" && step.kind !== "done" && (
        <>
          {p.bytes && <BytesToChange bytes={p.bytes} />}
          <PlanChecks plan={p} />
        </>
      )}

      {step.kind === "planned" && p && (
        <div className="row" style={{ marginTop: 10, gap: 8 }}>
          {offerable && (
            <button className="primary" onClick={() => setStep({ kind: "confirming" })}>
              {desired === "on" ? "Turn it on" : "Turn it off"}
            </button>
          )}
          {!offerable && !alreadySet && (
            // The blockers above are mostly things a person fixes in the
            // driveway: the engine, a charger, a scan. Checking again after
            // doing it should not mean asking the assistant again.
            <button onClick={() => void preview()}>Check again</button>
          )}
          {alreadySet && (
            <span className="explain">Already {desired}. There is nothing to change.</span>
          )}
        </div>
      )}

      {step.kind === "confirming" && p && (
        <div className="change-confirm">
          <div>
            <strong>{p.feature_name ?? featureId}</strong>: turn it {desired}.
          </div>
          {/* Said here, in the one place a person reads before typing, and not
              only in the detail of a passing check folded away above. */}
          {p.experiment && (
            <div className="banner caution" style={{ margin: "8px 0 0" }}>
              <span className="b-code">an experiment</span>
              <span>
                Where this setting lives has not been verified on a vehicle. The write will read back
                either way; only the vehicle can say whether these are the right bytes. Cycle the key
                and look — if nothing changed, the mapping is wrong, and turning it back puts the
                original byte back.
              </span>
            </div>
          )}
          <p className="explain" style={{ margin: "6px 0 8px" }}>
            {p.needs_write_gate_probe && !p.can_apply && (
              <>
                First the app asks the module whether it accepts changes. That request is aimed at
                something the module does not have, so nothing lands. If it says yes,{" "}
              </>
            )}
            {p.bytes ? (
              <>
                {p.needs_write_gate_probe && !p.can_apply ? "byte" : "Byte"} {p.bytes.byte_index} of
                record <span className="mono">{p.bytes.identifier}</span> is written from{" "}
                <span className="mono">{p.bytes.byte_before}</span> to{" "}
                <span className="mono">{p.bytes.byte_after}</span>, and the record is read straight
                back to check the module kept it.
              </>
            ) : (
              <>the setting is written and read straight back to check the module kept it.</>
            )}
          </p>
          <label htmlFor={`confirm-${featureId}`} className="faint" style={{ fontSize: 12 }}>
            Type {PHRASE} to confirm
          </label>
          <div className="row" style={{ gap: 8, marginTop: 4 }}>
            <input
              id={`confirm-${featureId}`}
              type="text"
              value={typed}
              autoFocus
              spellCheck={false}
              autoComplete="off"
              placeholder={PHRASE}
              onChange={(e) => setTyped(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && armed) void go(); }}
              style={{ width: 140 }}
            />
            <button className="primary" disabled={!armed} onClick={() => void go()}>
              Change it
            </button>
            <button onClick={() => { setTyped(""); setStep({ kind: "planned" }); }}>Cancel</button>
          </div>
        </div>
      )}

      {step.kind === "working" && (
        <div style={{ marginTop: 10 }}><Spinner label={step.label} /></div>
      )}

      {step.kind === "stopped" && (
        <div className="banner serious" style={{ marginTop: 10 }}>
          <span className="b-code">nothing written</span>
          <span>{step.why}</span>
        </div>
      )}

      {step.kind === "done" && <Outcome result={step.result} desired={desired} />}

      <ErrorBanner error={error} />
    </div>
  );
}

/** What the module said afterwards, and what that does and does not prove. */
function Outcome({ result, desired }: { result: ToolResult<ApplyResult>; desired: "on" | "off" }) {
  if (!result.success) return <FailedResult result={result} />;
  const r = result.data;
  if (r && !r.changed) {
    return (
      <div className="banner info" style={{ marginTop: 10 }}>
        <span className="b-code">already set</span>
        <span>The module already had it {desired}. Nothing was written.</span>
      </div>
    );
  }
  return (
    <div style={{ marginTop: 10 }}>
      <div className="banner ok">
        <span className="b-code">{r?.verified ? "written and read back" : "written"}</span>
        <span>
          {r?.verified
            ? `The module now holds the new value — it read back exactly as written.`
            : `The module accepted the write.`}
          {r?.cycle_ignition_to_apply !== false && (
            <>
              {" "}
              <strong>Turn the key off, wait a few seconds, and turn it back on</strong> before
              checking. Most modules only act on a new setting when they power up, so until then the
              vehicle may still behave the old way.
            </>
          )}
        </span>
      </div>
      {r?.before && r?.after && (
        <div className="provenance" style={{ marginTop: 6 }}>
          <span>{r.did}</span>
          <span className="mono">{r.before} → {r.after}</span>
          {r.module && <span>module {r.module}</span>}
        </div>
      )}
      <Warnings warnings={result.warnings.filter((w) => w.code !== "cycle_the_key_to_see_it")} />
    </div>
  );
}

/**
 * Every check and its answer, the ones that matter first.
 *
 * Failures split into two kinds and they read very differently: something the
 * user could fix (a better adapter, the key on, a charger on the battery), and
 * something this build will never do. Presenting both as "unavailable" would
 * send someone shopping for a cable that will not help.
 *
 * The passing checks are folded away. There are sixteen of them, and a column
 * of green ticks above the one line that says "turn the engine off" is how
 * that line gets missed.
 */
export function PlanChecks({ plan }: { plan: ChangePlan }) {
  const failed = plan.checks.filter((c) => !c.passed);
  const passed = plan.checks.filter((c) => c.passed);
  // The gate is not a blocker the person has to act on when the app is about
  // to deal with it for them, and listing it as one would contradict the
  // button beneath it.
  const shownFailed = plan.needs_write_gate_probe
    ? failed.filter((c) => c.id !== "mapping_known")
    : failed;

  return (
    <div style={{ marginTop: 12 }}>
      {shownFailed.length > 0 && (
        <table>
          <tbody>
            {shownFailed.map((c) => (
              <tr key={c.id}>
                <td style={{ width: 26 }}>
                  <span style={{ color: c.blocking_by_design ? "var(--serious)" : "var(--caution)" }}>
                    {c.blocking_by_design ? "✕" : "!"}
                  </span>
                </td>
                <td>
                  <div>{c.question}</div>
                  {c.detail && <div className="explain" style={{ marginTop: 2 }}>{c.detail}</div>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <details style={{ marginTop: 6 }}>
        <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
          {passed.length} check{passed.length === 1 ? "" : "s"} passed
          {plan.needs_write_gate_probe && " — and one the app will do first"}
        </summary>
        <table>
          <tbody>
            {passed.map((c) => (
              <tr key={c.id}>
                <td style={{ width: 26 }}><span style={{ color: "var(--ok)" }}>✓</span></td>
                <td>
                  <div>{c.question}</div>
                  {c.detail && <div className="explain" style={{ marginTop: 2 }}>{c.detail}</div>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>
    </div>
  );
}

/** The record as it is and as it would be, with the one byte that moves marked.
 *
 * Read off the vehicle during the preview, not reconstructed from the
 * catalogue: the whole value of showing it is that it would look wrong if the
 * mapping were aimed at the wrong byte, and a rendering of what the catalogue
 * *says* would look right either way.
 *
 * The pair of full records is here deliberately, next to the single byte. The
 * byte is what somebody checks; the records are what they compare against a
 * backup afterwards, and that comparison is the only way to prove a write did
 * nothing it was not asked to do. */
export function BytesToChange({ bytes }: { bytes: NonNullable<ChangePlan["bytes"]> }) {
  return (
    <div style={{ marginTop: 10 }}>
      {bytes.already_as_asked ? (
        <p className="explain" style={{ margin: 0 }}>
          Nothing would change: this setting is already what you are asking for.
        </p>
      ) : (
        <p className="explain" style={{ margin: 0 }}>
          One byte, in record <span className="mono">{bytes.identifier}</span>: byte{" "}
          {bytes.byte_index} goes from <span className="mono">{bytes.byte_before}</span> to{" "}
          <span className="mono">{bytes.byte_after}</span>. Everything else in the record is written
          back exactly as it was read.
        </p>
      )}
      <div style={{ overflowX: "auto", marginTop: 6 }}>
        <table>
          <tbody>
            <tr>
              <td className="faint" style={{ width: 60 }}>now</td>
              <td><RecordBytes hex={bytes.before} mark={bytes.byte_index} /></td>
            </tr>
            <tr>
              <td className="faint">after</td>
              <td><RecordBytes hex={bytes.after} mark={bytes.byte_index} /></td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** One record, split into bytes, with the one that moves picked out.
 *
 * Split because nobody can count to byte six along a twenty-character string,
 * and being able to is the entire point of showing it. */
function RecordBytes({ hex, mark }: { hex: string; mark: number }) {
  const bytes = hex.replace(/\s+/g, "").match(/.{1,2}/g) ?? [];
  return (
    <span className="mono" style={{ letterSpacing: "0.04em" }}>
      {bytes.map((b, i) => (
        <span
          key={i}
          style={
            i === mark
              ? { color: "var(--accent)", fontWeight: 600, textDecoration: "underline" }
              : undefined
          }
        >
          {b}
          {i < bytes.length - 1 ? " " : ""}
        </span>
      ))}
    </span>
  );
}
