// Types mirroring docs/API.md. The UI holds no vehicle logic; it renders what
// the core reports, including the parts the core says it is unsure about.

export type PermissionLevel = "L0" | "L1" | "L2" | "L3";
export type Severity = "info" | "caution" | "serious";
export type Verification = "verified" | "unverified" | (string & {});

/** `error.code` is a stable discriminant. Branch on it, never on `message`. */
export type ErrorCode =
  | "no_active_session" | "adapter_busy" | "no_data" | "vehicle_not_responding"
  | "adapter_rejected_command" | "adapter_error" | "adapter_not_identified"
  | "negative_response" | "iso_tp_error" | "transport_timeout"
  | "transport_not_found" | "transport_open_failed" | "decoder_not_found"
  | "decoder_input_invalid" | "operation_not_allowed" | "permission_level_disabled"
  | "confirmation_required" | "precondition_failed" | "capability_missing"
  | "not_found" | "bad_request" | "not_implemented" | "storage_error" | "internal"
  | (string & {});

export interface ApiError {
  code: ErrorCode;
  message: string;
  details?: Record<string, unknown> | null;
  capability_state?: AdapterCapabilities | null;
  provenance?: Provenance | null;
}

export interface Warning {
  code: string;
  message: string;
  severity: Severity;
}

export interface Provenance {
  source: string;
  raw_hex: string | null;
  decoder_id: string | null;
  decoder_version: string | null;
  verification: Verification;
  observed_at: string;
  evidence_ref: number | null;
}

export type DecodedScalar =
  | { type: "number"; value: number }
  | { type: "integer"; value: number }
  | { type: "boolean"; value: boolean }
  | { type: "text"; value: string }
  | { type: "raw"; value: string }
  | { type: "flags"; value: { id: string; label: string; set: boolean }[] };

export interface DecodedValue {
  signal_id: string;
  name: string;
  value: DecodedScalar;
  unit: string | null;
  valid_range: { min: number; max: number } | null;
  out_of_range: boolean;
  provenance: Provenance;
  timestamp: string;
}

/** The section-7 envelope. HTTP 200 only means the operation ran - check `success`. */
export interface ToolResult<D = unknown> {
  tool: string;
  timestamp: string;
  vehicle_session_id: string | null;
  module: string | null;
  success: boolean;
  values: DecodedValue[];
  data: D | null;
  raw_evidence_ref: number | null;
  warnings: Warning[];
  capability_used: string | null;
  execution_time_ms: number | null;
  error: ApiError | null;
}

export type ConnectionState =
  | { state: "disconnected" | "connecting" | "initializing" | "identifying" | "ready" }
  | { state: "degraded"; reason: string }
  | { state: "reconnecting"; attempt: number }
  | { state: "failed"; code: string; detail: string };

export interface AdapterCapabilities {
  transport: string;
  elm327_compatible: boolean;
  can_11_bit: boolean;
  can_29_bit: boolean;
  iso_tp: boolean;
  multiple_can_buses: boolean;
  j2534: boolean;
  supports_transmit: boolean;
  supports_long_messages: boolean;
  max_reliable_throughput: number | null;
  vendor: string | null;
  model: string | null;
  firmware: string | null;
  caveats: string[];
}

export interface AdapterHealth {
  state: ConnectionState;
  requests: number;
  responses: number;
  timeouts: number;
  no_data: number;
  adapter_errors: number;
  mean_latency_ms: number | null;
  battery_voltage: number | null;
  protocol: string | null;
}

export interface AdapterStatus {
  connected: boolean;
  state: ConnectionState;
  descriptor?: string | null;
  session_id?: string | null;
  health?: AdapterHealth | null;
  capabilities?: AdapterCapabilities | null;
  vehicle?: Vehicle | null;
}

export interface Scenario {
  id: string;
  description: string;
}

export interface Health {
  service: string;
  api_version: string;
  build_version: string;
  schema_version: number;
  database: string | null;
  bind: string;
  default_transport: string;
  default_scenario: string | null;
  scenarios: Scenario[];
  active: { session_id: string; state: ConnectionState; adapter: string } | null;
}

export interface SerialPortInfo {
  name: string;
  kind: string;
  vid: number | null;
  pid: number | null;
  serial_number: string | null;
  manufacturer: string | null;
  product: string | null;
  likely_obd_adapter: boolean;
}

export interface ProbedPort {
  port: SerialPortInfo;
  identification?: {
    descriptor: string;
    responded: boolean;
    banner: string | null;
    description: string | null;
    elm327_compatible: boolean;
    elapsed_ms: number;
    /** Line speed the device answered at. Null for Bluetooth, which ignores it. */
    baud_rate: number | null;
  } | null;
  error?: ApiError | null;
}

export interface PortsResponse {
  ports: (SerialPortInfo | ProbedPort)[];
  probed: boolean;
}

export interface Capability {
  id: string;
  description: string;
  level: PermissionLevel;
  enabled: boolean;
  mutating: boolean;
  preconditions: string[];
  required_adapter_flags: string[];
}

export interface ObservedConditions {
  ignition_on: boolean | null;
  engine_running: boolean | null;
  battery_voltage: number | null;
  connection_stable: boolean | null;
  vehicle_speed_kph: number | null;
}

export interface CapabilitiesResponse {
  max_enabled_level: PermissionLevel;
  capabilities: Capability[];
  observed_conditions: ObservedConditions | null;
}

export interface ModuleIdentity {
  ecu_name: string | null;
  calibration_ids: string[];
  calibration_verification_numbers: string[];
}

export interface ModuleRecord {
  id: string;
  session_id: string;
  module_key: string;
  name: string | null;
  address: string;
  protocol: string | null;
  identity: ModuleIdentity;
  software_version: string | null;
  discovered_at: string;
}

export interface SupportedPid {
  pid: number;
  hex: string;
  signal_id?: string | null;
  name?: string | null;
  unit?: string | null;
  verification?: Verification | null;
  decoder_available: boolean;
  /** How the payload is interpreted: numeric, bitfield, supported_pids, ... */
  kind?: string | null;
  /** False for masks and bitfields - real things, but not gauges. */
  is_measurement?: boolean;
}

export interface SignalsData {
  module: string;
  count: number;
  pids: SupportedPid[];
}

export type DtcStatus = "confirmed" | "pending" | "permanent";

/**
 * Roughly where on a vehicle something lives. A zone, never a point.
 *
 * `unknown` is a common and correct answer: it covers every code the standard
 * does not place, which is most manufacturer-specific ones. Nothing is drawn
 * for it — a diagram with no highlight invites somebody to read the absence as
 * "it is fine".
 */
export type Region =
  | "engine_bay" | "exhaust" | "fuel_system" | "transmission"
  | "cabin" | "wheels" | "electrical" | "unknown";

export interface Dtc {
  code: string;
  status: DtcStatus;
  module: string;
  description: string | null;
  structural_summary: string | null;
  verification: Verification;
  is_generic: boolean;
  /** Where it roughly lives, from the SAE subsystem ranges. Often unknown. */
  region: Region;
}

export interface DtcData {
  dtcs: Dtc[];
  confirmed_count: number;
  modules_read: string[];
}

export interface VinDecoded {
  vin: string;
  wmi: string;
  vds: string;
  vis: string;
  manufacturer: string | null;
  region: string | null;
  model_year: number | null;
  check_digit_valid: boolean;
}

export interface IdentifyData {
  vin: string;
  vehicle_id: string;
  vin_decoded: VinDecoded;
  reported_by: string | null;
  calibration_ids: string[];
  calibration_verification_numbers: string[];
}

export interface Vehicle {
  id: string;
  vin: string | null;
  make: string | null;
  model: string | null;
  year: number | null;
  trim: string | null;
  engine: string | null;
  transmission: string | null;
  discovered_at: string;
}

export interface ConnectData {
  connection_id: string;
  adapter: string;
  state: ConnectionState;
  protocol: string | null;
  protocol_label: string | null;
  capabilities: AdapterCapabilities;
}

export interface FreezeFrameData {
  module: string;
  frame: number;
  dtc: string | null;
  dtc_description: string | null;
  dtc_verification: Verification | null;
}

export type EventKind =
  | { kind: "session_started"; label: string | null }
  | { kind: "session_ended" }
  | { kind: "connection_state_changed"; from: string; to: ConnectionState }
  | { kind: "adapter_identified"; capabilities: AdapterCapabilities }
  | { kind: "adapter_request"; command: string }
  | { kind: "adapter_response"; command: string; lines: string[]; elapsed_ms: number; classification: string }
  | { kind: "adapter_failure"; command: string; error: ApiError }
  | { kind: "vehicle_identified"; vin: string; vehicle_id: string }
  | { kind: "module_discovered"; module_key: string; address: string }
  | { kind: "dtc_read"; module_key: string; code: string; status: DtcStatus }
  | { kind: "measurement_recorded"; module_key: string; signal_id: string; value: number | null; unit: string | null; raw_hex: string | null }
  | { kind: "safety_decision"; operation: string; level: PermissionLevel; allowed: boolean; initiator: string; confirmed: boolean; reason: string | null }
  | { kind: "tool_invoked"; tool: string; arguments: unknown; initiator: string }
  | { kind: "tool_completed"; tool: string; success: boolean; execution_time_ms: number | null; warnings: Warning[] }
  | { kind: "user_note"; text: string }
  | { kind: string; [k: string]: unknown };

export interface SessionEvent {
  id: number;
  session_id: string;
  seq: number;
  timestamp: string;
  kind: EventKind;
}

export interface SessionRecord {
  id: string;
  vehicle_id: string | null;
  started_at: string;
  ended_at: string | null;
  label: string | null;
}

export interface SessionSummary {
  session: SessionRecord;
  vehicle: Vehicle | null;
  event_count: number;
  module_count: number;
  dtc_count: number;
  measurement_count: number;
}

export interface Measurement {
  session_id: string;
  module_id: string;
  timestamp: string;
  signal_id: string;
  value: number | null;
  text_value: string | null;
  unit: string | null;
  raw_value: string | null;
}

/** Server to client frames on `/api/v1/live`. */
export type LiveFrame =
  | { type: "hello"; min_interval_ms: number; max_signals: number }
  | { type: "subscribed"; module: string; signals: string[]; interval_ms: number }
  | { type: "sample"; result: ToolResult }
  | { type: "unsubscribed" }
  | { type: "error"; error: ApiError };

/** Server to client frames on `/api/v1/sessions/{id}/stream`. */
export type StreamFrame =
  | { type: "event"; event: SessionEvent }
  | { type: "hello"; session_id: string; from_seq: number }
  | { type: "lagged"; missed: number; resume_after_seq: number }
  | { type: "error"; error: ApiError };

// ---------------------------------------------------------------- the agent

export type ProviderKindId = "anthropic" | "ollama" | "openai_compatible" | "xai";

/** How much the model should deliberate. Applied differently per provider. */
export type Speed = "quality" | "fast";

export interface ProviderView {
  id: string;
  kind: ProviderKindId;
  label: string;
  base_url: string | null;
  model: string;
  /** Whether a credential is stored. The key itself never crosses the wire. */
  has_key: boolean;
  /** Enough of the key to recognise it, never enough to use it. */
  key_hint: string | null;
  /** Where this provider's key is actually kept: operating_system, environment, plain_file, none. */
  key_source: "operating_system" | "environment" | "plain_file" | "none";
  /** That, in a sentence meant for a person. */
  key_source_explanation: string;
  speed: Speed;
  /** Step ceiling for one inspection, or null for the built-in default. */
  max_steps: number | null;
  /** Output-token ceiling per turn, or null for the default. */
  max_tokens: number | null;
  /** Ollama context window, or null for the default. */
  context_tokens: number | null;
  /** False when this kind of endpoint has no way to honour the speed setting. */
  speed_supported: boolean;
  /** False when a context window does not apply to this endpoint. */
  context_supported: boolean;
  selected: boolean;
}

export interface ProviderKindInfo {
  id: ProviderKindId;
  label: string;
  requires_key: boolean;
  default_base_url: string | null;
  help: string;
}

export interface ProvidersResponse {
  /** Whether the user owns this vehicle or is considering buying it. */
  purpose?: ScanPurpose;
  /** How direct the agent should be. */
  tone?: Tone;
  providers: ProviderView[];
  settings_path: string;
  storage_note: string;
  kinds: ProviderKindInfo[];
}

export interface ProbeResult {
  reachable: boolean;
  models?: string[];
  elapsed_ms?: number;
  detail?: string | null;
  error?: { code: string; message: string };
}

export interface AgentStatus {
  /** Whether the user owns this vehicle or is considering buying it. */
  purpose?: ScanPurpose;
  /** How direct the agent should be. */
  tone?: Tone;
  ready: boolean;
  provider: { id: string; label: string; model: string } | null;
  reason: string | null;
}

/** Where a claim came from. Drives how it is presented, and it always is. */
export type ClaimSource = "measured" | "catalog" | "model_knowledge";
export type FindingSeverity = "critical" | "serious" | "caution" | "info";
export type ReportVerdict = "walk_away" | "negotiate" | "looks_sound" | "inconclusive";

export interface CostRange {
  low: number;
  high: number;
  currency: string;
  basis: string;
}

export interface Finding {
  title: string;
  severity: FindingSeverity;
  plain_english: string;
  source: ClaimSource;
  evidence?: string[];
  evidence_refs?: number[];
  what_to_do?: string | null;
  estimated_cost?: CostRange | null;
}

export interface Report {
  verdict: ReportVerdict;
  headline: string;
  summary: string;
  findings: Finding[];
  watch_items?: Finding[];
  not_checked?: string[];
  next_steps?: string[];
}

export type TraceEntry =
  | { type: "thinking"; text: string }
  | { type: "tool"; name: string; arguments: unknown }
  | { type: "tool_done"; name: string; success: boolean; evidence_ref: number | null };

export interface AgentUsage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
}

export interface InspectResponse {
  report: Report | null;
  text: string;
  steps: number;
  truncated: boolean;
  usage: AgentUsage;
  trace: TraceEntry[];
}

/**
 * A question the assistant put to the person, with the answers to choose from.
 *
 * Only ever appears when the answer is something the vehicle cannot give:
 * what a dash menu shows, whether a noise happens cold or warm, whether the
 * key is in the car. Answering sends an ordinary message, so a button here is
 * a shortcut to typing and never an action taken on anyone's behalf.
 */
export interface AgentQuestion {
  question: string;
  options: string[];
  /** What the answer would let it work out. Optional. */
  why: string | null;
}

export interface ChatResponse {
  text: string;
  steps: number;
  truncated: boolean;
  usage: AgentUsage;
  trace: TraceEntry[];
  /** Null on almost every turn. */
  question: AgentQuestion | null;
}

/// One service 06 on-board monitor test result, as the core reports it.
export interface MonitorReading {
  mid: number;
  tid: number;
  name: string;
  system: string | null;
  /** Decided by limits the vehicle itself sent. Exact regardless of scaling. */
  passed: boolean;
  /** How close to the limit, 0 = at it, 1 = mid-band. Null for a flat band. */
  margin: number | null;
  value: number | null;
  min: number | null;
  max: number | null;
  unit: string | null;
  raw: { value: number; min: number; max: number; uasid: number };
  unknown_monitor: boolean;
  unknown_scaling: boolean;
}

export interface MonitorTestsData {
  module: string;
  /** False when the vehicle does not implement service 06 at all. */
  supported: boolean;
  monitors: MonitorReading[];
  failing?: number;
  marginal?: number;
  marginal_threshold?: number;
  scaling_verification?: Verification;
}

/** Which namespace an explanation lives in. */
export type ExplainKind = "signal" | "code" | "concept";

/** One thing explained at two reading levels. */
export interface Explanation {
  id: string;
  /** For someone with no mechanical background. */
  easy: string;
  /** For someone who could act on it. */
  technical: string;
}

export interface ExplanationsResponse {
  signals: Explanation[];
  codes: Explanation[];
  concepts: Explanation[];
}

export type FeatureSupport = "described_only" | "read_only" | "writable";

export interface FeatureView {
  id: string;
  name: string;
  easy: string;
  technical: string;
  risk: string;
  risk_label: string;
  modules: string[];
  requires: string[];
  support: FeatureSupport;
  verification: Verification;
  source: string | null;
  notes: string | null;
  /** False for classes this product refuses regardless of data. */
  writable_in_principle: boolean;
  /** How much this claim has to do with the vehicle in front of us. */
  authority: Authority;
  /** That, in language meant for a person. */
  authority_explanation: string;
  /**
   * Whether the mapping was measured on this vehicle rather than one like it.
   *
   * A false here is the difference between a fact and a hypothesis, and the
   * screen has to say which it is showing. The server refuses the write either
   * way — this exists so the person is told before they ask, not after.
   */
  measured_on_this_vehicle: boolean;
}

/** Where an answer came from, best first. Mirrors `aim_decoders::Authority`. */
export type Authority =
  | "measured_this_session"
  | "measured_earlier"
  | "owner_supplied_oem_data"
  | "measured_on_similar_vehicle"
  | "community_this_model"
  | "community_related_model"
  | "generic_standard"
  | "model_knowledge";

export interface FeaturesData {
  features: FeatureView[];
  catalog_size: number;
  narrowed_to_vehicle: boolean;
  /** How many of these were measured on a different, similar vehicle. */
  from_a_similar_vehicle: number;
}

export interface PlanCheck {
  id: string;
  question: string;
  passed: boolean;
  detail: string | null;
  /** True when failing is permanent for this build, not a fixable prerequisite. */
  blocking_by_design: boolean;
}

export interface ChangePlan {
  feature_id: string;
  feature_name: string | null;
  risk: string | null;
  desired: "on" | "off";
  checks: PlanCheck[];
  can_apply: boolean;
  blocked_reason: string | null;
  modules: string[];
  mapping_source: string | null;
}

/** One user-supplied profile file, and what it contributed. */
export interface ProfileLoad {
  path: string;
  source: string;
  kind: "features" | "pids" | "explanations" | "unrecognised";
  loaded: number;
  error: string | null;
}

export interface ProfilesResponse {
  report: { directory: string | null; files: ProfileLoad[] };
  feature_count: number;
  features_from_profiles: number;
}

export interface ClearResult {
  cleared_by: string[];
}

/** Readiness as reported by one module. */
export interface ModuleReadiness {
  module: string;
  address: string;
  name: string | null;
  values: DecodedValue[];
}

export interface ReadinessData {
  modules: ModuleReadiness[];
  module_count: number;
  /** Spread in distance-since-cleared between modules, when they disagree. */
  disagreement_km: number | null;
}

export type ScanPurpose = "owner" | "buyer";
export type Tone = "practical" | "neutral" | "blunt";

/** One fault as UDS reports it: three-byte code plus status bits. */
export interface UdsFault {
  code: string;
  base_code: string;
  description: string | null;
  structural_summary: string | null;
  is_generic: boolean | null;
  status: number;
  status_summary: string;
  failing_now: boolean;
  confirmed: boolean;
  warning_lamp: boolean;
}

export interface ScannedModule {
  request_address: string;
  address: string;
  name: string;
  /** True for the legislated emissions block a code reader can also see. */
  in_legislated_range: boolean;
  faults: UdsFault[];
  fault_count: number;
  /** Why this module produced no faults, when that needs explaining. */
  note: string | null;
}

export interface FullScanData {
  modules: ScannedModule[];
  module_count: number;
  fault_count: number;
  addresses_probed: number;
}

export type FaultChange = "appeared" | "gone" | "unchanged";

export interface FaultDelta {
  code: string;
  module: string;
  description: string | null;
  change: FaultChange;
}

export interface SignalDelta {
  signal_id: string;
  unit: string | null;
  before: number;
  after: number;
  delta: number;
  /** Null when the earlier reading was zero. */
  relative: number | null;
  samples_before: number;
  samples_after: number;
}

export interface SessionComparison {
  before: string;
  after: string;
  before_at: string;
  after_at: string;
  days_apart: number;
  faults: FaultDelta[];
  signals: SignalDelta[];
  /** Null when either scan did not read a VIN. */
  same_vehicle: boolean | null;
}

export interface ComparisonResponse {
  comparison: SessionComparison;
  notable_signals: string[];
}

/** What an update check found.
 *
 * `error` is separate from a failed request on purpose: a check that reached
 * GitHub and was refused (rate limit, no network) is a different thing from a
 * check that never ran, and neither should look like "you are up to date". */
export interface UpdateStatus {
  current: string;
  latest: string | null;
  update_available: boolean;
  notes: string | null;
  url: string | null;
  size: number | null;
  error: string | null;
}

/**
 * Whether a manufacturer as-built file is held for this vehicle.
 *
 * The file records how one vehicle was configured at the factory, for *every*
 * module on it — including the ones asleep right now and the ones on a bus the
 * adapter cannot reach. On one 2019 F-250 that was 29 modules against six
 * awake on the bus, which is why the app mentions it unprompted rather than
 * waiting to be asked.
 */
export interface AsBuiltStatus {
  held: boolean;
  vin: string | null;
  imported_at?: string | null;
  source?: string | null;
  modules_awake_on_the_bus?: number;
  what_it_would_add?: string;
  how_to_get_one?: string[];
  what_it_is_not?: string;
  /** Present instead of the above when no single VIN has been established. */
  why_not_yet?: string;
}

/** The result of importing one. */
export interface AsBuiltImport {
  imported: boolean;
  vin: string;
  modules: number;
  lines: number;
  /** How many of those modules are not answering — the reason to have the file. */
  modules_not_answering_on_the_bus: number;
  source: string | null;
}
