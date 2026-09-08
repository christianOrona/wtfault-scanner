// Choosing what to connect to. Two paths: the virtual truck, or a real adapter
// on a COM port.
//
// The probe is a deliberate action because it briefly opens every port. Ports
// that fail to open are shown WITH their reason - "COM3 is in use by another
// program" is usually the answer the user actually needs.

import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "../api/client";
import type { Health, PortsResponse, ProbedPort, SerialPortInfo, ToolResult, ConnectData } from "../api/types";
import { ErrorBanner, FailedResult, Spinner } from "./primitives";

type Transport = "simulator" | "serial";

export function ConnectDialog({
  health,
  onConnected,
  onDismiss,
}: {
  health: Health | null;
  onConnected: () => void;
  /** Close without connecting, to browse stored sessions. */
  onDismiss?: () => void;
}) {
  const [transport, setTransport] = useState<Transport>(
    health?.default_transport === "serial" ? "serial" : "simulator",
  );
  const [scenario, setScenario] = useState(health?.default_scenario ?? "healthy");
  const [port, setPort] = useState("");
  const [label, setLabel] = useState("");

  const [ports, setPorts] = useState<PortsResponse | null>(null);
  const [probing, setProbing] = useState(false);
  const [portError, setPortError] = useState<{ code: string; message: string } | null>(null);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [failed, setFailed] = useState<ToolResult<ConnectData> | null>(null);

  const loadPorts = useCallback(async (probe: boolean) => {
    setProbing(true);
    setPortError(null);
    try {
      const res = await api.ports(probe);
      setPorts(res);
      // Preselect the first port that answered like an ELM327, else the first hint.
      const first =
        res.ports.find((p) => isProbed(p) && p.identification?.elm327_compatible) ??
        res.ports.find((p) => (isProbed(p) ? p.port : p).likely_obd_adapter) ??
        res.ports[0];
      if (first) setPort((isProbed(first) ? first.port : first).name);
    } catch (e) {
      setPortError(describeError(e));
    } finally {
      setProbing(false);
    }
  }, []);

  useEffect(() => {
    if (transport === "serial" && !ports) void loadPorts(false);
  }, [transport, ports, loadPorts]);

  async function connect() {
    setBusy(true);
    setError(null);
    setFailed(null);
    try {
      const result = await api.connect({
        transport,
        ...(transport === "serial" ? { port } : { scenario }),
        ...(label.trim() ? { label: label.trim() } : {}),
      });
      // 200 only means it ran. A degraded connect still succeeds - the adapter
      // being fine and the truck answering are different facts - so the caller
      // renders the warning; only an actual failure keeps us on this screen.
      if (!result.success) {
        setFailed(result);
        return;
      }
      onConnected();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const canConnect = !busy && (transport === "simulator" ? !!scenario : !!port);

  return (
    <div className="overlay">
      <div className="dialog">
        <div className="row" style={{ justifyContent: "space-between", alignItems: "flex-start" }}>
          <h1>Connect</h1>
          {/* Dismissable on purpose: every past scan is stored, and blocking the
              whole window behind this dialog made previous sessions
              unreachable without a vehicle to hand. */}
          {onDismiss && (
            <button onClick={onDismiss} title="Close and browse past scans">
              Close
            </button>
          )}
        </div>
        <p className="muted">
          {health
            ? "Just ask your car what the fuck is wrong."
            : "Looking for the diagnostic core..."}
        </p>

        <ErrorBanner error={error} />
        {failed && <FailedResult result={failed} />}

        <div className="field">
          <label>Connect to</label>
          <div className="seg">
            <button
              aria-pressed={transport === "simulator"}
              onClick={() => setTransport("simulator")}
            >
              Virtual vehicle
            </button>
            <button aria-pressed={transport === "serial"} onClick={() => setTransport("serial")}>
              Real adapter
            </button>
          </div>
        </div>

        {transport === "simulator" ? (
          <div className="field">
            <label>Scenario</label>
            <select value={scenario} onChange={(e) => setScenario(e.target.value)}>
              {(health?.scenarios ?? []).map((s) => (
                <option key={s.id} value={s.id}>{s.id}</option>
              ))}
            </select>
            <span className="faint">
              {health?.scenarios.find((s) => s.id === scenario)?.description}
            </span>
          </div>
        ) : (
          <div className="field">
            <label>Serial port</label>
            <ErrorBanner error={portError} />
            <div className="row">
              <select
                value={port}
                onChange={(e) => setPort(e.target.value)}
                style={{ flex: 1, minWidth: 180 }}
              >
                {!ports?.ports.length && <option value="">no ports found</option>}
                {ports?.ports.map((p) => {
                  const info = isProbed(p) ? p.port : p;
                  return (
                    <option key={info.name} value={info.name}>
                      {info.name}
                      {info.kind ? ` - ${info.kind}` : ""}
                      {info.likely_obd_adapter ? " (likely OBD)" : ""}
                    </option>
                  );
                })}
              </select>
              <button onClick={() => void loadPorts(false)} disabled={probing}>Refresh</button>
              <button onClick={() => void loadPorts(true)} disabled={probing}>
                {probing ? <Spinner label="Probing" /> : "Probe"}
              </button>
            </div>

            <span className="faint">
              A paired Bluetooth ELM327 shows up as an outgoing COM port. The
              &quot;likely OBD&quot; hint is name-based only - probe to confirm.
            </span>

            {ports?.probed && <ProbeResults ports={ports.ports} />}

            {/* The first screen must never be a dead end.
                The desktop build defaults to the real adapter, so someone
                opening it before pairing one lands here with an empty list and
                a disabled button, and nothing tells them the app is fully
                usable right now against the virtual vehicle. */}
            {!ports?.ports.length && !probing && (
              <div className="banner info" style={{ marginTop: 8 }}>
                <span className="b-code">no adapter</span>
                <div>
                  <div>
                    No COM ports yet, so there is nothing to connect to. To pair one: Settings
                    &gt; Bluetooth &amp; devices &gt; Add device (PIN is usually 1234 or 0000),
                    then use the <strong>Outgoing</strong> port under More Bluetooth options &gt;
                    COM Ports.
                  </div>
                  <div style={{ marginTop: 8 }}>
                    You do not need one to try the app. The virtual vehicle is a full 2019 F-250
                    with real faults to find.
                  </div>
                  <button
                    className="primary"
                    style={{ marginTop: 8 }}
                    onClick={() => setTransport("simulator")}
                  >
                    Use the virtual vehicle instead
                  </button>
                </div>
              </div>
            )}
          </div>
        )}

        <div className="field">
          <label>Session label (optional)</label>
          <input
            type="text"
            value={label}
            placeholder="e.g. morning scan, cold start"
            onChange={(e) => setLabel(e.target.value)}
          />
        </div>

        <div className="row" style={{ justifyContent: "flex-end", marginTop: 16 }}>
          <button className="primary" disabled={!canConnect} onClick={() => void connect()}>
            {busy ? <Spinner label="Connecting" /> : "Connect"}
          </button>
        </div>
      </div>
    </div>
  );
}

function ProbeResults({ ports }: { ports: (SerialPortInfo | ProbedPort)[] }) {
  return (
    <div className="card" style={{ marginTop: 8 }}>
      <table>
        <thead>
          <tr><th>Port</th><th>Answered</th><th>Detail</th></tr>
        </thead>
        <tbody>
          {ports.map((p) => {
            const info = isProbed(p) ? p.port : p;
            const id = isProbed(p) ? p.identification : null;
            const err = isProbed(p) ? p.error : null;
            return (
              <tr key={info.name}>
                <td className="mono">{info.name}</td>
                <td>
                  {err ? <span className="tag">no</span>
                    : id?.elm327_compatible ? <span className="tag confirmed" style={{ color: "var(--ok)", borderColor: "#1e4a26", background: "#12251a" }}>ELM327</span>
                    : id?.responded ? <span className="tag">responded</span>
                    : <span className="tag">silent</span>}
                </td>
                <td className="faint">
                  {err ? `${err.code}: ${err.message}`
                    : id?.banner ?? id?.description ?? "no banner at any line speed"}
                  {id?.elapsed_ms != null && <span className="mono"> ({id.elapsed_ms} ms)</span>}
                  {/* Which speed a wired cable answered at. Worth showing: it
                      is the setting that most often makes a working adapter
                      look dead, and it is invisible everywhere else. */}
                  {id?.baud_rate != null && (
                    <span className="mono"> at {id.baud_rate} baud</span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function isProbed(p: SerialPortInfo | ProbedPort): p is ProbedPort {
  return typeof (p as ProbedPort).port === "object";
}
