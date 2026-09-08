// What this adapter can reach, and what it cannot.
//
// The complaint this answers: a feature was listed, the app said it could not
// do it, and the reason ("your adapter is on one bus") only appeared after the
// user had already gone looking. Adapter limits are not an error to discover
// mid-task — they are a property of the hardware, knowable the moment it is
// plugged in, and they decide what the whole app can do.
//
// Two halves, kept strictly apart:
//
//   Observed  - what this adapter actually demonstrated during connection.
//               Facts, from the capability record.
//   General   - what to look for in a different adapter. This is not measured
//               by this project and is labelled as such, in the same way the
//               agent's cost estimates are.

import type { AdapterCapabilities } from "../api/types";
import { useExplain } from "../explain";

/** One thing an adapter either can or cannot do, and what it unlocks. */
interface Ability {
  id: string;
  /** Plain-language name. */
  label: string;
  present: (c: AdapterCapabilities) => boolean;
  /** What having it makes possible. */
  unlocks: string;
  /** What its absence blocks. Empty when nothing important depends on it. */
  blocks: string;
}

const ABILITIES: Ability[] = [
  {
    id: "transmit",
    label: "Can ask the vehicle questions",
    present: (c) => c.supports_transmit,
    unlocks: "Everything. Without this the adapter can only listen.",
    blocks: "Nothing can be read at all.",
  },
  {
    id: "iso_tp",
    label: "Handles long answers",
    present: (c) => c.iso_tp,
    unlocks: "The vehicle identification number, calibration details, and self-test results.",
    blocks: "Anything longer than a single message, including the VIN.",
  },
  {
    id: "can29",
    label: "Speaks the wider addressing scheme",
    present: (c) => c.can_29_bit,
    unlocks: "Modules on vehicles that use 29-bit addressing, common on trucks and heavy vehicles.",
    blocks: "Some modules on some vehicles will simply not answer.",
  },
  {
    id: "buses",
    label: "Reaches more than one network",
    present: (c) => c.multiple_can_buses,
    unlocks:
      "Doors, body, instrument cluster and comfort modules. On many vehicles these sit on a second, slower network.",
    blocks:
      "Anything not on the main engine network. Mirror folding, lighting, locking and cluster settings all live there.",
  },
  {
    id: "j2534",
    label: "Manufacturer-level access",
    present: (c) => c.j2534,
    unlocks: "Module configuration and programming, with the right software.",
    blocks: "Nothing this app does today; this app does not write configuration.",
  },
];

export function AdapterFitness({ caps }: { caps: AdapterCapabilities }) {
  const { easy } = useExplain();
  const have = ABILITIES.filter((a) => a.present(caps));
  const missing = ABILITIES.filter((a) => !a.present(caps));

  return (
    <div className="section">
      <h2>What this adapter can reach</h2>

      <div className="explain">
        Everything below was observed while connecting — what the adapter actually did, not
        what its label claims. A cheap adapter that reports itself as an ELM327 v1.5 or v2.1
        is reporting a name, and the name is frequently borrowed.
      </div>

      <table style={{ marginTop: 10 }}>
        <tbody>
          {have.map((a) => (
            <tr key={a.id}>
              <td style={{ width: 24, color: "var(--ok)" }}>✓</td>
              <td>
                <div>{a.label}</div>
                <div className="explain" style={{ marginTop: 2 }}>{a.unlocks}</div>
              </td>
            </tr>
          ))}
          {missing.map((a) => (
            <tr key={a.id}>
              <td style={{ width: 24, color: "var(--caution)" }}>—</td>
              <td>
                <div className="faint">{a.label}</div>
                <div className="explain" style={{ marginTop: 2 }}>
                  <strong>Not available.</strong> {a.blocks}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      {/* The caveats are the adapter's own admissions, recorded during
          connection. They are the most honest thing on this screen. */}
      {caps.caveats.length > 0 && (
        <div className="banner caution" style={{ marginTop: 12 }}>
          <span className="b-code">observed limits</span>
          <ul className="caveats">
            {caps.caveats.map((c, i) => <li key={i}>{c}</li>)}
          </ul>
        </div>
      )}

      {!caps.multiple_can_buses && (
        <div className="banner info" style={{ marginTop: 4 }}>
          <span className="b-code">why some things are greyed out</span>
          <div>
            <strong>This adapter can only reach the main engine network.</strong>
            <div style={{ marginTop: 6 }}>
              Most vehicles run at least two internal networks. Engine, transmission and
              emissions live on the fast one, which is what the diagnostic socket exposes by
              default. Doors, mirrors, lighting, locking and the instrument cluster usually sit
              on a second, slower one. No amount of software makes an adapter wired to the first
              reach the second — it needs hardware that can switch between them.
            </div>
          </div>
        </div>
      )}

      {!easy && (
        <div className="provenance" style={{ marginTop: 10 }}>
          <span>{caps.vendor}</span>
          <span>{caps.model}</span>
          {caps.firmware && <span>{caps.firmware}</span>}
          <span>{caps.transport}</span>
          {caps.max_reliable_throughput != null && (
            <span>~{caps.max_reliable_throughput.toFixed(1)} req/s observed</span>
          )}
        </div>
      )}

      <details style={{ marginTop: 12 }}>
        <summary className="faint" style={{ cursor: "pointer", fontSize: 12 }}>
          what to look for in a different adapter
        </summary>
        <div className="banner caution" style={{ marginTop: 8 }}>
          <span className="b-code">general knowledge</span>
          <span>
            The rest of this section is general automotive knowledge, not something this app has
            tested. No adapter has been measured by this project except the one you have plugged
            in. Treat it as a starting point for your own research, not a recommendation.
          </span>
        </div>
        <div className="explain">
          <p style={{ marginTop: 0 }}>
            <strong>The capability that matters most is switching networks.</strong> If you want
            this app to reach door, body or cluster modules, that is the one thing to check for.
            Look for an adapter that explicitly advertises switching between the fast and slow
            vehicle networks — often written as HS-CAN and MS-CAN. Most inexpensive adapters do
            not, whatever else they claim.
          </p>
          <p>
            <strong>Second: how it handles long answers.</strong> Cheap adapters commonly drop
            part of a long reply, which is what produces the "incomplete response" warnings you
            may have seen on this vehicle. Adapters built around a purpose-made interpreter chip
            rather than a copied one handle this considerably better, and are also faster.
          </p>
          <p>
            <strong>Third: whether it is a genuine design or a copy.</strong> A great many
            adapters sold as ELM327 are copies reporting a borrowed version string. They mostly
            work for reading codes and live data; they are where the reliability problems come
            from once you ask for anything longer.
          </p>
          <p style={{ marginBottom: 0 }}>
            Names worth researching for a Ford truck, from general reputation rather than any
            testing here: OBDLink EX (wired) and OBDLink MX+ (wireless), both built on a
            purpose-made interpreter chip with network switching. The community around Ford
            diagnostics has settled on these; verify that for yourself before spending money.
          </p>
        </div>
      </details>
    </div>
  );
}
