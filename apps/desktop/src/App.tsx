import { useCallback, useEffect, useState } from "react";
import { api, describeError } from "./api/client";
import type {
  AdapterStatus, AgentStatus, Health, IdentifyData, ModuleRecord, ToolResult,
} from "./api/types";
import { ConnectDialog } from "./components/ConnectDialog";
import { CodesPane } from "./components/CodesPane";
import { LivePane } from "./components/LivePane";
import { AdapterPane } from "./components/AdapterPane";
import { RecorderPane } from "./components/RecorderPane";
import { SessionsPane } from "./components/SessionsPane";
import { InspectPane } from "./components/InspectPane";
import { AskPane } from "./components/AskPane";
import { SettingsPane } from "./components/SettingsPane";
import { FeaturesPane } from "./components/FeaturesPane";
import { ErrorBanner, FailedResult, Pill, Spinner, Warnings } from "./components/primitives";
import { useFlightRecorder } from "./hooks/useFlightRecorder";
import { ExplainToggle } from "./explain";

type Tab = "inspect" | "ask" | "codes" | "live" | "features" | "adapter" | "recorder" | "sessions" | "settings";

/**
 * The tab bar.
 *
 * `hint` is the one-line answer to "what is behind this word?", shown on hover
 * at either reading level. Navigation is the first thing a new user meets and
 * it was the one part of the screen with nothing explaining it — a row of nouns
 * with no way to find out which one holds what you want.
 */
const TABS: { id: Tab; label: string; hint: string }[] = [
  // The two agent tabs lead: the plain-English answer is the product, and the
  // instrument panes are the evidence behind it.
  { id: "inspect", label: "Inspect", hint: "Let the assistant check the whole car and write you a report." },
  { id: "ask", label: "Ask", hint: "Ask a question about this car in your own words." },
  { id: "codes", label: "Codes", hint: "Fault codes the car has stored, and what each one means." },
  { id: "live", label: "Live data", hint: "Watch the car's sensors move while the engine runs." },
  { id: "features", label: "Settings on the car", hint: "Things this car can be configured to do, and how far this app can go." },
  { id: "adapter", label: "Adapter", hint: "The box plugged into the car: what it is and how it is doing." },
  { id: "recorder", label: "Flight recorder", hint: "Every question asked and every answer given, in order. The proof." },
  { id: "sessions", label: "Sessions", hint: "Past visits to this and other cars, saved so you can look again." },
  { id: "settings", label: "Settings", hint: "Which AI model to use, and how hard it should think." },
];

export default function App() {
  const [health, setHealth] = useState<Health | null>(null);
  const [adapter, setAdapter] = useState<AdapterStatus | null>(null);
  const [error, setError] = useState<{ code: string; message: string } | null>(null);
  const [booted, setBooted] = useState(false);
  /** True while the core is merely still starting, rather than actually absent. */
  const [suppressBootError, setSuppressBootError] = useState(true);

  const [identify, setIdentify] = useState<ToolResult<IdentifyData> | null>(null);
  const [modules, setModules] = useState<ModuleRecord[]>([]);
  const [scan, setScan] = useState<ToolResult<{ modules: ModuleRecord[] }> | null>(null);
  const [selectedModule, setSelectedModule] = useState<string | null>(null);

  const [tab, setTab] = useState<Tab>("inspect");
  const [agent, setAgent] = useState<AgentStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [evidenceRef, setEvidenceRef] = useState<number | null>(null);
  /** True once the connect dialog has been dismissed to browse history. */
  const [browsing, setBrowsing] = useState(false);

  const sessionId = adapter?.session_id ?? null;
  const recorder = useFlightRecorder(sessionId, true);

  const refresh = useCallback(async () => {
    try {
      const [h, a] = await Promise.all([api.health(), api.adapter()]);
      setHealth(h);
      setAdapter(a);
      setError(null);
      return a;
    } catch (e) {
      setError(describeError(e));
      setAdapter(null);
      return null;
    } finally {
      setBooted(true);
    }
  }, []);

  // On boot the core may already have a live session - the server outlives this
  // window, and starting the UI a second time must not look like an empty
  // truck. Adopt what is already there instead of re-scanning: a rescan costs
  // real adapter round trips, and the stored list is the same data.
  //
  // The core may also not be up yet. In the desktop app the window and the
  // server start together and the window usually wins the race; giving up on
  // the first refused connection would show "cannot reach the diagnostic core"
  // about a core that arrives a second later. So keep trying, and only report
  // it once it has actually failed for a while.
  useEffect(() => {
    let cancelled = false;
    let attempt = 0;

    const tick = async () => {
      if (cancelled) return;
      const a = await refresh();
      if (cancelled) return;

      if (!a) {
        attempt += 1;
        // Stay quiet for the first few seconds, then say what is wrong.
        setSuppressBootError(attempt < 8);
        setTimeout(() => void tick(), 1000);
        return;
      }
      setSuppressBootError(false);

      if (!a.connected) return;
      try {
        const res = await api.modules();
        if (cancelled) return;
        setModules(res.modules);
        setSelectedModule((cur) => cur ?? res.modules[0]?.module_key ?? null);
      } catch {
        /* No stored modules yet; Rescan is one click away in the header. */
      }
    };

    void tick();
    return () => { cancelled = true; };
  }, [refresh]);

  /** Re-read what the agent may have discovered while it worked. */
  const refreshAfterAgent = useCallback(async () => {
    const a = await refresh();
    if (!a?.connected) return;
    try {
      const res = await api.modules();
      setModules(res.modules);
      setSelectedModule((cur) => cur ?? res.modules[0]?.module_key ?? null);
    } catch {
      /* Nothing stored yet; the sidebar stays as it was. */
    }
  }, [refresh]);

  const refreshAgent = useCallback(() => {
    api.agentStatus().then(setAgent).catch(() => setAgent(null));
  }, []);

  useEffect(() => { refreshAgent(); }, [refreshAgent]);

  // Poll only while connected: this keeps the header state and the traffic
  // counters honest without hammering an idle server.
  useEffect(() => {
    if (!adapter?.connected) return;
    const t = setInterval(() => { api.adapter().then(setAdapter).catch(() => {}); }, 3000);
    return () => clearInterval(t);
  }, [adapter?.connected]);

  /** After connecting: identify the vehicle, then scan for modules. */
  const runInitialScan = useCallback(async () => {
    await refresh();
    setBusy("Reading the VIN");
    try {
      setIdentify(await api.identify());
    } catch (e) {
      setError(describeError(e));
    }
    setBusy("Scanning for modules");
    try {
      const res = await api.scanModules();
      setScan(res);
      const found = res.data?.modules ?? [];
      setModules(found);
      setSelectedModule(found[0]?.module_key ?? null);
    } catch (e) {
      setError(describeError(e));
    }
    setBusy(null);
  }, [refresh]);

  async function rescan() {
    setBusy("Scanning for modules");
    try {
      const res = await api.scanModules();
      setScan(res);
      const found = res.data?.modules ?? [];
      setModules(found);
      if (!found.some((m) => m.module_key === selectedModule)) {
        setSelectedModule(found[0]?.module_key ?? null);
      }
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  }

  async function disconnect() {
    setBusy("Disconnecting");
    try {
      await api.disconnect();
    } catch (e) {
      setError(describeError(e));
    }
    setIdentify(null);
    setScan(null);
    setModules([]);
    setSelectedModule(null);
    setBusy(null);
    await refresh();
  }

  /** Jump from any value or failure to the adapter exchange behind it. */
  const showEvidence = useCallback((ref: number) => {
    setEvidenceRef(ref);
    setTab("recorder");
  }, []);

  const connected = !!adapter?.connected;
  const vin = identify?.data?.vin ?? adapter?.vehicle?.vin ?? null;
  const vinDecoded = identify?.data?.vin_decoded;

  return (
    <div className="app">
      <div className="topbar">
        <span className="brand-mark" aria-hidden="true" />
        <span className="brand">Wrench<span className="brand-gpt">GPT</span></span>
        {adapter && <Pill state={adapter.state} />}
        {adapter?.descriptor && <span className="faint mono">{adapter.descriptor}</span>}
        {vin && (
          <span className="row" style={{ gap: 6 }}>
            <span className="faint">VIN</span>
            <span className="mono">{vin}</span>
            {vinDecoded && (
              <span className="faint">
                {[vinDecoded.manufacturer, vinDecoded.model_year].filter(Boolean).join(" - ")}
              </span>
            )}
          </span>
        )}
        {busy && <Spinner label={busy} />}
        <span className="spacer" />
        <ExplainToggle />
        {/* Dismissing the connect dialog to read history must not strand the
            user with no way back to a vehicle. */}
        {!connected && browsing && (
          <button className="primary" onClick={() => setBrowsing(false)}>
            Connect to a vehicle
          </button>
        )}
        {connected && (
          <>
            <button onClick={() => void rescan()} disabled={!!busy}>Rescan</button>
            <button className="danger" onClick={() => void disconnect()} disabled={!!busy}>
              Disconnect
            </button>
          </>
        )}
      </div>

      <div className="body">
        {connected && (
          <div className="sidebar">
            <div className="section">
              <h2>Modules ({modules.length})</h2>
              <div className="list">
                {modules.map((m) => (
                  <button
                    key={m.id}
                    className="list-item"
                    aria-selected={m.module_key === selectedModule}
                    onClick={() => setSelectedModule(m.module_key)}
                  >
                    <span className="li-title">{m.name ?? `OBD module at ${m.address}`}</span>
                    <span className="li-sub">{m.module_key} - {m.address}</span>
                  </button>
                ))}
                {!modules.length && !busy && (
                  <span className="faint">No modules found. Try Rescan.</span>
                )}
              </div>
            </div>

            {vinDecoded && (
              <div className="section">
                <h2>Vehicle</h2>
                <div className="card" style={{ fontSize: 12 }}>
                  <div className="mono">{vinDecoded.vin}</div>
                  <div className="faint" style={{ marginTop: 6 }}>
                    {vinDecoded.manufacturer ?? "manufacturer not decoded"}
                    <br />
                    {vinDecoded.region ?? "region not decoded"}
                    <br />
                    model year {vinDecoded.model_year ?? "unknown"}
                  </div>
                  {!vinDecoded.check_digit_valid && (
                    <div className="banner serious" style={{ marginTop: 8, marginBottom: 0 }}>
                      <span className="b-code">check digit</span>
                      <span>This VIN fails its own SAE check digit.</span>
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>
        )}

        <div className="main">
          {connected && (
            <div className="tabs" role="tablist">
              {TABS.map((t) => (
                <button
                  key={t.id}
                  role="tab"
                  aria-selected={tab === t.id}
                  title={t.hint}
                  onClick={() => setTab(t.id)}
                >
                  {t.label}
                </button>
              ))}
            </div>
          )}

          {!connected ? (
            <div className="pane">
              {!suppressBootError && <ErrorBanner error={error} />}
              {suppressBootError && error && <Spinner label="Starting the diagnostic core" />}
              {booted && !error && <SessionsPane health={health} />}
            </div>
          ) : (
            <>
              {(error || identify?.success === false || scan?.success === false ||
                !!identify?.warnings.length || !!scan?.warnings.length ||
                adapter.state.state === "degraded") && (
                // Bounded: a connect that produced a wall of warnings must not
                // push the pane it is warning about off the screen.
                <div style={{ padding: "12px 16px 0", maxHeight: "40%", overflowY: "auto", flex: "0 0 auto" }}>
                  <ErrorBanner error={error} />
                  {adapter.state.state === "degraded" && (
                    <div className="banner serious">
                      <span className="b-code">adapter_degraded</span>
                      <span>
                        The adapter is working; the vehicle is not answering. Check the key is on
                        and the adapter is seated. Reads will fail with vehicle_not_responding until
                        it does.
                      </span>
                    </div>
                  )}
                  {identify && !identify.success && <FailedResult result={identify} onEvidence={showEvidence} />}
                  {identify && <Warnings warnings={identify.warnings} />}
                  {scan && !scan.success && <FailedResult result={scan} onEvidence={showEvidence} />}
                  {scan && <Warnings warnings={scan.warnings} />}
                </div>
              )}

              {/* The two agent panes stay mounted and are hidden when
                  inactive, unlike every other tab.

                  An inspection runs for minutes. Unmounting on a tab switch
                  threw away the running state and the finished report, while
                  the request carried on in the background with nobody left to
                  receive it — so looking at the Codes tab mid-scan silently
                  lost the scan. The instrument panes below are cheap to
                  rebuild and hold nothing worth keeping, so they still unmount;
                  these two do not. */}
              <div className="tab-panel" hidden={tab !== "inspect"}>
                <InspectPane
                  agent={agent}
                  connected={connected}
                  sessionId={sessionId}
                  vin={vin}
                  descriptor={adapter?.descriptor ?? null}
                  moduleKey={selectedModule}
                  onEvidence={showEvidence}
                  onOpenSettings={() => setTab("settings")}
                  onFinished={refreshAfterAgent}
                />
              </div>
              <div className="tab-panel" hidden={tab !== "ask"}>
                <AskPane
                  agent={agent}
                  connected={connected}
                  onEvidence={showEvidence}
                  onOpenSettings={() => setTab("settings")}
                  onFinished={refreshAfterAgent}
                />
              </div>
              {tab === "settings" && <SettingsPane onChanged={refreshAgent} />}
              {tab === "codes" && <CodesPane moduleKey={selectedModule} onEvidence={showEvidence} />}
              {tab === "live" && <LivePane moduleKey={selectedModule} onEvidence={showEvidence} />}
              {tab === "features" && <FeaturesPane connected={connected} />}
              {tab === "adapter" && <AdapterPane adapter={adapter} />}
              {tab === "recorder" && (
                <RecorderPane
                  events={recorder.events}
                  live={recorder.live}
                  socket={recorder.socket}
                  recovering={recorder.recovering}
                  highlight={evidenceRef}
                  onHighlightShown={() => setEvidenceRef(null)}
                />
              )}
              {tab === "sessions" && <SessionsPane health={health} />}
            </>
          )}
        </div>
      </div>

      {booted && !connected && !browsing && (
        <ConnectDialog
          health={health}
          onConnected={() => { setBrowsing(false); void runInitialScan(); }}
          onDismiss={() => setBrowsing(true)}
        />
      )}
    </div>
  );
}
