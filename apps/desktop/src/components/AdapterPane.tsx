// What the adapter is, what it demonstrated it can do, and what it only claims.
//
// docs/API.md calls `capabilities.caveats` the honest part of the API, so it is
// given the top of this pane rather than a collapsed "details" section. A cheap
// clone reporting v2.1 while behaving like v1.5 is a fact the user should see
// before they trust a reading, not after.

import { useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { AdapterStatus, CapabilitiesResponse } from "../api/types";
import { ErrorBanner, Pill, stateDetail } from "./primitives";
import { PaneIntro } from "../explain";
import { AdapterFitness } from "./AdapterFitness";

export function AdapterPane({ adapter }: { adapter: AdapterStatus | null }) {
  const [caps, setCaps] = useState<CapabilitiesResponse | null>(null);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);

  useEffect(() => {
    api.capabilities().then(setCaps).catch((e) => setError(describeError(e)));
  }, [adapter?.session_id]);

  if (!adapter?.connected) return <div className="empty">Nothing connected.</div>;

  const c = adapter.capabilities;
  const h = adapter.health;
  const detail = stateDetail(adapter.state);

  return (
    <div className="pane">
      <PaneIntro kind="concept" id="adapter" />
      <ErrorBanner error={error} />

      <div className="section">
        <h2>Connection</h2>
        <div className="card">
          <div className="row" style={{ justifyContent: "space-between" }}>
            <div className="row">
              <Pill state={adapter.state} />
              <span className="mono">{adapter.descriptor}</span>
            </div>
            <span className="faint mono">{adapter.session_id}</span>
          </div>
          {detail && (
            <div className={`banner ${adapter.state.state === "failed" ? "serious" : "caution"}`} style={{ marginTop: 10, marginBottom: 0 }}>
              <span className="b-code">{adapter.state.state}</span>
              <span>{detail}</span>
            </div>
          )}
        </div>
      </div>

      {/* Fitness first: what the hardware can reach decides what the whole
          app can do, and it was previously only discoverable by hitting a
          wall mid-task. The raw caveat list is folded into it. */}
      {c && <AdapterFitness caps={c} />}

      <div className="section">
        <h2>Adapter</h2>
        <div className="card">
          <table>
            <tbody>
              <Row k="Transport" v={c?.transport} />
              <Row k="Vendor" v={c?.vendor} />
              <Row k="Model" v={c?.model} />
              <Row k="Firmware" v={c?.firmware} />
              <Row k="Protocol" v={h?.protocol} />
              <Row k="ELM327 compatible" v={yn(c?.elm327_compatible)} />
              <Row k="CAN 11-bit / 29-bit" v={c ? `${yn(c.can_11_bit)} / ${yn(c.can_29_bit)}` : null} />
              <Row k="ISO-TP" v={yn(c?.iso_tp)} />
              <Row k="Multiple CAN buses" v={yn(c?.multiple_can_buses)} />
              <Row k="J2534" v={yn(c?.j2534)} />
              <Row k="Long messages" v={yn(c?.supports_long_messages)} />
              <Row
                k="Max reliable throughput"
                v={c?.max_reliable_throughput != null ? `${c.max_reliable_throughput} msg/s` : null}
              />
            </tbody>
          </table>
        </div>
      </div>

      {h && (
        <div className="section">
          <h2>Traffic</h2>
          <div className="card">
            <table>
              <tbody>
                <Row k="Requests / responses" v={`${h.requests} / ${h.responses}`} />
                <Row k="Timeouts" v={String(h.timeouts)} />
                <Row k="No data" v={String(h.no_data)} />
                <Row
                  k="Adapter errors"
                  v={
                    <>
                      {h.adapter_errors}
                      {h.adapter_errors > 0 && (
                        <span className="faint"> - normal for a cheap clone</span>
                      )}
                    </>
                  }
                />
                <Row k="Mean latency" v={h.mean_latency_ms != null ? `${h.mean_latency_ms.toFixed(1)} ms` : null} />
                <Row k="Battery" v={h.battery_voltage != null ? `${h.battery_voltage.toFixed(1)} V` : null} />
              </tbody>
            </table>
          </div>
        </div>
      )}

      {caps?.observed_conditions && (
        <div className="section">
          <h2>Observed conditions</h2>
          <div className="card">
            <div className="faint" style={{ marginBottom: 8 }}>
              Every field here came from an actual reading. A null is &quot;not measured&quot;, never
              an assumption.
            </div>
            <table>
              <tbody>
                <Row k="Ignition on" v={yn(caps.observed_conditions.ignition_on)} />
                <Row k="Engine running" v={yn(caps.observed_conditions.engine_running)} />
                <Row k="Battery" v={caps.observed_conditions.battery_voltage != null ? `${caps.observed_conditions.battery_voltage} V` : null} />
                <Row k="Connection stable" v={yn(caps.observed_conditions.connection_stable)} />
                <Row k="Vehicle speed" v={caps.observed_conditions.vehicle_speed_kph != null ? `${caps.observed_conditions.vehicle_speed_kph} km/h` : null} />
              </tbody>
            </table>
          </div>
        </div>
      )}

      {caps && (
        <div className="section">
          <h2>Permissions - ceiling {caps.max_enabled_level}</h2>
          <div className="card">
            <div className="faint" style={{ marginBottom: 8 }}>
              Disabled operations are listed rather than hidden, so their refusal is explicit.
              Nothing in this build writes to the vehicle.
            </div>
            <table>
              <thead>
                <tr><th>Capability</th><th>Level</th><th>State</th><th>Description</th></tr>
              </thead>
              <tbody>
                {caps.capabilities.map((cap) => (
                  <tr key={cap.id}>
                    <td className="mono">{cap.id}</td>
                    <td><span className="tag level">{cap.level}</span></td>
                    <td>
                      {cap.enabled
                        ? <span className="faint">available</span>
                        : <span className="tag confirmed">refused</span>}
                      {cap.mutating && <span className="tag" style={{ marginLeft: 6 }}>mutating</span>}
                    </td>
                    <td className="faint">{cap.description}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <tr>
      <td className="faint" style={{ width: 200 }}>{k}</td>
      <td className="mono">{v ?? <span className="faint">not reported</span>}</td>
    </tr>
  );
}

function yn(b: boolean | null | undefined): string | null {
  return b == null ? null : b ? "yes" : "no";
}
