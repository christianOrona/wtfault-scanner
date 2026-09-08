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
  AdapterStatus, CapabilitiesResponse, ConnectData, DtcData, FreezeFrameData,
  Health, IdentifyData, Measurement, ModuleIdentity, ModuleRecord, PortsResponse,
  SessionEvent, SessionSummary, SignalsData, ToolResult, ApiError, Dtc, MonitorTestsData,
  AgentStatus, InspectResponse, ChatResponse as AgentChatResponse,
  ProvidersResponse, ProviderView, ProviderKindId, ProbeResult, Speed, ExplanationsResponse, FeaturesData, ChangePlan, ProfilesResponse, ClearResult, ReadinessData, ScanPurpose, Tone, FullScanData,
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

export const api = {
  health: () => request<Health>("/health"),
  capabilities: () => request<CapabilitiesResponse>("/capabilities"),
  explanations: () => request<ExplanationsResponse>("/explanations"),
  profiles: () => request<ProfilesResponse>("/profiles"),
  exportFile: (body: { filename: string; content: string }) =>
    post<{ path: string; directory: string; filename: string }>("/export", body),
  setVoice: (body: { purpose?: ScanPurpose; tone?: Tone }) =>
    post<{ purpose: ScanPurpose; tone: Tone }>("/settings/voice", body),
  features: () => request<ToolResult<FeaturesData>>("/features"),
  previewFeature: (id: string, desired: "on" | "off") =>
    post<ToolResult<ChangePlan>>(`/features/${enc(id)}/preview`, { desired }),

  ports: (probe = false) => request<PortsResponse>(`/adapters/ports${probe ? "?probe=true" : ""}`),
  adapter: () => request<AdapterStatus>("/adapter"),
  connect: (body: { transport?: string; port?: string; scenario?: string; label?: string }) =>
    post<ToolResult<ConnectData>>("/adapter/connect", body),
  disconnect: () => post<ToolResult<unknown>>("/adapter/disconnect"),

  identify: () => post<ToolResult<IdentifyData>>("/vehicles/identify"),

  scanModules: () => post<ToolResult<{ modules: ModuleRecord[] }>>("/modules"),
  modules: () => request<{ modules: ModuleRecord[] }>("/modules"),
  moduleIdentity: (key: string) =>
    request<ToolResult<{ module: ModuleRecord; identity: ModuleIdentity }>>(`/modules/${enc(key)}`),
  signals: (key: string) => request<ToolResult<SignalsData>>(`/modules/${enc(key)}/signals`),
  monitorTests: (key: string) =>
    request<ToolResult<MonitorTestsData>>(`/modules/${enc(key)}/monitor-tests`),
  dtcs: (key: string) => request<ToolResult<DtcData>>(`/modules/${enc(key)}/dtcs`),
  readiness: () => request<ToolResult<ReadinessData>>("/readiness"),
  scanAllModules: () => post<ToolResult<FullScanData>>("/modules/scan-all"),
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
