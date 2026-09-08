# AI Mechanic — Localhost API (`/api/v1`)

The Rust diagnostic core exposes a versioned HTTP + WebSocket API on
**127.0.0.1 only**. The Tauri/React shell, and any future mobile client, are
clients of this contract and hold no vehicle logic of their own.

Every example below was captured from a running server against the simulator.

```bash
# start the server against the virtual 2019 F-250
cargo run -p aim-api -- --simulator --scenario dpf-regen
# → http://127.0.0.1:8787
```

- [Conventions](#conventions)
- [Core envelopes](#core-envelopes)
- [Endpoints](#endpoints)
- [WebSockets](#websockets)
- [Typical UI flow](#typical-ui-flow)
- [Not in this build](#not-in-this-build)

---

## Conventions

**Base URL** `http://127.0.0.1:8787/api/v1` (port configurable with `--port`).

**No authentication.** The server binds to loopback and can talk to a vehicle.
It is not meant to be reachable from a network, and adding a token would imply
otherwise. Do not proxy it.

**Content type** is `application/json` in both directions. Request bodies are
optional where marked; `POST` with no body behaves as `{}`.

**Timestamps** are RFC 3339 UTC strings, e.g. `2026-09-06T08:21:13.392280635Z`.

### Two answer shapes

This is the single most important thing to get right in the UI.

| Situation | HTTP status | Body |
|---|---|---|
| A vehicle operation **ran** — succeeded *or* failed | `200` | [`ToolResult`](#toolresult) — check `success` |
| The request itself was wrong or impossible | real status (`400`, `404`, `409`, `501`, …) | [`Error envelope`](#error-envelope) |

A truck answering `NO DATA`, an ECU sending a negative response, a refused
`clear_dtcs` — these are **diagnostic outcomes with evidence attached**, not
HTTP failures. They come back `200` with `success: false` and a structured
`error` *inside* the envelope, alongside the warnings and the evidence pointer.

"Nothing is connected", "no such session", "you sent an empty signal list" are
wrong requests, and get real status codes.

So the UI needs exactly two handlers:

```ts
const res = await fetch(url, init);
const body = await res.json();

if (!res.ok) {
  // body.error — a structural problem with the request
  return showRequestError(body.error);
}
if ("success" in body && !body.success) {
  // body.error — the vehicle or the safety gate said no. Still render
  // body.warnings and body.raw_evidence_ref: this is diagnostic evidence.
  return showDiagnosticFailure(body);
}
return render(body);
```

### Error codes

`error.code` is a stable snake_case discriminant. Branch on it; never parse
`error.message`, which is developer-facing and may change.

| Code | Status | Meaning |
|---|---|---|
| `no_active_session` | 409 | Nothing connected. Call `POST /adapter/connect`. |
| `adapter_busy` | 409 | Already connected; disconnect first. |
| `no_data` | 502 | Request was valid, nothing answered it. |
| `vehicle_not_responding` | 502 | Adapter is fine, the vehicle is silent. |
| `adapter_rejected_command` | 502 | The adapter answered `?`. |
| `adapter_error` | 502 | `BUS ERROR`, `BUFFER FULL`, `STOPPED`, … |
| `adapter_not_identified` | 502 | The device is not ELM327-compatible. |
| `negative_response` | 502 | The ECU refused (service `0x7F`); `details.nrc` has the code. |
| `iso_tp_error` | 502 | Multi-frame reassembly failed, e.g. a truncated response. |
| `transport_timeout` | 504 | Deadline expired. |
| `transport_not_found` | 404 | No such port. |
| `transport_open_failed` | 502 | Port exists but will not open (in use, unpaired). |
| `decoder_not_found` | 404 | No decoder definition for that signal. |
| `decoder_input_invalid` | 400 | Payload was the wrong shape for the decoder. |
| `operation_not_allowed` | 403 | Not in the capability allowlist. Fail-closed. |
| `permission_level_disabled` | 403 | Exists but its level is off in this build (L2/L3). |
| `confirmation_required` | 403 | L1 operation needs a human confirmation. |
| `precondition_failed` | 409 | A declared precondition is not met. |
| `capability_missing` | 409 | The connected adapter cannot do this. |
| `not_found` | 404 | No such session/module/event. |
| `bad_request` | 400 | Malformed request. |
| `not_implemented` | 501 | A documented seam that is not built yet. |
| `storage_error`, `internal` | 500 | Server-side fault. |

---

## Core envelopes

### Error envelope

```json
{
  "error": {
    "code": "no_data",
    "message": "adapter answered \"01FE\" with no_data",
    "details": { "command": "01FE", "classification": "no_data", "lines": ["NO DATA"] },
    "capability_state": { "elm327_compatible": true, "can_29_bit": true, "...": "..." },
    "provenance": { "raw_hex": "410c", "decoder_id": "obd2.mode01.pid0C", "...": "..." }
  }
}
```

`details`, `capability_state` and `provenance` are omitted when not applicable.
`capability_state` is present on adapter failures — it is what the adapter was
*observed* to be at the moment it failed, which is often the explanation.

### ToolResult

Returned by every endpoint that talks to the vehicle. This is the §7 envelope
from the handoff, and the future agent runtime receives exactly the same shape.

```json
{
  "tool": "read_live_data",
  "timestamp": "2026-09-06T08:21:13.392280635Z",
  "vehicle_session_id": "ses_be7a0a7df3d0444fab7911cb968bd0bf",
  "module": "ECU_7E8",
  "success": true,
  "values": [ /* DecodedValue[] */ ],
  "data": { "module": "ECU_7E8", "requested": ["engine_rpm"] },
  "raw_evidence_ref": 84,
  "warnings": [],
  "capability_used": "obd2.read_live_data",
  "execution_time_ms": 8,
  "error": null
}
```

| Field | Notes |
|---|---|
| `success` | **Check this.** `200` only means the operation ran. |
| `values` | Decoded readings. Empty for operations that return structured data instead. |
| `data` | Operation-specific payload; the shape is documented per endpoint. |
| `raw_evidence_ref` | Row id in the session event log. `GET /sessions/{id}/events` resolves it to the exact adapter exchange. Present on failures too. |
| `warnings` | See below. Render these — several are safety-relevant. |
| `capability_used` | The safety capability this ran under. |
| `error` | Present only when `success` is false. Same shape as the error envelope's `error`. |

### DecodedValue

Raw bytes and user-facing meaning are never mixed. Every reading carries the
bytes it came from and which decoder produced it.

```json
{
  "signal_id": "engine_rpm",
  "name": "Engine RPM",
  "value": { "type": "number", "value": 1093.0 },
  "unit": "rpm",
  "valid_range": { "min": 0.0, "max": 16383.75 },
  "out_of_range": false,
  "provenance": {
    "source": "decoder",
    "raw_hex": "1114",
    "decoder_id": "obd2.mode01.pid0C",
    "decoder_version": "3",
    "verification": "verified",
    "observed_at": "2026-09-06T08:21:13.39212749Z",
    "evidence_ref": 84
  },
  "timestamp": "2026-09-06T08:21:13.39212749Z"
}
```

`value` is a tagged union — switch on `value.type`:

| `type` | `value` | Example |
|---|---|---|
| `number` | float | `1093.0` |
| `integer` | int | `41` |
| `boolean` | bool | `true` |
| `text` | string | `"1FT7W2BT6KEC00001"` |
| `raw` | hex string | `"1a2b3c4d"` |
| `flags` | `[{ "id", "label", "set" }]` | monitor status bits |

**`provenance.verification` drives presentation.** `verified` means the decoder
definition has been validated. `unverified` means it has **not** — the diesel
particulate-filter and exhaust-gas-temperature PIDs are unverified today. Show
unverified values as raw evidence, clearly marked, and never as a measured
fact. `out_of_range: true` means the value fell outside its declared range and
should be shown as suspect.

### Warning

```json
{ "code": "unverified_decoder", "message": "decoder definitions not validated for: dpf_temp_bank1_inlet", "severity": "caution" }
```

`severity` is `info` | `caution` | `serious`. Codes you will see:

| Code | Severity | Meaning |
|---|---|---|
| `unverified_decoder` | caution | A value came from an unvalidated definition. |
| `value_out_of_range` | serious | A decoded value is outside its declared range. |
| `adapter_caveat` | caution | An observed adapter limitation (one per caveat). |
| `adapter_degraded` | serious | Connected, but the vehicle is not answering. |
| `dtc_not_in_catalog` | caution | A code decoded structurally but has no description. |
| `dtc_count_mismatch` | caution | The ECU's claimed code count disagrees with what it sent. |
| `unknown_signal` / `signal_unavailable` | caution | A requested signal could not be read. |
| `pids_without_decoders` | info | The module supports PIDs this build cannot decode. |
| `identity_field_unavailable` | info | A module did not report part of its identity. |
| `vin_check_digit_invalid` | serious | The VIN failed its own SAE check digit. |

---

## Endpoints

### Server

#### `GET /health`

Server identity and what is currently connected. Safe to poll.

```json
{
  "service": "ai-mechanic",
  "api_version": "v1",
  "build_version": "0.1.0",
  "schema_version": 1,
  "database": "/home/user/.ai-mechanic/sessions.sqlite",
  "bind": "127.0.0.1:8787",
  "default_transport": "simulator",
  "default_scenario": "dpf-regen",
  "scenarios": [
    { "id": "healthy",    "description": "Warm engine at idle, no stored codes, ..." },
    { "id": "dpf-regen",  "description": "Particulate filter regeneration in progress, ..." },
    { "id": "bus-silent", "description": "Adapter responds but the vehicle does not answer ..." }
  ],
  "active": { "session_id": "ses_...", "state": { "state": "ready" }, "adapter": "sim:dpf-regen" }
}
```

`database` is `null` for an in-memory run. `active` is `null` when nothing is
connected.

#### `GET /capabilities`

The safety allowlist, the build's permission ceiling, and what the core
currently believes about the vehicle.

```json
{
  "max_enabled_level": "L1",
  "capabilities": [
    {
      "id": "obd2.read_dtcs",
      "description": "Read stored, pending and permanent DTCs",
      "level": "L0",
      "enabled": true,
      "mutating": false,
      "preconditions": [],
      "required_adapter_flags": []
    }
  ],
  "observed_conditions": {
    "ignition_on": true,
    "engine_running": true,
    "battery_voltage": 14.1,
    "connection_stable": true,
    "vehicle_speed_kph": null
  }
}
```

`enabled: false` capabilities (`obd2.clear_dtcs`, `config.write_configuration`,
`program.program_module`) are registered so their refusal is explicit and
auditable. Show them as unavailable rather than hiding them.

`observed_conditions` is `null` when nothing is connected. Every field comes
from an actual reading, never an assumption.

#### `GET /tools`

The typed tool registry the future agent will call through. Plain JSON Schema,
no vendor wrapper.

```json
{
  "tools": [
    {
      "name": "read_pid",
      "description": "Read one live parameter from a module, ...",
      "capability": "obd2.read_pid",
      "permission_level": "L0",
      "parameters": {
        "type": "object",
        "properties": {
          "module": { "type": "string", "description": "Module key from scan_modules, e.g. \"ECU_7E8\"." },
          "signal": { "type": "string", "description": "Signal id such as \"coolant_temp\", or a PID like \"0x05\"." }
        },
        "required": ["module", "signal"],
        "additionalProperties": false
      },
      "enabled": true,
      "returns": "One or more decoded values, each carrying the raw bytes it came from."
    }
  ],
  "enabled": ["adapter_health", "get_module_identity", "identify_vehicle", "read_dtcs",
              "read_freeze_frame", "read_live_data", "read_pid", "read_supported_pids",
              "scan_modules"]
}
```

#### `POST /tools/{name}` → `ToolResult`

Generic dispatch. Equivalent to the specific endpoints below, and the path the
agent runtime will use.

```json
{
  "arguments": { "module": "ECU_7E8", "signal": "engine_rpm" },
  "initiator": "user:ui",
  "confirmation": null
}
```

`initiator` defaults to `user:api`. Arguments are validated against the schema
and **rejected on any mismatch** — an unknown property, a missing required
argument or a wrong type is a `bad_request` inside the envelope, and no request
reaches the vehicle. An unknown tool name is `operation_not_allowed` with the
available names in `error.details.available`.

---

### Adapter

#### `GET /adapters/ports`

Serial ports the OS can see. On Windows a paired Bluetooth ELM327 appears here
as an outgoing COM port.

```json
{
  "ports": [
    { "name": "COM5", "kind": "bluetooth", "vid": null, "pid": null,
      "serial_number": null, "manufacturer": null, "product": null,
      "likely_obd_adapter": true }
  ],
  "probed": false
}
```

`likely_obd_adapter` is a **name-based hint only**. Confirm with a probe.

##### `GET /adapters/ports?probe=true`

Opens each port and sends `ATZ`/`ATI`. Slower (a couple of seconds per port)
and it briefly opens every port, so make it a deliberate user action.

```json
{
  "ports": [
    { "port": { "name": "COM5", "...": "..." },
      "identification": {
        "descriptor": "COM5", "responded": true,
        "banner": "ELM327 v2.1", "description": null,
        "elm327_compatible": true, "elapsed_ms": 412
      } },
    { "port": { "name": "COM3", "...": "..." },
      "error": { "code": "transport_open_failed", "message": "..." } }
  ],
  "probed": true
}
```

Ports that fail to open are reported **with their reason** rather than dropped —
"COM3 is in use by another program" is exactly what the user needs to see.

#### `GET /adapter`

Current adapter state. Returns `{"connected": false, "state": {"state": "disconnected"}}`
when nothing is connected.

```json
{
  "connected": true,
  "state": { "state": "ready" },
  "descriptor": "sim:dpf-regen",
  "session_id": "ses_be7a0a7df3d0444fab7911cb968bd0bf",
  "health": {
    "state": { "state": "ready" },
    "requests": 32, "responses": 30, "timeouts": 0,
    "no_data": 0, "adapter_errors": 2,
    "mean_latency_ms": 8.0,
    "battery_voltage": 14.1,
    "protocol": "iso15765_can11_500"
  },
  "capabilities": {
    "transport": "simulated",
    "elm327_compatible": true,
    "can_11_bit": true, "can_29_bit": true,
    "iso_tp": true, "multiple_can_buses": false, "j2534": false,
    "supports_transmit": true, "supports_long_messages": true,
    "max_reliable_throughput": 100.0,
    "vendor": "unknown", "model": "ELM327 v2.1", "firmware": "ELM327 v2.1",
    "caveats": [
      "device rejected ATAT1 (adaptive timing) with not_understood",
      "device did not answer AT@1 (device description); vendor is unidentified",
      "banner claims ELM327 v2.x; inexpensive clones commonly report v2.1 while implementing v1.5 behaviour, ...",
      "11/29-bit CAN support is inferred from the ELM327 command set, not measured on this vehicle",
      "single CAN bus only: ELM327-class adapters cannot switch between HS-CAN and MS-CAN"
    ]
  },
  "vehicle": { "...": "..." }
}
```

**`state.state`** is one of `disconnected`, `connecting`, `initializing`,
`identifying`, `ready`, `degraded`, `reconnecting`, `failed`. Three carry extra
fields:

```json
{ "state": "degraded",     "reason": "adapter identified but the vehicle did not answer service 01 PID 00" }
{ "state": "reconnecting", "attempt": 2 }
{ "state": "failed",       "code": "adapter_init_failed", "detail": "ATZ was answered with timeout" }
```

**`capabilities.caveats` is the honest part of this API.** Surface it. A cheap
clone that lies about its version produces a caveat rather than a silent wrong
assumption, and `adapter_errors` counting up is normal for such a device.

**`health.protocol`** values: `unknown`, `j1850_pwm`, `j1850_vpw`, `iso9141_2`,
`iso14230_kwp_slow`, `iso14230_kwp_fast`, `iso15765_can11_500`,
`iso15765_can29_500`, `iso15765_can11_250`, `iso15765_can29_250`.

#### `POST /adapter/connect` → `ToolResult`

Opens the transport, runs the ELM327 init and identification sequence,
negotiates the vehicle protocol, and **starts a new session**.

```json
{
  "transport": "simulator",
  "port": "COM5",
  "scenario": "dpf-regen",
  "label": "morning scan"
}
```

All fields optional; they default to how the server was launched. `transport`
is `serial` | `simulator`. `scenario` is `healthy` | `dpf-regen` | `bus-silent`
(simulator only).

`data`:

```json
{
  "connection_id": "con_...",
  "adapter": "sim:dpf-regen",
  "state": { "state": "ready" },
  "protocol": "iso15765_can11_500",
  "protocol_label": "ISO 15765-4 CAN 11/500",
  "capabilities": { "...": "..." }
}
```

Two outcomes worth handling separately:

- **`state: ready`** — adapter and vehicle both answering.
- **`state: degraded`** — the adapter works, the vehicle is silent (key off,
  unplugged, wrong bus). `success` is still `true` with a `serious`
  `adapter_degraded` warning, because "the adapter is fine" and "the truck is
  answering" are different facts. Reads will then fail `vehicle_not_responding`.

Connecting while already connected is `409 adapter_busy`.

#### `POST /adapter/disconnect` → `ToolResult`

Closes the link and **ends the session**. The session's history remains
readable afterwards.

---

### Vehicle

#### `POST /vehicles/identify` → `ToolResult`

Reads the VIN (service 09 PID 02) and calibration identifiers, records the
vehicle and attaches it to the session.

`values`: the decoded VIN. `data`:

```json
{
  "vin": "1FT7W2BT6KEC00001",
  "vehicle_id": "veh_...",
  "vin_decoded": {
    "vin": "1FT7W2BT6KEC00001",
    "wmi": "1FT", "vds": "7W2BT6", "vis": "KEC00001",
    "manufacturer": "Ford Motor Company (US, truck)",
    "region": "United States",
    "model_year": 2019,
    "check_digit_valid": true
  },
  "reported_by": "7E8",
  "calibration_ids": ["SIMULATED-CAL-01"],
  "calibration_verification_numbers": ["1a2b3c4d"]
}
```

**Only fields the VIN standard actually encodes are filled in.** Manufacturer,
region and model year are decoded from the WMI and the model-year character.
**Model, trim, engine and transmission stay `null`** — this project has no
licensed VIN-decoding database and will not guess. Do not display a model name
the API did not give you.

A `check_digit_valid: false` produces a `serious` warning; show it.

---

### Modules

#### `POST /modules` → `ToolResult` — scan

Discovers responding ECUs (broadcast service 01 PID 00) and asks each for its
own name (service 09 PID 0A).

`data.modules` is an array of module records:

```json
{
  "id": "mod_1034f5b82985480c8e5bb65c9085b9ca",
  "session_id": "ses_...",
  "module_key": "ECU_7E8",
  "name": "SIM ENGINE CONTROL",
  "address": "7E8",
  "protocol": "iso15765_can11_500",
  "identity": { "ecu_name": null, "calibration_ids": [], "calibration_verification_numbers": [] },
  "software_version": null,
  "discovered_at": "2026-09-06T08:21:13.284374149Z"
}
```

`module_key` is what every other module endpoint takes. A module that reports
no name keeps the fallback `"OBD module at 7EA"` — **which module sits at which
address is vehicle-specific and is never guessed.** Do not label `7EA` as a
transmission controller; the API does not know that and neither should the UI.

`identity` is empty until you call the identity endpoint below.

#### `GET /modules`

The scanned modules for the active session, without re-scanning.

```json
{ "modules": [ /* module records */ ] }
```

#### `GET /modules/{key}` → `ToolResult` — identity

Reads ECU name, calibration ids and CVNs (service 09 info types 0A/04/06) and
updates the stored module.

```json
{ "module": { /* module record */ }, "identity": { "ecu_name": "SIM ENGINE CONTROL", "calibration_ids": ["SIMULATED-CAL-01"], "calibration_verification_numbers": ["1a2b3c4d"] } }
```

Fields a module does not report produce `identity_field_unavailable` warnings
rather than failing the read.

#### `GET /modules/{key}/signals` → `ToolResult` — supported PIDs

Walks the full supported-PID mask chain (`0x00`, `0x20`, `0x40`, …).

```json
{
  "module": "ECU_7E8",
  "count": 39,
  "pids": [
    { "pid": 1, "hex": "01", "signal_id": "monitor_status", "name": "Monitor status since DTCs cleared",
      "unit": null, "verification": "verified", "decoder_available": true },
    { "pid": 254, "hex": "FE", "decoder_available": false }
  ]
}
```

`decoder_available: false` means the vehicle supports the PID but this build has
no definition for it — a visible gap rather than a hidden one. Use `signal_id`
to build the live-data picker.

#### `GET /modules/{key}/dtcs` → `ToolResult`

Reads services 03 (stored), 07 (pending) and 0A (permanent).

```json
{
  "dtcs": [
    {
      "code": "P2463",
      "status": "confirmed",
      "module": "ECU_7E8",
      "description": "Diesel particulate filter restriction, soot accumulation",
      "structural_summary": "Powertrain — SAE standard code",
      "verification": "verified",
      "is_generic": true
    }
  ],
  "confirmed_count": 2,
  "modules_read": ["ECU_7E8"]
}
```

`status` is `confirmed` | `pending` | `permanent`. **The same code can appear
twice with different statuses** — confirmed and permanent are two distinct
observations. Group by `(code, status)`, not by `code`.

`description` is `null` when the code is not in the catalog; the code still
decodes structurally. **Never synthesise a description client-side.** An
unlisted code also produces a `dtc_not_in_catalog` warning.

#### `GET /modules/{key}/freeze-frame?frame=0` → `ToolResult`

The stored snapshot from when a fault was recorded. `frame` defaults to `0`.

```json
{
  "module": "ECU_7E8",
  "frame": 0,
  "dtc": "P2463",
  "dtc_description": "Diesel particulate filter restriction, soot accumulation",
  "dtc_verification": "verified"
}
```

`values` holds the frozen parameters. **This is historical data.** It is
deliberately not written to the measurement series, so do not plot it on a live
graph — that is the difference the freeze frame exists to show.

#### `POST /modules/{key}/read` → `ToolResult` — live data

```json
{ "signals": ["engine_rpm", "coolant_temp", "dpf_temp_bank1_inlet"] }
```

Each signal is read as a separate request (ELM327-class adapters handle
multi-PID requests inconsistently, and a clone that truncates one would
attribute a reading to the wrong signal). Signals that cannot be read produce
`unknown_signal` / `signal_unavailable` warnings; the sample still succeeds as
long as **something** was read. If nothing could be read the result fails with
`no_data` — a live graph must never silently flatline.

A signal may be a signal id (`engine_rpm`) or a PID number. Number spellings are
fixed and unambiguous:

| Spelling | Base | Resolves to |
|---|---|---|
| `0x0C` | hex | PID 0x0C |
| `12` | decimal | PID 0x0C |
| `0C` | hex (contains a letter) | PID 0x0C |

Prefer signal ids everywhere in the UI.

---

### Sessions

Session history is readable whether or not an adapter is connected.

#### `GET /sessions?limit=50`

Newest first.

```json
{
  "sessions": [
    {
      "session": { "id": "ses_...", "vehicle_id": "veh_...", "started_at": "...", "ended_at": null, "label": "morning scan" },
      "vehicle": { "id": "veh_...", "vin": "1FT7W2BT6KEC00001", "make": "Ford Motor Company (US, truck)",
                   "model": null, "year": 2019, "trim": null, "engine": null, "transmission": null,
                   "discovered_at": "..." },
      "event_count": 130,
      "module_count": 3,
      "dtc_count": 4,
      "measurement_count": 12
    }
  ]
}
```

#### `GET /sessions/{id}`

Everything about one session except its events.

```json
{
  "session": { "...": "..." },
  "vehicle": { "...": "..." },
  "connections": [ { "id": "con_...", "adapter_id": "sim:dpf-regen", "transport": "simulated",
                     "connected_at": "...", "disconnected_at": "...", "firmware": "ELM327 v2.1",
                     "capabilities": { "...": "..." } } ],
  "modules": [ /* module records */ ],
  "dtcs": [ { "session_id": "...", "module_id": "mod_...", "code": "P2463", "status": "confirmed",
              "description": "...", "occurrence": 3, "freeze_frame_ref": null, "read_at": "..." } ],
  "test_runs": [],
  "diagnoses": [],
  "agent_traces": [],
  "event_count": 130
}
```

`occurrence` counts how many times a code was read in the session.
`test_runs`, `diagnoses` and `agent_traces` are always empty in this build —
the tables and the API exist as the seam for later phases.

#### `GET /sessions/{id}/events?after_seq=0&limit=500`

The Flight Recorder. Append-only, gap-free, ordered.

```json
{
  "events": [
    { "id": 84, "session_id": "ses_...", "seq": 84, "timestamp": "...",
      "kind": { "kind": "adapter_response", "command": "010C",
                "lines": ["7E8 04 41 0C 11 14"], "elapsed_ms": 8, "classification": "data" } }
  ],
  "after_seq": 0,
  "total": 130
}
```

`id` is the row id that `raw_evidence_ref` and `provenance.evidence_ref` point
at. `seq` is the per-session ordinal, and it is **gap-free** — a missing number
means something is wrong, not that nothing happened.

Page with `after_seq` = the last `seq` you have.

`kind.kind` discriminates the payload:

| `kind.kind` | Payload |
|---|---|
| `session_started` | `label` |
| `session_ended` | — |
| `connection_state_changed` | `from` (string), `to` (state object) |
| `adapter_identified` | `capabilities` |
| `adapter_request` | `command` |
| `adapter_response` | `command`, `lines[]`, `elapsed_ms`, `classification` |
| `adapter_failure` | `command`, `error` |
| `vehicle_identified` | `vin`, `vehicle_id` |
| `module_discovered` | `module_key`, `address` |
| `dtc_read` | `module_key`, `code`, `status` |
| `measurement_recorded` | `module_key`, `signal_id`, `value`, `unit`, `raw_hex` |
| `safety_decision` | `operation`, `level`, `allowed`, `initiator`, `confirmed`, `reason` |
| `tool_invoked` | `tool`, `arguments`, `initiator` |
| `tool_completed` | `tool`, `success`, `execution_time_ms`, `warnings[]` |
| `test_started` / `test_finished` | `test_id`/`run_id`, `result` |
| `agent_message` | `role`, `content` |
| `user_note` | `text` |

`classification` on `adapter_response` is one of `ok`, `data`, `info`,
`no_data`, `not_understood`, `unable_to_connect`, `searching_timed_out`,
`bus_error`, `stopped`, `buffer_full`, `adapter_error`, `timeout`.

#### `GET /sessions/{id}/modules`, `GET /sessions/{id}/dtcs`

Stored modules / DTCs for any session. `{ "modules": [...] }`, `{ "dtcs": [...] }`.

#### `GET /sessions/{id}/measurements?signal=engine_rpm&limit=500`

Recorded readings, newest first. `signal` is optional.

```json
{
  "measurements": [
    { "session_id": "ses_...", "module_id": "mod_...", "timestamp": "...",
      "signal_id": "engine_rpm", "value": 1093.0, "text_value": null,
      "unit": "rpm", "raw_value": "1114" }
  ]
}
```

`raw_value` is the hex the reading was decoded from — every stored measurement
can be re-derived from its evidence.

---

## WebSockets

Both streams send JSON **text** frames with a `type` discriminant.

### `GET /api/v1/sessions/{id}/stream?after_seq=0` — flight recorder

Replays the backlog after `after_seq`, then follows live.

Server → client:

```jsonc
{ "type": "event", "event": { /* SessionEvent, as above */ } }   // backlog, then live
{ "type": "hello", "session_id": "ses_...", "from_seq": 130 }    // backlog finished
{ "type": "lagged", "missed": 42, "resume_after_seq": 130 }      // you fell behind
{ "type": "error", "error": { /* error object */ } }
```

The `hello` frame marks the boundary between replayed history and live events;
everything before it is backlog. On `lagged`, re-read the gap over HTTP from
`resume_after_seq` — the server tells you rather than silently dropping events.

The client sends nothing. Close the socket to stop.

### `GET /api/v1/live` — live data

Client → server:

```jsonc
{ "type": "subscribe", "module": "ECU_7E8", "signals": ["engine_rpm", "coolant_temp"], "interval_ms": 250 }
{ "type": "unsubscribe" }
```

Server → client:

```jsonc
{ "type": "hello", "min_interval_ms": 50, "max_signals": 32 }
{ "type": "subscribed", "module": "ECU_7E8", "signals": ["engine_rpm"], "interval_ms": 250 }
{ "type": "sample", "result": { /* ToolResult, identical to POST /modules/{key}/read */ } }
{ "type": "unsubscribed" }
{ "type": "error", "error": { /* error object */ } }
```

- `interval_ms` defaults to 500 and is **clamped to `min_interval_ms`** (50).
  The `subscribed` frame tells you the interval actually used. Asking for 10 ms
  is honoured as 50, not rejected — but the adapter cannot go faster than it
  can, and queuing requests it can never drain would just add latency.
- 1–32 signals. Each costs one adapter round trip, so a 10-signal subscription
  at 50 ms will not keep up on a real Bluetooth clone; watch `execution_time_ms`
  in the samples and let the user reduce the set.
- Subscribing again replaces the current subscription.
- Losing the adapter mid-stream sends `error` and stops sampling; the socket
  stays open so the user sees why.
- Samples are recorded in the session's measurement series, so a live graph and
  the stored history agree.

---

## Typical UI flow

```
GET  /health                              → is the server up, what is connected
GET  /adapters/ports?probe=true           → let the user pick a COM port
POST /adapter/connect {port|scenario}     → session starts; render caveats + degraded state
POST /vehicles/identify                   → VIN header
POST /modules                             → module list for the sidebar
GET  /modules/ECU_7E8/dtcs                → codes
GET  /modules/ECU_7E8/signals             → build the live-data picker
WS   /live  → subscribe                   → charts
WS   /sessions/{id}/stream                → the timeline, live
POST /adapter/disconnect                  → session ends
GET  /sessions                            → history
```

Notes for the UI:

- **Always render `warnings`.** Several are the difference between an honest
  reading and a confident wrong one.
- **Never show an unverified value as a measurement.** Check
  `provenance.verification`.
- **Never invent a module name, a DTC description, or a vehicle model.** If the
  API returns `null`, that is the answer.
- **Make evidence reachable.** `raw_evidence_ref` → the event log entry → the
  literal adapter exchange. That is the Flight Recorder's whole point.

---

## Not in this build

These endpoints exist and answer `501 not_implemented` with an explanation, so
the UI can render "not in this build" rather than inferring it from a 404.

| Endpoint | Why |
|---|---|
| `POST /agent/messages` | The agent runtime is not built. The tool registry it will call through is live at `GET /api/v1/tools`. |
| `POST /modules/{key}/tests/{testId}/run` | No active (L1) test is implemented. Active tests need confirmation and precondition UX, scheduled for the bidirectional-diagnostics phase. |

Also absent by design: any write, configuration or programming operation.
`obd2.clear_dtcs` is implemented end to end and permanently refused —
`GET /capabilities` lists it as `enabled: false`, and calling it returns
`permission_level_disabled` before a single byte reaches the vehicle.
