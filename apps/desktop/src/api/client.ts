// The one place that knows how to talk to the core.
//
// docs/API.md is explicit that there are exactly two answer shapes, and that
// confusing them is the easiest way to get this UI wrong:
//
//   * a vehicle operation that RAN -> HTTP 200 + ToolResult; check `success`
//   * a request that was WRONG     -> a real status code + error envelope
//
// So `request` throws only for the second kind. A truck that answers NO DATA is
// a diagnostic outcome with evidence attached, and it comes back as a value.

import type {
  Contribution,
  SendOutcome,
  ProcedureCheck, ProcedureInfo, ProcedureMeasurement,
  AdapterStatus, CapabilitiesResponse, ConnectData, DtcData, FreezeFrameData,
  Health, IdentifyData, Measurement, ModuleIdentity, ModuleRecord, PortsResponse,
  SessionEvent, SessionSummary, SignalsData, ToolResult, ApiError, Dtc, MonitorTestsData,
  VpicHeld, VpicLookup, ObdbStatus, ObdbFetch,
  AgentStatus, InspectResponse, ChatResponse as AgentChatResponse,
  ProvidersResponse, ProviderView, ProviderKindId, ProbeResult, Speed, ExplanationsResponse, FeaturesData, ChangePlan, ApplyResult, WriteGateResult, ProfilesResponse, ClearResult, ReadinessData, ScanPurpose, Tone, FullScanData, ComparisonResponse, UpdateStatus, DownloadState, ModuleProbeData, VehicleKnowledge, SupportReport, AsBuiltStatus, AsBuiltImport,
} from "./types";

/**
 * Base URL.
 *
 * Normally empty, which means every request is relative to whatever origin the
 * page was loaded from — and that is always the diagnostic core:
 *
 *  * under `vite dev`, the dev server proxies `/api` to it;
 *  * in the desktop app, the window is loaded from the core's own port and the
 *    core serves the UI as well as the API.
 *
 * Keeping it same-origin is deliberate. The alternative needs CORS headers on a
 * loopback server that can talk to a vehicle, which would let any web page the
 * user visits read the truck. The two overrides below exist for pointing a dev
 * UI at a core running somewhere else, and neither is used in a shipped build.
 */
declare global {
  interface Window {
    __AIM_API_ORIGIN__?: string;
  }
}

export const API_ORIGIN: string =
  window.__AIM_API_ORIGIN__ ??
  (import.meta as { env?: Record<string, string> }).env?.VITE_API_ORIGIN ??
  "";

export const API_BASE = `${API_ORIGIN}/api/v1`;

export function wsUrl(path: string): string {
  if (API_ORIGIN) return `${API_ORIGIN.replace(/^http/, "ws")}/api/v1${path}`;
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}/api/v1${path}`;
}

/** A request the server rejected structurally. Carries the stable `code`. */
export class RequestError extends Error {
  readonly status: number;
  readonly error: ApiError;
  constructor(status: number, error: ApiError) {
    super(error.message);
    this.name = "RequestError";
    this.status = status;
    this.error = error;
  }
  get code() {
    return this.error.code;
  }
}

/** The core is not reachable at all - a different problem from any of its errors. */
export class TransportError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TransportError";
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${API_BASE}${path}`, {
      ...init,
      headers: {
        ...(init?.body ? { "content-type": "application/json" } : {}),
        ...init?.headers,
      },
    });
  } catch (cause) {
    throw new TransportError(
      `cannot reach the diagnostic core at ${API_BASE || location.origin}. Is aim-api running?`,
    );
  }

  const text = await res.text();
  let body: unknown = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      throw new TransportError(`the core answered ${res.status} with a non-JSON body`);
    }
  }

  if (!res.ok) {
    const err = (body as { error?: ApiError } | null)?.error;
    throw new RequestError(
      res.status,
      err ?? { code: "internal", message: `HTTP ${res.status}`, details: null },
    );
  }
  return body as T;
}

const post = <T>(path: string, body?: unknown) =>
  request<T>(path, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body) });

/** Where an export went. On a phone it is handed to the share sheet
 *  (`shared`) and `path` is inside the app, which is no use to a person. */
export interface Exported {
  path: string;
  directory: string;
  filename: string;
  bytes: number;
  shared: boolean;
}

/** What importing another install's database added, or would add. */
export interface ImportSummary {
  /** False for a preview: nothing was kept. */
  committed: boolean;
  sessions_added: number;
  sessions_extended: number;
  sessions_already_here: number;
  sessions_skipped: { id: string; reason: string }[];
  vehicles_added: number;
  /** VINs of the sessions added or extended; null for one that never gave a VIN. */
  vehicles: (string | null)[];
  events_added: number;
  measurements_added: number;
  dtcs_added: number;
  captures_added: number;
  findings_added: number;
  findings_updated: number;
  findings_kept: number;
  vehicle_records_taken: number;
}

export const api = {
  health: () => request<Health>("/health"),
  capabilities: () => request<CapabilitiesResponse>("/capabilities"),
  explanations: () => request<ExplanationsResponse>("/explanations"),
  profiles: () => request<ProfilesResponse>("/profiles"),
  /** Ask the core whether a newer release exists. Never throws for "no": a
   *  failed check reports its own error rather than looking like "up to date". */
  updateCheck: () => request<UpdateStatus>("/update/check"),
  /** Start fetching the installer in the background. Asking twice is harmless. */
  updateDownload: () => post<DownloadState>("/update/download", {}),
  /** How far along that download is. */
  updateDownloadStatus: () => request<DownloadState>("/update/download"),
  /** Install what has been downloaded. The app closes and comes back updated. */
  updateApply: () =>
    post<{ started: boolean; installer: string; note: string }>("/update/apply", {}),
  /** What this application has established about a vehicle — including what it
   *  has ruled out, which is the expensive kind.
   *
   *  Readable with nothing plugged in: pass a VIN for a specific vehicle, or
   *  omit it for the connected one. With neither, the answer is the list of
   *  vehicles anything is known about. */
  vehicleKnowledge: (vin?: string) =>
    request<VehicleKnowledge>(vin ? `/vehicles/knowledge?vin=${enc(vin)}` : "/vehicles/knowledge"),
  /** What this machine knows about how the app is running, and how the last run
   *  ended. Reads local state; sends nothing anywhere. */
  /** The problem report. With `withhold`, VINs and the user's name are taken out. */
  supportReport: (withhold = false) =>
    request<SupportReport>(`/support/report${withhold ? "?withhold=true" : ""}`),
  /** Send the report text, exactly as shown, to the address this build has. */
  supportSend: (text: string) => post<SendOutcome>("/support/send", { text }),
  /** What the connected vehicle taught, ready to share. Sends nothing. */
  contribution: () => request<Contribution>("/vehicles/contribution"),
  /** Open the log folder in the desktop's own file manager. */
  supportReveal: () => post<{ opened: string }>("/support/reveal", {}),
  exportFile: (body: { filename: string; content: string }) => post<Exported>("/export", body),
  /** A copy of everything this install has recorded, as one database file. */
  exportDatabase: (filename: string) => post<Exported>("/export/database", { filename }),
  /** Merge a database exported by another install into this one. With
   *  `dryRun` the answer is what would be added and nothing is kept. */
  importDatabase: (file: Blob, dryRun: boolean) =>
    request<ImportSummary>(`/import/database${dryRun ? "?dry_run=true" : ""}`, {
      method: "POST",
      body: file,
      // Not JSON, and the core refuses anything else: see the route for why.
      headers: { "content-type": "application/octet-stream" },
    }),
  /** One session's adapter exchanges as a replay transcript, VIN anonymised. */
  exportTranscript: (id: string, filename: string) =>
    post<Exported>(`/sessions/${enc(id)}/transcript`, { filename }),
  setVoice: (body: { purpose?: ScanPurpose; tone?: Tone }) =>
    post<{ purpose: ScanPurpose; tone: Tone }>("/settings/voice", body),
  features: () => request<ToolResult<FeaturesData>>("/features"),
  previewFeature: (id: string, desired: "on" | "off") =>
    post<ToolResult<ChangePlan>>(`/features/${enc(id)}/preview`, { desired }),
  /** Ask the module that owns a feature whether it takes writes. Writes to an
   *  identifier the module has just said it does not have, so nothing lands. */
  probeFeatureGate: (id: string, confirmation: string) =>
    post<ToolResult<WriteGateResult>>(`/features/${enc(id)}/write-gate`, { confirmation }),
  /** Change a setting. The confirmation is recorded verbatim in the audit trail. */
  applyFeature: (id: string, desired: "on" | "off", confirmation: string) =>
    post<ToolResult<ApplyResult>>(`/features/${enc(id)}/apply`, { desired, confirmation }),

  ports: (probe = false) => request<PortsResponse>(`/adapters/ports${probe ? "?probe=true" : ""}`),
  adapter: () => request<AdapterStatus>("/adapter"),
  connect: (body: { transport?: string; port?: string; scenario?: string; vehicle?: string; label?: string }) =>
    post<ToolResult<ConnectData>>("/adapter/connect", body),
  disconnect: () => post<ToolResult<unknown>>("/adapter/disconnect"),

  identify: () => post<ToolResult<IdentifyData>>("/vehicles/identify"),

  /** The NHTSA vPIC reply kept for the connected vehicle. Never sends anything. */
  vpicStatus: () => request<{ cached: VpicHeld | null }>("/vehicles/vpic"),
  /** Look the connected vehicle's VIN up with NHTSA vPIC. Sends the VIN unless a reply is kept. */
  lookupVpic: (refresh = false) => post<VpicLookup>("/vehicles/vpic", { refresh }),
  /** Which OBDb signal set belongs to the connected vehicle. Never sends anything. */
  obdbStatus: () => request<ObdbStatus>("/vehicles/obdb"),
  /** Fetch the connected vehicle's OBDb signal set from GitHub, unless a copy is kept. */
  fetchObdb: (refresh = false) => post<ObdbFetch>("/vehicles/obdb", { refresh }),
  /** Whether an as-built file is held for this vehicle, and how to get one. */
  asBuiltStatus: () => request<ToolResult<AsBuiltStatus>>("/vehicles/as-built"),
  /** Import one. Refused unless its VIN is the connected vehicle's. */
  importAsBuilt: (text: string, source?: string) =>
    post<ToolResult<AsBuiltImport>>("/vehicles/as-built", { text, source }),
  /** Forget it. It names somebody's vehicle, so removing it is one click. */
  forgetAsBuilt: () =>
    request<ToolResult<{ removed: boolean; vin: string }>>("/vehicles/as-built", {
      method: "DELETE",
    }),

  scanModules: () => post<ToolResult<{ modules: ModuleRecord[] }>>("/modules"),
  modules: () => request<{ modules: ModuleRecord[] }>("/modules"),
  moduleIdentity: (key: string) =>
    request<ToolResult<{ module: ModuleRecord; identity: ModuleIdentity }>>(`/modules/${enc(key)}`),
  signals: (key: string) => request<ToolResult<SignalsData>>(`/modules/${enc(key)}/signals`),
  monitorTests: (key: string) =>
    request<ToolResult<MonitorTestsData>>(`/modules/${enc(key)}/monitor-tests`),
  dtcs: (key: string) => request<ToolResult<DtcData>>(`/modules/${enc(key)}/dtcs`),
  /** Read-only: which sessions, security and identifiers a module answers. */
  probeModule: (key: string) =>
    request<ToolResult<ModuleProbeData>>(`/modules/${enc(key)}/capabilities`),
  readiness: () => request<ToolResult<ReadinessData>>("/readiness"),
  scanAllModules: () => post<ToolResult<FullScanData>>("/modules/scan-all"),
  /** The guided tests this build ships. Touches no vehicle. */
  procedures: () => request<{ procedures: ProcedureInfo[] }>("/procedures"),
  /** Read whether the vehicle is in the state a test needs. Read-only. */
  checkProcedure: (id: string) => request<ToolResult<ProcedureCheck>>(`/procedures/${enc(id)}`),
  /** Take the test's readings, once its conditions hold. Read-only. */
  runProcedure: (id: string) =>
    post<ToolResult<ProcedureMeasurement>>(`/procedures/${enc(id)}/run`),
  compareSessions: (before: string, after: string) =>
    request<ComparisonResponse>(`/sessions/${enc(before)}/compare/${enc(after)}`),
  clearDtcs: (confirmation: string, module?: string) =>
    post<ToolResult<ClearResult>>("/dtcs/clear", { confirmation, module }),
  freezeFrame: (key: string, frame = 0) =>
    request<ToolResult<FreezeFrameData>>(`/modules/${enc(key)}/freeze-frame?frame=${frame}`),
  read: (key: string, signals: string[]) =>
    post<ToolResult<{ module: string; requested: string[] }>>(`/modules/${enc(key)}/read`, { signals }),

  sessions: (limit = 50) => request<{ sessions: SessionSummary[] }>(`/sessions?limit=${limit}`),
  session: (id: string) => request<SessionDetail>(`/sessions/${enc(id)}`),
  events: (id: string, afterSeq = 0, limit = 500) =>
    request<{ events: SessionEvent[]; after_seq: number; total: number }>(
      `/sessions/${enc(id)}/events?after_seq=${afterSeq}&limit=${limit}`,
    ),
  sessionModules: (id: string) => request<{ modules: ModuleRecord[] }>(`/sessions/${enc(id)}/modules`),
  sessionDtcs: (id: string) => request<{ dtcs: StoredDtc[] }>(`/sessions/${enc(id)}/dtcs`),
  measurements: (id: string, signal?: string, limit = 500) =>
    request<{ measurements: Measurement[] }>(
      `/sessions/${enc(id)}/measurements?limit=${limit}${signal ? `&signal=${enc(signal)}` : ""}`,
    ),

  // ---- the agent ----

  agentStatus: () => request<AgentStatus>("/agent"),

  /**
   * A full inspection. Minutes, not seconds: every step is a model turn plus
   * real adapter round trips, so this uses no client-side timeout and the UI
   * shows progress instead.
   */
  inspect: (userRequest?: string) =>
    post<InspectResponse>("/agent/inspect", { request: userRequest ?? null }),

  ask: (messages: { role: "user" | "assistant"; content: string }[]) =>
    post<AgentChatResponse>("/agent/messages", { messages }),

  // ---- providers ----

  providers: () => request<ProvidersResponse>("/settings/providers"),
  addProvider: (body: ProviderInput) => post<{ providers: ProviderView[] }>("/settings/providers", body),
  updateProvider: (id: string, body: ProviderInput) =>
    request<{ providers: ProviderView[] }>(`/settings/providers/${enc(id)}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),
  deleteProvider: (id: string) =>
    request<{ providers: ProviderView[] }>(`/settings/providers/${enc(id)}`, { method: "DELETE" }),
  selectProvider: (id: string) =>
    post<{ providers: ProviderView[] }>(`/settings/providers/${enc(id)}/select`),
  testProvider: (id: string) => post<ProbeResult>(`/settings/providers/${enc(id)}/test`),
};

/** Creating or editing a provider. */
export interface ProviderInput {
  kind: ProviderKindId;
  label: string;
  base_url?: string | null;
  model: string;
  /** Omit to keep the stored key; send `""` to clear it. */
  api_key?: string;
  speed?: Speed;
  max_steps?: number | null;
  max_tokens?: number | null;
  context_tokens?: number | null;
  select?: boolean;
}

const enc = encodeURIComponent;

export interface StoredDtc extends Omit<Dtc, "module"> {
  session_id: string;
  module_id: string;
  occurrence: number;
  freeze_frame_ref: number | null;
  read_at: string;
}

export interface SessionDetail {
  session: SessionSummary["session"];
  vehicle: SessionSummary["vehicle"];
  connections: {
    id: string;
    adapter_id: string;
    transport: string;
    connected_at: string;
    disconnected_at: string | null;
    firmware: string | null;
  }[];
  modules: ModuleRecord[];
  dtcs: StoredDtc[];
  test_runs: unknown[];
  diagnoses: unknown[];
  agent_traces: unknown[];
  event_count: number;
}

/**
 * Turn anything thrown by this module into something renderable.
 * A ToolResult failure is not thrown - it is a value - so it is not handled here.
 */
export function describeError(e: unknown): { code: string; message: string } {
  if (e instanceof RequestError) return { code: e.error.code, message: e.error.message };
  if (e instanceof TransportError) return { code: "unreachable", message: e.message };
  return { code: "internal", message: e instanceof Error ? e.message : String(e) };
}
