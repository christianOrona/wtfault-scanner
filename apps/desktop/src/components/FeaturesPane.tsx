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

import { useCallback, useEffect, useRef, useState } from "react";
import { api, describeError, type ProfilePreview } from "../api/client";
import { VehicleKnowledgePanel } from "./VehicleKnowledgePanel";
import type { FeatureView, FeaturesData, ToolResult } from "../api/types";
import { ChangeFlow } from "./ChangeFlow";
import { ErrorBanner, FailedResult, Spinner, Warnings } from "./primitives";
import { PaneIntro, useExplain } from "../explain";
import { AsBuiltPanel } from "./AsBuiltPanel";

const MEASURED_HERE = {
  label: "measured here",
  tone: "var(--ok)",
  blurb:
    "Where this setting lives was measured on this vehicle. Nobody has changed it with this app yet: the first time, the app asks the module whether it accepts changes, then reads the new value back.",
};

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
    // Not "never changed". This used to say so, and for a comfort setting the
    // app will in fact offer to try — so the card contradicted the button
    // beneath it. The preview is where "can it be changed here" is decided,
    // and the blurb now points there instead of pre-empting it.
    blurb:
      "Somebody supplied the location of this setting but it has not been checked against this vehicle. It can be read. Whether it can be changed here depends on what kind of setting it is — the preview below says, and calls it an experiment if it would be one.",
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

      {/* What this truck has already taught us, including what it has ruled
          out. Above the catalogue on purpose: this screen should not keep
          offering something that has been tried here and did not work. */}
      <VehicleKnowledgePanel />

      <div className="row" style={{ justifyContent: "space-between", marginBottom: 12 }}>
        <div className="row">
          <strong>Vehicle settings</strong>
          {result?.data && (
            <span className="faint">
              {features.length} known for vehicles like yours
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
          {/* Counted separately because it is a different kind of thing, not a
              fourth degree of the same one: these were measured, just not on
              this vehicle. */}
          {!!result?.data?.from_a_similar_vehicle && (
            <span className="tag" style={{ color: "var(--caution)" }}>
              {result.data.from_a_similar_vehicle} from a similar vehicle
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
            <strong>A reference list, not a scan of your vehicle.</strong>
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

      {/* The manufacturer's own record of this vehicle, which covers every
          module rather than only the ones awake on the bus. Placed above the
          list because it changes what the list can answer. */}
      <AsBuiltPanel connected={connected} />

      {/* The way a setting gets onto this list. The API could take a profile
          from the start and no screen offered it, so somebody who had a
          mapping for their car had nowhere to put it, and anything dropped in
          the folder needed the app restarted before it showed. */}
      <AddSettings connected={connected} onAdded={() => void load()} />

      {features.map((f) => (
        <FeatureCard
          key={f.id}
          feature={f}
          expanded={open === f.id}
          onToggle={() => setOpen(open === f.id ? null : f.id)}
          onChanged={() => void load()}
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
  onChanged,
}: {
  feature: FeatureView;
  expanded: boolean;
  onToggle: () => void;
  /** After a change the module read back, so the list shows the new state. */
  onChanged: () => void;
}) {
  const { easy } = useExplain();
  // Which way the person is looking at changing it. Null until they pick,
  // because a preview reads the vehicle and should not happen on expanding a
  // card to read about it.
  const [desired, setDesired] = useState<"on" | "off" | null>(null);
  // A location measured on this vehicle, on a module nobody has written yet, is
  // "read only" to the catalogue and was labelled "can be read … never
  // changed" — on the double horn chirp, directly above the button that turned
  // it off. It is a measured setting awaiting its first change, and says so.
  const s =
    feature.support === "read_only" && feature.measured_on_this_vehicle && feature.verification === "verified"
      ? MEASURED_HERE
      : SUPPORT[feature.support];

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between", alignItems: "flex-start" }}>
        <div style={{ minWidth: 0 }}>
          <div className="row" style={{ gap: 8 }}>
            <strong>{feature.name}</strong>
            <span className="tag" style={{ color: s.tone }}>{s.label}</span>
            <span className="tag">{feature.risk_label}</span>
            {/* Said on the card rather than only inside Details. Somebody
                scanning this list should be able to see which of these are
                about their truck without opening each one. */}
            {feature.authority === "measured_on_similar_vehicle" && (
              <span className="tag" style={{ color: "var(--caution)" }}>
                from a similar vehicle
              </span>
            )}
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

          {/* The candidate case, and the offer that goes with it. Scoping a
              measured mapping to one exact VIN and stopping meant the next
              identical truck got nothing at all; offering it and saying where
              it came from lets this vehicle settle the question, without a
              single byte being written to find out. */}
          {!feature.measured_on_this_vehicle && feature.support !== "described_only" && (
            <div className="banner caution" style={{ marginTop: 10, marginBottom: 0 }}>
              <span className="b-code">from another vehicle</span>
              <span>
                {feature.authority_explanation}
                <div style={{ marginTop: 6 }}>
                  Read it and this app will tell you what it thinks the setting currently is.
                  Check that against what your vehicle actually shows. If they agree, that is
                  evidence measured on <em>your</em> vehicle; if they do not, this mapping is
                  not for your vehicle. Reading writes nothing. Whether it can be changed here is
                  decided by the checks in the preview — a comfort setting may be offered as an
                  experiment, and is called one.
                </div>
              </span>
            </div>
          )}

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
            <button
              className={`mini${desired === "on" ? " primary" : ""}`}
              onClick={() => setDesired("on")}
            >
              What would turning it on involve?
            </button>
            <button
              className={`mini${desired === "off" ? " primary" : ""}`}
              onClick={() => setDesired("off")}
            >
              Turning it off?
            </button>
          </div>

          {/* Keyed on the direction so switching from on to off starts a
              fresh preview instead of showing the other direction's bytes
              under the new button. */}
          {desired && (
            <ChangeFlow
              key={desired}
              featureId={feature.id}
              desired={desired}
              autoPreview
              onChanged={onChanged}
            />
          )}

          {!easy && (
            <div className="provenance" style={{ marginTop: 8 }}>
              <span>{feature.id}</span>
              {feature.modules.length > 0 && <span>modules {feature.modules.join(", ")}</span>}
              <span>{feature.verification}</span>
              <span>{feature.authority.replace(/_/g, " ")}</span>
              {feature.source && <span>from {feature.source}</span>}
            </div>
          )}
        </>
      )}
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

/**
 * Add settings for a car from a profile file, and see them without a restart.
 *
 * Two steps, like every import here: the file is read and what it would add
 * is shown, and nothing is kept until that is agreed to. A setting from a
 * file arrives unverified whatever the file says about itself, which is the
 * core's rule and not this screen's to soften.
 */
function AddSettings({ connected, onAdded }: { connected: boolean; onAdded: () => void }) {
  const picker = useRef<HTMLInputElement>(null);
  const [pending, setPending] = useState<{ name: string; text: string; preview: ProfilePreview } | null>(null);
  const [said, setSaid] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  async function chosen(file: File) {
    setWorking(true);
    setSaid(null);
    setPending(null);
    try {
      const text = await file.text();
      const { preview } = await api.previewProfile(text, file.name);
      setPending({ name: file.name, text, preview });
    } catch (e) {
      setSaid(`Could not read ${file.name}: ${describeError(e).message}`);
    } finally {
      setWorking(false);
    }
  }

  async function add() {
    if (!pending) return;
    setWorking(true);
    try {
      const done = await api.importProfile(pending.text, pending.name);
      setSaid(
        `Added ${done.features} ${done.features === 1 ? "setting" : "settings"} from ${pending.name}. ` +
          (done.reconnect_to_use
            ? "Press Disconnect and connect again to see them: the vehicle connected now keeps the list it started with."
            : "They are listed the next time you connect."),
      );
      setPending(null);
      onAdded();
    } catch (e) {
      setSaid(`Nothing was added: ${describeError(e).message}`);
    } finally {
      setWorking(false);
    }
  }

  async function checkFolder() {
    setWorking(true);
    setSaid(null);
    try {
      const r = await api.reloadProfiles();
      const gained = r.settings_now - r.settings_before;
      setSaid(
        (gained > 0
          ? `Found ${gained} more ${gained === 1 ? "setting" : "settings"} in the profiles folder.`
          : "Nothing new in the profiles folder.") +
          (gained > 0 && r.reconnect_to_use ? " Press Disconnect and connect again to see them." : ""),
      );
      onAdded();
    } catch (e) {
      setSaid(`Could not check the folder: ${describeError(e).message}`);
    } finally {
      setWorking(false);
    }
  }

  const blocking = pending?.preview.findings.filter((f) => f.severity === "blocking") ?? [];
  const worth = pending?.preview.findings.filter((f) => f.severity !== "blocking") ?? [];

  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between", gap: 12 }}>
        <div>
          <strong>Add settings for this car</strong>
          <div className="faint">
            Have a mapping, yours or somebody else's? Add its profile file and its settings are
            listed here. {connected ? "" : "You do not need to be connected to add one. "}
            A setting from a file arrives unverified, whatever the file says about itself.
          </div>
        </div>
        <div className="row" style={{ gap: 8 }}>
          <input
            ref={picker}
            type="file"
            accept=".yaml,.yml"
            style={{ display: "none" }}
            onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (file) void chosen(file);
            }}
          />
          <button onClick={() => picker.current?.click()} disabled={working}>
            {working && !pending ? <Spinner /> : "Add a profile file"}
          </button>
          <button
            title="For a file you put in the profiles folder yourself. Settings shows where the folder is."
            onClick={() => void checkFolder()}
            disabled={working}
          >
            Check the folder again
          </button>
        </div>
      </div>

      {pending && (
        <div style={{ marginTop: 10 }}>
          <strong>{pending.name}</strong>
          {pending.preview.changes.length > 0 && (
            <ul style={{ margin: "6px 0 0 18px" }}>
              {pending.preview.changes.map((c) => (
                <li key={c.id}>
                  {c.name}
                  {c.overrides_measured
                    ? " — replaces a setting measured on this vehicle"
                    : c.overrides_existing
                      ? " — replaces the one already listed"
                      : ""}
                </li>
              ))}
            </ul>
          )}
          {blocking.map((f, i) => (
            <div key={i} className="banner caution" style={{ marginTop: 8, marginBottom: 0 }}>
              <span className="b-code">cannot be added</span>
              <span>{f.detail}</span>
            </div>
          ))}
          {worth.map((f, i) => (
            <div key={i} className="faint" style={{ marginTop: 6 }}>{f.detail}</div>
          ))}
          <div className="row" style={{ gap: 8, marginTop: 10 }}>
            {pending.preview.acceptable && (
              <button className="primary" onClick={() => void add()} disabled={working}>
                {working ? <Spinner /> : `Add ${pending.preview.changes.length === 1 ? "this setting" : `these ${pending.preview.changes.length} settings`}`}
              </button>
            )}
            <button onClick={() => setPending(null)} disabled={working}>Cancel</button>
          </div>
        </div>
      )}

      {said && <div className="faint" style={{ marginTop: 8 }}>{said}</div>}
    </div>
  );
}
