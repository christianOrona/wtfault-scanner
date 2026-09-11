// What this vehicle could be set to do, and exactly how far this app can go.
//
// The honest shape of this screen matters more than usual, because it is the
// one place the product describes something it mostly cannot do yet. Three
// states, and each is said plainly rather than greyed out:
//
//   Described only  - we know the feature exists, not where the setting lives.
//                     That mapping is manufacturer-specific and unpublished,
//                     and this project will not guess it.
//   Read only       - somebody supplied a mapping but nobody has verified it.
//                     Good enough to look at, never good enough to write.
//   Changeable      - a verified mapping exists and policy permits the class.
//
// "Preview" runs the whole validation chain and shows every check with its
// answer. That is deliberately more informative than a disabled button: a
// person who sees "your adapter cannot reach the second CAN bus" knows what to
// buy, and a person who sees a grey button knows nothing.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { ChangePlan, FeatureView, FeaturesData, ToolResult } from "../api/types";
import { ErrorBanner, FailedResult, Spinner, Warnings } from "./primitives";
import { PaneIntro, useExplain } from "../explain";

const SUPPORT: Record<FeatureView["support"], { label: string; tone: string; blurb: string }> = {
  described_only: {
    label: "explained only",
    tone: "var(--text-faint)",
    blurb:
      "We can tell you what this is. We do not know which setting inside the car holds it, so we cannot read it or change it.",
  },
  read_only: {
    label: "can be read",
    tone: "var(--caution)",
    blurb:
      "Somebody supplied the location of this setting but it has not been checked against a real vehicle, so it can be looked at and never changed.",
  },
  writable: {
    label: "can be changed",
    tone: "var(--ok)",
    blurb: "The location of this setting has been verified.",
  },
};

export function FeaturesPane({ connected }: { connected: boolean }) {
  const [result, setResult] = useState<ToolResult<FeaturesData> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [why, setWhy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setResult(await api.features());
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (connected) void load();
  }, [connected, load]);

  const features = result?.data?.features ?? [];

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="vehicle_features" />

      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <div className="row">
          <strong>Vehicle settings</strong>
          {result?.data && (
            <span className="faint">
              {features.length} known for trucks like yours
            </span>
          )}
          {/* The answer, at the top, in a line. This screen used to open with
              several paragraphs about why a mapping is needed and what
              verification means — good writing, and not what somebody wanting
              to fold their mirrors came to find out. */}
          {result?.data && !!features.length && (
            <span className="tag" style={{ color: countOf(features, "writable") ? "var(--ok)" : "var(--text-faint)" }}>
              {countOf(features, "writable")} changeable
              {" · "}
              {countOf(features, "read_only")} readable
              {" · "}
              {countOf(features, "described_only")} not measured yet
            </span>
          )}
        </div>
        <button onClick={() => void load()} disabled={busy}>
          {busy ? <Spinner label="Reading" /> : "Refresh"}
        </button>
      </div>

      {/* The complaint this answers, verbatim: "it tells me my car has autofold
          mirrors to activate, but then it tells me it can't". The list was
          being read as a scan of the vehicle. It is not — it is a reference
          list filtered by make and year, and saying so up front is the
          difference between a useful reference and a broken promise. */}
      {/* One line by default, the reasoning behind a toggle.
          Both of these blocks say something true and necessary, and having both
          open permanently meant the screen began with two full-width walls of
          text before a single feature was visible. The claim stays where
          somebody will read it; the argument moves to where somebody can ask
          for it. */}
      <div className="banner caution">
        <span className="b-code">read this first</span>
        <div style={{ minWidth: 0 }}>
          <div className="row" style={{ gap: 10, justifyContent: "space-between" }}>
            <strong>A reference list, not a scan of your truck.</strong>
            <button className="mini" onClick={() => setWhy((w) => !w)}>
              {why ? "Less" : "Why?"}
            </button>
          </div>
          {why && (
            <div style={{ marginTop: 6 }}>
              These are settings that exist on vehicles of this make and year. Nothing here has
              been read from your vehicle, so a feature appearing in this list does not mean yours
              was built with the hardware — and one missing does not mean it wasn't. Use it to know
              what to ask for; do not treat it as a diagnosis.
              <div style={{ marginTop: 10 }}>
                <strong>And this app will not guess where a setting lives.</strong> Which bits
                inside a module hold something like auto-folding mirrors is not published by any
                manufacturer. Getting it wrong writes the wrong thing into a door module, so
                nothing is written until somebody has measured it on a real vehicle. Any feature
                below that says <em>not measured yet</em> tells you how to close that gap
                yourself, and everything loaded from a profile file is labelled with the file it
                came from.
              </div>
            </div>
          )}
        </div>
      </div>

      <ErrorBanner error={error} />
      {result && !result.success && <FailedResult result={result} />}
      {result && <Warnings warnings={result.warnings} />}

      {!connected && (
        <div className="banner caution">
          <span className="b-code">not connected</span>
          <span>Connect to a vehicle to see which of these could apply to it.</span>
        </div>
      )}

      {features.map((f) => (
        <FeatureCard
          key={f.id}
          feature={f}
          expanded={open === f.id}
          onToggle={() => setOpen(open === f.id ? null : f.id)}
        />
      ))}

      {connected && !busy && !features.length && (
        <div className="empty">No configurable features are catalogued for this vehicle yet.</div>
      )}
    </div>
  );
}

function FeatureCard({
  feature,
  expanded,
  onToggle,
}: {
  feature: FeatureView;
  expanded: boolean;
  onToggle: () => void;
}) {
  const { easy } = useExplain();
  const [plan, setPlan] = useState<ToolResult<ChangePlan> | null>(null);
  const [busy, setBusy] = useState(false);
  const s = SUPPORT[feature.support];

  async function preview(desired: "on" | "off") {
    setBusy(true);
    try {
      setPlan(await api.previewFeature(feature.id, desired));
    } catch {
      /* The plan is advisory; a failure to fetch it is not worth a banner. */
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between", alignItems: "flex-start" }}>
        <div style={{ minWidth: 0 }}>
          <div className="row" style={{ gap: 8 }}>
            <strong>{feature.name}</strong>
            <span className="tag" style={{ color: s.tone }}>{s.label}</span>
            <span className="tag">{feature.risk_label}</span>
          </div>
          <div className="explain">{easy ? feature.easy : feature.technical}</div>
        </div>
        <button className="mini" onClick={onToggle}>
          {expanded ? "Less" : "Details"}
        </button>
      </div>

      {expanded && (
        <>
          <div className="explain" style={{ marginTop: 10 }}>{s.blurb}</div>

          {feature.notes && (
            <div className="banner caution" style={{ marginTop: 10, marginBottom: 0 }}>
              <span className="b-code">worth knowing</span>
              <span>{feature.notes}</span>
            </div>
          )}

          {!feature.writable_in_principle && (
            <div className="banner serious" style={{ marginTop: 10, marginBottom: 0 }}>
              <span className="b-code">will not be changed</span>
              <span>
                This is classed <strong>{feature.risk_label}</strong>. This app does not change
                things in that category, and supplying a verified location for it would not
                change that.
              </span>
            </div>
          )}

          {/* The screen used to explain why nothing could be done and stop
              there, which on a vehicle where every mapping is null is a wall of
              text ending in "no". This is the other half: nobody has measured
              it *yet*, and measuring it is a procedure rather than a mystery.

              Only shown where it is true. A feature refused on risk grounds
              gets the banner above and no encouragement, because no amount of
              measuring changes that answer. */}
          {feature.support === "described_only" && feature.writable_in_principle && (
            <div className="banner info" style={{ marginTop: 10, marginBottom: 0 }}>
              <span className="b-code">not measured yet</span>
              <span>
                Nobody has recorded which bits hold this on your vehicle — that is a gap, not a
                refusal, and you can close it without waiting for a new version:
                <ol style={{ margin: "6px 0 0", paddingLeft: 18 }}>
                  <li>Capture the module's configuration from the Settings tab.</li>
                  <li>Change the setting once, using the vehicle's own menu or a tool that
                      already does it.</li>
                  <li>Capture again. The app reports only the bits that moved — that is the
                      mapping.</li>
                  <li>Do it once more in reverse, which is what separates the real bit from a
                      coincidence.</li>
                </ol>
              </span>
            </div>
          )}

          <div className="row" style={{ marginTop: 10, gap: 6 }}>
            <button className="mini" onClick={() => void preview("on")} disabled={busy}>
              {busy ? <Spinner /> : "What would turning it on involve?"}
            </button>
            <button className="mini" onClick={() => void preview("off")} disabled={busy}>
              Turning it off?
            </button>
          </div>

          {plan?.data && <PlanChecks plan={plan.data} />}

          {!easy && (
            <div className="provenance" style={{ marginTop: 8 }}>
              <span>{feature.id}</span>
              {feature.modules.length > 0 && <span>modules {feature.modules.join(", ")}</span>}
              <span>{feature.verification}</span>
              {feature.source && <span>from {feature.source}</span>}
            </div>
          )}
        </>
      )}
    </div>
  );
}

/**
 * Every check and its answer.
 *
 * Failures split into two kinds and they read very differently: something the
 * user could fix (a better adapter, the key on, a charger on the battery), and
 * something this build will never do. Presenting both as "unavailable" would
 * send someone shopping for a cable that will not help.
 */
function PlanChecks({ plan }: { plan: ChangePlan }) {
  return (
    <div className="section" style={{ marginTop: 12, marginBottom: 0 }}>
      <h2>
        What has to be true{plan.can_apply ? "" : " — and is not"}
      </h2>
      <table>
        <tbody>
          {plan.checks.map((c) => (
            <tr key={c.id}>
              <td style={{ width: 26 }}>
                <span style={{ color: c.passed ? "var(--ok)" : c.blocking_by_design ? "var(--serious)" : "var(--caution)" }}>
                  {c.passed ? "✓" : c.blocking_by_design ? "✕" : "!"}
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
    </div>
  );
}

/** How many features are in one support state.
 *
 * The header answers "what can I actually do here" before any explanation of
 * why. On most vehicles the honest answer is "none yet", and saying so in a
 * line beats burying it under the reasoning. */
function countOf(features: FeatureView[], support: FeatureView["support"]): number {
  return features.filter((f) => f.support === support).length;
}
