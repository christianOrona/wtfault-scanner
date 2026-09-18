# WTFault Scanner — Localhost API (`/api/v1`)

The Rust diagnostic core exposes a versioned HTTP + WebSocket API on
**127.0.0.1 only**. The Tauri/React shell, and any future mobile client, are
clients of this contract and hold no vehicle logic of their own.

Examples were captured from build 0.4.2 running against the simulator, and are
trimmed where a list would run to pages. The few that come from a real vehicle
say so.

```bash
# start the server against the virtual 2019 F-250
cargo run -p aim-api -- --simulator --scenario dpf-regen
# → http://127.0.0.1:8787
```

Simulator scenarios: `healthy` (warm idle, nothing stored), `dpf-regen`
(regeneration in progress, filter codes stored), `bus-silent` (adapter answers,
vehicle does not), `parked` (key on, engine off — the state a configuration
change is made in). `cargo run -p aim-api -- --list-scenarios` prints them.

- [Conventions](#conventions)
- [Core envelopes](#core-envelopes)
- [Endpoints](#endpoints)
  - [Server](#server) · [Adapter](#adapter) · [Vehicle](#vehicle) ·
    [Modules](#modules) · [Procedures](#procedures) ·
    [Vehicle configuration](#vehicle-configuration) ·
    [Changing a setting](#changing-a-setting) · [Sessions](#sessions) ·
    [Assistant](#assistant) · [Model and privacy settings](#model-and-privacy-settings) ·
    [Updates](#updates) · [Problem reports and exports](#problem-reports-and-exports)
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

**Timestamps** are RFC 3339 UTC strings, e.g. `2026-09-15T19:25:20.2279718Z`.
One exception: a stored configuration capture's `taken_at` is written as
`2026-09-15 19:25:41.7923936 +00:00:00`. Parse both.

**One conversation with the vehicle at a time.** There is one adapter and one
set of wires, so vehicle requests queue behind each other. A full scan can hold
that queue for a minute or more. `GET /health` and `GET /adapter` never join it:
they answer immediately and report what is running in a `busy` field, so a
screen can say "still working, 42 s" instead of looking hung.

### Two answer shapes

This is the single most important thing to get right in the UI.

| Situation | HTTP status | Body |
|---|---|---|
| A vehicle operation **ran** — succeeded *or* failed | `200` | [`ToolResult`](#toolresult) — check `success` |
| The request itself was wrong or impossible | real status (`400`, `404`, `409`, `501`, …) | [`Error envelope`](#error-envelope) |

A truck answering `NO DATA`, an ECU sending a negative response, a safety check
that refused a write — these are **diagnostic outcomes with evidence attached**,
not HTTP failures. They come back `200` with `success: false` and a structured
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

Endpoints that do not talk to a vehicle (settings, history, updates) return
their own JSON on success rather than a `ToolResult`, and use the error envelope
for failures.

### Error codes

`error.code` is a stable snake_case discriminant. Branch on it; never parse
`error.message`, which is developer-facing and may change.

The status column applies when the code arrives in the error envelope. The same
codes appear inside a `ToolResult` with HTTP `200`.

| Code | Status | Meaning |
|---|---|---|
| `bad_request` | 400 | Malformed request. |
| `decoder_input_invalid` | 400 | Payload was the wrong shape for the decoder. |
| `operation_not_allowed` | 403 | Not in the capability allowlist, or unknown tool. Fail-closed. |
| `permission_level_disabled` | 403 | Exists but is not available this way — L3, or a write asked for through `/tools`. |
| `confirmation_required` | 403 | Needs a human confirmation that was not supplied. |
| `transport_not_found` | 404 | No such port. |
| `decoder_not_found` | 404 | No decoder definition for that signal. |
| `not_found` | 404 | No such session, module, capture, provider or event. |
| `no_active_session` | 409 | Nothing connected. Call `POST /adapter/connect`. |
| `adapter_busy` | 409 | Already connected; disconnect first. |
| `precondition_failed` | 409 | A declared precondition is not met (engine running, no as-built file, no model configured, …). |
| `capability_missing` | 409 | The connected adapter cannot do this. |
| `cancelled` | 499 | The caller cancelled. |
| `storage_error`, `internal` | 500 | Server-side fault. |
| `not_implemented` | 501 | A documented seam that is not built yet. |
| `no_data` | 502 | Request was valid, nothing answered it. |
| `vehicle_not_responding` | 502 | Adapter is fine, the vehicle is silent. |
| `adapter_rejected_command` | 502 | The adapter answered `?`. |
| `adapter_error` | 502 | `BUS ERROR`, `BUFFER FULL`, `STOPPED`, … |
| `adapter_init_failed` | 502 | The adapter's initialization sequence failed. |
| `adapter_not_identified` | 502 | The device is not ELM327-compatible. |
| `transport_open_failed` | 502 | Port exists but will not open (in use, unpaired, out of range). |
| `transport_disconnected`, `transport_io`, `transport_unsupported` | 502 | The link dropped, an I/O error, or a transport not compiled in. |
| `negative_response` | 502 | The ECU refused (service `0x7F`); `details.nrc` has the code. |
| `iso_tp_error` | 502 | Multi-frame reassembly failed, e.g. a truncated response. |
| `protocol_malformed_response`, `unexpected_response` | 502 | A reply that could not be parsed, or answered a different question. |
| `decoded_value_out_of_range` | 502 | A decoded value fell outside its definition's range. |
| `transport_timeout` | 504 | Deadline expired. |

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

Returned by every endpoint that talks to the vehicle. The assistant receives
exactly this shape from the tools it calls, so the interface and the model are
never looking at different versions of what the vehicle said.

```json
{
  "tool": "read_live_data",
  "timestamp": "2026-09-15T19:25:20.8452236Z",
  "vehicle_session_id": "ses_7f800d78c5f545f38fad8d73d9574c7f",
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
  "signal_id": "vin",
  "name": "Vehicle Identification Number",
  "value": { "type": "text", "value": "1FT7W2BT6KEC00001" },
  "unit": null,
  "valid_range": null,
  "out_of_range": false,
  "provenance": {
    "source": "decoder",
    "raw_hex": "013146543757324254364b45433030303031",
    "decoder_id": "obd2.mode09.pid02",
    "decoder_version": "1",
    "verification": "verified",
    "observed_at": "2026-09-15T19:25:20.3194497Z",
    "evidence_ref": 43
  },
  "timestamp": "2026-09-15T19:25:20.3194497Z"
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
particulate-filter and exhaust-gas-temperature PIDs, and every community signal
definition, are unverified today. Show unverified values as raw evidence,
clearly marked, and never as a measured fact. `out_of_range: true` means the
value fell outside its declared range and should be shown as suspect.

### Warning

```json
{ "code": "unverified_decoder", "message": "decoder definitions not validated for: dpf_temp_bank1_inlet", "severity": "caution" }
```

`severity` is `info` | `caution` | `serious`. There are around seventy codes and
more arrive with each feature, so **render any warning by its severity and
message**; branch on a code only where you need to. The ones worth knowing:

| Code | Severity | Meaning |
|---|---|---|
| `unverified_decoder` | caution | A value came from an unvalidated definition. |
| `value_out_of_range` | serious | A decoded value is outside its declared range. |
| `adapter_caveat` | caution | An observed adapter limitation (one per caveat). |
| `adapter_degraded` | serious | Connected, but the vehicle is not answering. |
| `dtc_not_in_catalog` | caution | A code decoded structurally but has no description. |
| `dtc_count_mismatch` | caution | The ECU's claimed code count disagrees with what it sent. |
| `dtc_service_unanswered` | info | A module did not answer, or refused, one of the fault services. |
| `dtc_read_over_uds` | info | A primary-bus module answered no OBD-II fault service, so its codes were read with UDS. |
| `module_did_not_report_codes` | caution | A module's codes could not be read at all. Not the same as having none. |
| `uds_codes_not_faults` | info | Codes a module returned whose status bits describe no fault (self-test not completed); left out of the list. |
| `unknown_signal` / `signal_unavailable` | caution | A requested signal could not be read. |
| `pids_without_decoders` | info | The module supports PIDs this build cannot decode. |
| `identity_field_unavailable` | info | A module did not report part of its identity. |
| `vin_check_digit_invalid` | serious | The VIN failed its own SAE check digit. |
| `module_refused_*` | info | A module answered with a refusal, named by reason: `security_required`, `not_present`, `wrong_session`, `vehicle_conditions`, `busy`, `malformed_request`, `without_saying_why`. |
| `monitor_scaling_unknown` | info | A self-test result's unit could not be scaled with certainty. |
| `conditions_not_met` | caution | A procedure was asked to run while the vehicle was not ready; nothing was measured. |
| `write_conditions_measured` | info | Speed, engine speed or voltage could not be read before a change. |
| `cycle_the_key_to_see_it` | info | A setting was written; most modules act on it after an ignition cycle. |
| `a_difference_is_not_a_finding` | info | A factory comparison found changed bytes; what they mean is not known. |

---

## Endpoints

### Server

#### `GET /health`

Server identity, what is connected, and what the vehicle is being asked right
now. Safe to poll, including in the middle of a scan.

```json
{
  "service": "ai-mechanic",
  "api_version": "v1",
  "build_version": "0.4.2",
  "schema_version": 5,
  "database": "C:/Users/you/AppData/Roaming/ai-mechanic/data/sessions.sqlite",
  "bind": "127.0.0.1:8787",
  "default_transport": "simulator",
  "default_scenario": "dpf-regen",
  "scenarios": [
    { "id": "healthy", "description": "Warm engine at idle, no stored codes, particulate filter loading normally" },
    { "id": "parked",  "description": "Key on, engine off, parked: the state a configuration change is made in" }
  ],
  "active": { "session_id": "ses_...", "state": { "state": "ready" }, "adapter": "sim:dpf-regen" },
  "busy": null
}
```

`database` is `null` for an in-memory run. `active` is `null` when nothing is
connected.

`busy` is `null` when the adapter is free. While something is running:

```json
{
  "busy": {
    "doing": "a full scan of every module",
    "seconds": 42,
    "explanation": "The adapter is busy with a full scan of every module, 42s so far. Only one conversation with the vehicle can run at a time, so anything else is waiting for this."
  }
}
```

**While `busy` is set, `active` is `null` even though something is connected.**
The health check deliberately does not wait for the running operation to read
the session, because a health check that went quiet during a scan would look
exactly like a crash. Use `GET /adapter` for the connection itself.

#### `GET /capabilities`

The safety allowlist, the build's permission ceiling, and what the core
currently believes about the vehicle.

```json
{
  "max_enabled_level": "L2",
  "capabilities": [
    { "id": "obd2.read_dtcs", "description": "Read stored, pending and permanent DTCs",
      "level": "L0", "enabled": true, "mutating": false,
      "preconditions": [], "required_adapter_flags": [] },
    { "id": "config.write_feature", "description": "Change a vehicle configuration setting",
      "level": "L2", "enabled": true, "mutating": true,
      "preconditions": ["ignition_on", "engine_off", "stable_connection", "vehicle_stationary",
                        { "battery_voltage_at_least": 12.4 }],
      "required_adapter_flags": ["supports_transmit"] },
    { "id": "program.program_module", "description": "...",
      "level": "L3", "enabled": false, "mutating": true,
      "preconditions": [], "required_adapter_flags": [] }
  ],
  "observed_conditions": {
    "ignition_on": true, "engine_running": false, "battery_voltage": 14.1,
    "connection_stable": true, "vehicle_speed_kph": null
  }
}
```

`enabled` is whether the capability's level is within this build's ceiling.
Everything up to L2 is enabled: clearing codes (`obd2.clear_dtcs`, L1), asking a
module whether it takes writes (`config.probe_write_gate`, L1) and changing a
setting (`config.write_feature`, L2), each behind a typed human confirmation.
Programming (`program.program_module`, L3) is registered and disabled so its
refusal is explicit and auditable. Show it as unavailable rather than hiding it.

A precondition is a string, or an object when it carries a value
(`{ "battery_voltage_at_least": 12.4 }`).

`observed_conditions` is `null` when nothing is connected. Every field comes
from an actual reading, never an assumption; `null` means not read yet.

#### `GET /explanations`

Every plain-language explanation this build ships, in one document.

```json
{
  "signals":  [ { "id": "coolant_temp", "easy": "How hot the engine is...", "technical": "..." } ],
  "codes":    [ { "id": "P2463", "easy": "...", "technical": "..." } ],
  "concepts": [ { "id": "adapter", "easy": "The little box you plugged into the socket under the dashboard...", "technical": "..." } ]
}
```

Sent whole rather than looked up per item: the set is a few tens of kilobytes,
it never changes while the process runs, and a client that has it can explain
anything on screen without another round trip.

#### `GET /profiles`

What user-supplied profile files contributed at startup, including the ones that
failed to load.

```json
{
  "feature_count": 14,
  "features_from_profiles": 0,
  "report": {
    "directory": "C:/Users/you/AppData/Roaming/ai-mechanic/profiles",
    "files": [
      { "path": ".../ford-f250-2019/features.yaml", "source": "user:ford-f250-2019/features.yaml",
        "kind": "features", "loaded": 12, "error": null }
    ]
  }
}
```

Reported rather than silent: a user who cannot see what got loaded cannot tell
an app that read their file from one that ignored it. `kind` is `features`,
`pids`, `explanations` or `unrecognised`.

#### `POST /profiles/preview`

What importing a profile would add and what it would override. Changes nothing.

```json
{ "text": "version: 1\nprofile: example\nfeatures:\n  - id: example_setting\n ...", "source": "example.yaml" }
```

Send `text`, or `url` to fetch it (`https` only, size-capped, no credentials
attached — a profile is public data by definition). `data`:

```json
{
  "preview": {
    "acceptable": true,
    "source": "example.yaml",
    "changes": [
      { "id": "example_setting", "name": "An example setting", "overrides_existing": false,
        "overrides_measured": false, "claimed_verification": "verified",
        "verification_on_import": "unverified" }
    ],
    "findings": [
      { "code": "claimed_verified", "feature_id": "example_setting", "severity": "note",
        "detail": "The file says this mapping is verified. That is a claim about work this computer did not witness, so it is imported unverified and has to be confirmed on your vehicle before it can be written." }
    ]
  }
}
```

#### `POST /profiles/import`

Same body as the preview. Refused with `400` when the preview has a blocking
finding, or when the build has no profiles directory.

```json
{
  "imported": true,
  "path": "C:/Users/you/AppData/Roaming/ai-mechanic/profiles/imported-example-yaml.yaml",
  "features": 1,
  "preview": { "...": "as above" },
  "note": "Imported unverified, whatever the file claimed about itself. It takes effect when the app next starts: ..."
}
```

Every verification claim in the file is stripped before it is written, so no
trusted-looking definition ever exists on disk. It loads on the **next start**,
deliberately: swapping definitions under a live session would change what a
reading means halfway through one.

#### `GET /tools`

The typed tool registry the assistant calls through. Plain JSON Schema, no
vendor wrapper.

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
  "enabled": ["adapter_health", "get_module_identity", "identify_vehicle", "list_catalog_signals",
              "list_vehicle_features", "preview_configuration_change", "probe_module_capabilities",
              "read_catalog_signal", "read_dtcs", "read_freeze_frame", "read_live_data",
              "read_monitor_tests", "read_pid", "read_supported_pids", "scan_all_modules",
              "scan_modules"]
}
```

`clear_dtcs` is registered and **not** enabled. Nothing that writes to a vehicle
is offered as a tool: clearing codes, probing a write gate and changing a setting
each have their own route, because the assistant reaches the vehicle exclusively
through this registry and must not be able to do any of them.

#### `POST /tools/{name}` → `ToolResult`

Generic dispatch. Equivalent to the specific endpoints, and the path the
assistant uses.

```json
{
  "arguments": { "module": "ECU_7E8", "signal": "control_module_voltage" },
  "initiator": "user:ui",
  "confirmation": null
}
```

`initiator` defaults to `user:api`. Arguments are validated against the schema
and **rejected on any mismatch** — an unknown property, a missing required
argument or a wrong type is a `bad_request` inside the envelope, and no request
reaches the vehicle:

```json
{ "success": false, "error": { "code": "bad_request",
  "message": "read_pid: missing required argument \"signal\"",
  "details": { "tool": "read_pid", "reason": "missing required argument \"signal\"" } } }
```

An unknown tool name is `operation_not_allowed` with the available names in
`error.details.available`. A registered tool that is not enabled is
`permission_level_disabled`, whatever the build's ceiling — the tool's own level
and the capability's are allowed to differ, and the stricter one wins. Refusals
are recorded in the flight recorder like everything else.

---

### Adapter

#### `GET /adapters/ports`

Serial ports the OS can see. On Windows a paired Bluetooth adapter appears here
as a COM port — two of them, and only one works.

```json
{
  "ports": [
    { "name": "COM4", "kind": "bluetooth", "vid": null, "pid": null,
      "serial_number": null, "manufacturer": null, "product": null,
      "likely_obd_adapter": true, "unusable_because": null },
    { "name": "COM3", "kind": "bluetooth_incoming", "vid": null, "pid": null,
      "serial_number": null, "manufacturer": null, "product": null,
      "likely_obd_adapter": false,
      "unusable_because": "This is the incoming half of a Bluetooth pairing - it waits for something to connect to this computer, and never reaches your adapter. Windows always creates one alongside the outgoing port. Pick the other one." }
  ],
  "probed": false
}
```

`kind` is `usb`, `bluetooth`, `bluetooth_incoming`, `native` or `unknown`.
**`bluetooth_incoming` is the port Windows creates for connections *to* this
machine.** Opening it succeeds and every write then times out, which looks
exactly like a dead adapter; `unusable_because` says so, so the interface can
tell the two ports apart. It is `null` on a port worth trying.

`likely_obd_adapter` is a **name-based hint only**. Confirm with a probe.

##### `GET /adapters/ports?probe=true`

Opens each port and sends `ATZ`/`ATI`. Slower (a couple of seconds per port)
and it briefly opens every port, so make it a deliberate user action.

```json
{
  "ports": [
    { "port": { "name": "COM4", "...": "..." },
      "identification": {
        "descriptor": "COM4", "responded": true,
        "banner": "ELM327 v1.4b", "description": null,
        "elm327_compatible": true, "elapsed_ms": 412, "baud_rate": null
      } },
    { "port": { "name": "COM3", "...": "..." },
      "error": { "code": "transport_open_failed", "message": "..." } }
  ],
  "probed": true
}
```

Ports that fail to open are reported **with their reason** rather than dropped —
"COM3 is in use by another program" is exactly what the user needs to see.
`baud_rate` is the line speed a wired cable answered at; `null` for Bluetooth,
which ignores it.

#### `GET /adapter`

Current adapter state. This is the poll the desktop window runs on, and it
**never waits** for a running vehicle request.

```json
{
  "connected": true,
  "state": { "state": "ready" },
  "descriptor": "sim:parked",
  "session_id": "ses_7f800d78c5f545f38fad8d73d9574c7f",
  "health": {
    "state": { "state": "ready" },
    "requests": 15, "responses": 6, "timeouts": 0,
    "no_data": 0, "adapter_errors": 0,
    "mean_latency_ms": 8.13,
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
    "vendor": "unknown", "model": "ELM327 v2.1", "firmware": "ELM327 v2.1", "baud": null,
    "caveats": [
      "device rejected ATAT1 (adaptive timing) with not_understood",
      "device did not answer AT@1 (device description); vendor is unidentified",
      "banner claims ELM327 v2.x; inexpensive clones commonly report v2.1 while implementing v1.5 behaviour, ..."
    ]
  },
  "fitness": {
    "grade": "unmeasured",
    "summary": "Not established yet — 15 request(s) so far, and a rate over that few says nothing.",
    "advice": ["Only one CAN bus is reachable. Modules on a second bus will not answer, and their absence is not evidence that the vehicle lacks them."],
    "requests": 15, "requests_per_second": null, "mean_latency_ms": 8.13,
    "error_rate": 0.0, "failure_rate": 0.0, "timeout_rate": 0.0,
    "can_write": true, "reaches_second_bus": false, "silence_is_evidence": false
  },
  "vehicle": null,
  "busy": null
}
```

Nothing connected: `{ "connected": false, "state": { "state": "disconnected" }, "busy": null }`.

**Two fields for the middle of a long operation:**

- `busy` — `null`, or `{ "doing": "a full scan of every module", "seconds": 42 }`.
- `stale: true` — present when a request is running. The rest of the body is the
  last snapshot read while the adapter was free, because nothing in it (what the
  adapter is, what it can do, whose session it is) changes during a scan.
  Counters in `health` and `fitness` are as of that snapshot.

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
`multiple_can_buses` is `true` only when the adapter identified itself as one
that can switch buses on command (an STN-based OBDLink, for example) — never
from its banner, which can claim to be anything.

**`fitness`** turns the counters into something a caller can act on: a `grade`
(`unmeasured`, `good`, `workable`, `marginal`, `unreliable`), a one-line
`summary`, and `advice` — empty on a good link, specific and actionable
otherwise. `silence_is_evidence` is whether an absence of findings means
anything on this link; until it is `true`, "nothing answered" is not "nothing is
there".

**`health.protocol`** values: `unknown`, `j1850_pwm`, `j1850_vpw`, `iso9141_2`,
`iso14230_kwp_slow`, `iso14230_kwp_fast`, `iso15765_can11_500`,
`iso15765_can29_500`, `iso15765_can11_250`, `iso15765_can29_250`.

#### `POST /adapter/connect` → `ToolResult`

Opens the transport, runs the ELM327 init and identification sequence,
negotiates the vehicle protocol, and **starts a new session**.

```json
{
  "transport": "serial",
  "port": "COM4",
  "scenario": "dpf-regen",
  "label": "morning scan"
}
```

All fields optional; they default to how the server was launched. `transport`
is `serial` | `simulator`. `scenario` is `healthy` | `dpf-regen` | `bus-silent` |
`parked` (simulator only).

`data`:

```json
{
  "connection_id": "con_9761365db47b4df38e41c5c391c84e3d",
  "adapter": "sim:parked",
  "state": { "state": "ready" },
  "protocol": "iso15765_can11_500",
  "protocol_label": "ISO 15765-4 CAN 11/500",
  "answered_before": null,
  "capabilities": { "...": "..." }
}
```

A protocol that answered through the same port before is tried first, and a
wired cable's remembered line speed is reused; both fall back to the full search
if they no longer work.

`answered_before` is `null` on a healthy connect and the first time anything is
plugged in. When a connection fails to reach the vehicle but this adapter or this
vehicle worked before, it says when and how many modules replied, with an
`answered_before` warning. That is history, never reassurance: it makes an
intermittent fault — a connector, a cable — more likely than a dead bus.

Two outcomes worth handling separately:

- **`state: ready`** — adapter and vehicle both answering.
- **`state: degraded`** — the adapter works, the vehicle is silent (key off,
  unplugged, wrong bus). `success` is still `true` with a `serious`
  `adapter_degraded` warning, because "the adapter is fine" and "the truck is
  answering" are different facts. Reads will then fail `vehicle_not_responding`.

A port that will not open is `transport_open_failed` inside the envelope, and
the service is kept: its session holds the flight recorder trace of exactly how
the connection failed. Connecting while already connected is `409 adapter_busy`.

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
  "vehicle_id": "veh_36474bcfcd7f4326a1290e466ca5db6a",
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

#### `GET /vehicles/identity`

Everything established about the connected vehicle, with the evidence for each
value. Never touches the bus: the `POST` above asks the vehicle, this reports
what every read so far has already established.

```json
{
  "identity": {
    "fields": [
      { "field": "vin",
        "candidates": [ { "value": "1FT7W2BT6KEC00001",
          "evidence": [ { "about": "vin", "source": "vin_structure", "value": "1FT7W2BT6KEC00001",
                          "reported_by": null, "note": null } ] } ] },
      { "field": "make",
        "candidates": [ { "value": "Ford Motor Company (US, truck)",
          "evidence": [ { "about": "make", "source": "vin_structure", "value": "Ford Motor Company (US, truck)",
                          "reported_by": null, "note": "decoded from the VIN's world manufacturer identifier" } ] } ] }
    ],
    "observations": [],
    "unresolved": ["model"]
  }
}
```

A field with **more than one candidate** is one two sources disagree about, and
it is passed to everything downstream as unknown rather than as either side's
answer. `unresolved` lists what nothing has established. `identity` is `null`
when nothing is connected.

#### `GET /vehicles/scorecard?vin=`

One set of numbers for what the app knows about a vehicle, read by VIN from whatever database the core is pointed at, with nothing plugged in. An unknown VIN answers with an empty scorecard rather than an error; a missing `vin` is `400 bad_request`. One short line each: `identity` lists identity field names that are settled, contested (sources disagree) or unresolved; `modules` counts modules found, how many are named from what they reported versus still an address placeholder, and how many per protocol; `findings` counts vehicle knowledge by outcome; `as_built` says whether an as-built file is held.

```json
{
  "vin": "1FT7W2BT6KEC00001",
  "identity": { "settled": ["make", "model_year", "vin"], "contested": [], "unresolved": ["model"] },
  "modules": { "found": 6, "named": 2, "placeholder": 4, "by_protocol": { "ISO 15765-4 CAN 11/500": 6 } },
  "findings": { "established": 3, "ruled_out": 1, "observed": 7 },
  "as_built": { "held": true }
}
```

To compare a cold start with what was learned, save a snapshot from each database. Start the core against a new, empty database with `--db` pointing at a new file (omitting `--db` uses an in-memory database). `scripts/scorecard.ps1 -Vin <VIN>` saves a timestamped snapshot, and `scripts/scorecard.ps1 -Diff <before.json>, <after.json>` compares two and exits 1 when anything was lost.

#### `POST /vehicles/scorecard/diff`

Compares two saved scorecards sent as `{ "before": <scorecard>, "after": <scorecard> }`, where `before` is the earlier snapshot. Each section lists what was `gained`, `lost` and `unchanged`. For identity, a field moving towards settled is a gain. For counts a higher number is a gain, except `placeholder`, where fewer is better.

```json
{
  "identity": { "gained": ["model: unresolved -> settled"], "lost": [], "unchanged": ["make: settled", "model_year: settled", "vin: settled"] },
  "modules": { "gained": ["named: 2 -> 5", "placeholder: 4 -> 1"], "lost": [], "unchanged": ["found: 6", "protocol ISO 15765-4 CAN 11/500: 6"] },
  "findings": { "gained": ["established: 3 -> 4"], "lost": [], "unchanged": ["observed: 7", "ruled_out: 1"] },
  "as_built": { "gained": [], "lost": [], "unchanged": ["held: yes"] }
}
```

#### `GET /vehicles/as-built` → `ToolResult`

Whether the manufacturer's factory configuration file is held for this vehicle,
and how to get one.

```json
{
  "held": false,
  "vin": "1FT7W2BT6KEC00001",
  "source": null,
  "imported_at": null,
  "modules_awake_on_the_bus": 0,
  "supported_on_this_make": true,
  "why_not_supported": null,
  "how_to_get_one": [
    "Ford publishes it per VIN on the Motorcraft service site. The account is free; knowing the file exists is the hard part.",
    "Search for this VIN: 1FT7W2BT6KEC00001",
    "Download the .ab file and import it below. It stays on this machine."
  ],
  "what_it_would_add": "The manufacturer's record of how this exact vehicle was configured at the factory, for every module on it - including any asleep right now, ...",
  "what_it_is_not": "A snapshot of how the vehicle left the factory, not how it is now. ..."
}
```

#### `POST /vehicles/as-built` → `ToolResult` — import

```json
{ "text": "<AS_BUILT_DATA><VEHICLE><VIN>1FT7W2BT6KEC00001</VIN><DATA LABEL=\"7A0-02-01\">...", "source": "1FT7W2BT6KEC00001.ab" }
```

The file's **contents**, not a path: the server is not given a way to read
arbitrary files off the machine, even though it runs on the same one today.

```json
{ "imported": true, "vin": "1FT7W2BT6KEC00001", "source": "1FT7W2BT6KEC00001.ab",
  "lines": 2, "modules": 1, "modules_not_answering_on_the_bus": 0 }
```

**The file's VIN must be the connected vehicle's.** Otherwise `success: false`
with `precondition_failed`: the file describes a real vehicle correctly, just
not this one, and its values would be another truck's configuration presented
as yours. Stored against the VIN, on this machine only.

**Ford makes only.** These files are issued by Ford, and their blocks are
numbered for Ford's layout, so an import on any other established make is
refused with `precondition_failed` rather than writing values against
identifiers that mean something else there. `supported_on_this_make` on the
`GET` says so before anything is offered; a make that has not been established
yet is not refused, because the VIN check already covers the wrong-vehicle
case.

Why it matters: an as-built block corresponds to a data identifier
(`block N ↔ 0xDE00 + (N − 1)`), and the file covers every module, including ones
asleep or on a bus the adapter cannot reach.

#### `DELETE /vehicles/as-built` → `ToolResult`

Forget the file held for this vehicle. `data`: `{ "removed": true, "vin": "..." }`.

#### `GET /vehicles/obdb`

Which community signal set (OBDb, CC BY-SA 4.0) belongs to the connected
vehicle, and whether a copy is kept. **Never touches the network.**

OBDb is organised by make and model, which come from the NHTSA vPIC lookup, so
that has to have happened first. Until it has, `repository` is null and
`why_not` names the missing step.

```json
{ "repository": "Mazda-Mazda3", "kept": false, "source": "https://github.com/OBDb/Mazda-Mazda3" }
```

#### `POST /vehicles/obdb` → fetch and keep the signal set

```json
{ "refresh": false }
```

**The only call in this section that sends anything.** On request only, and
listed in `docs/CODE_SIGNING.md`. With `refresh` false (the default) a kept copy
is returned without touching the network.

```json
{ "from_cache": false, "commands": 214, "repository": "Mazda-Mazda3",
  "path": "<profiles>/catalog/obdb/Mazda-Mazda3.json",
  "source": "https://github.com/OBDb/Mazda-Mazda3", "loads_on_next_start": true }
```

Keeping a set records it against the vehicle's VIN as a finding, so the
scorecard shows the gain and the next visit starts ahead of this one. The
signals are unverified until each has actually been read on this vehicle, which
the finding says plainly: a list of what to ask is not a measurement.

A repository that exists but has no signals in it yet answers `not_found`
rather than keeping an empty set, because a vehicle with nothing to offer and
a vehicle nobody has filled in yet are different facts. An oversized reply, or
anything that is not a signal set, is refused before anything is written.

#### `GET /vehicles/knowledge`

What this application has **concluded** about a vehicle, kept against its VIN
across every session. Distinct from the session log, which records what the
vehicle *said*.

Three answers depending on what is asked and what is connected:

```json
// ?vin=1FT7W2BT6KEC00001 — works with nothing plugged in
{ "vin": "1FT7W2BT6KEC00001", "count": 3, "findings": [ /* Finding */ ] }

// no vin, a vehicle connected
{ "count": 2, "findings": [ /* Finding */ ] }

// no vin, nothing connected — which vehicles anything is known about
{ "vehicles": [ { "vin": "1FT7W2BT6KEC00001", "findings": 3 } ] }
```

A finding:

```json
{
  "subject": "module.7A0.write_gate",
  "outcome": "established",
  "claim": "The module at 7A0 accepts configuration writes without a security handshake.",
  "evidence": "Probed with identifier F1FE after confirming it absent: the module's refusal was read for its reason rather than its existence.",
  "authority": "measured_this_session",
  "observed_at": "2026-09-15T19:25:41.9454168Z",
  "session_id": "ses_7f800d78c5f545f38fad8d73d9574c7f"
}
```

`outcome` is `established`, `ruled_out` or `observed`. **Ruled out is the
expensive one** — "this does not work on this vehicle" costs a session on a real
truck to learn and is the finding most easily lost, so show it prominently.
`subject` is the key: one finding per subject per VIN, and recording the same
subject again replaces it. A setting written and read back is `observed`, never
`established`; only somebody seeing the vehicle behave differently establishes
it.

#### `POST /vehicles/knowledge`

Record a finding.

```json
{
  "subject": "feature.mirror_auto_fold.fold_on_lock",
  "outcome": "ruled_out",
  "claim": "Writing the documented fold-on-lock bytes to both door modules does not fold the mirrors on this truck.",
  "evidence": "Written, read back, latched through an ignition cycle; mirrors did not move. Restored byte-for-byte.",
  "authority": "measured_this_session",
  "vin": null
}
```

→ `{ "recorded": true }`

`claim` and `evidence` are both required — a finding nobody can attribute is a
rumour. `authority` defaults to `measured_this_session`. `vin` is optional: when
absent the connected vehicle is used; naming one records against a vehicle that
is not plugged in, for seeding history at a desk. An unknown `outcome` is `400`.

---

### Modules

#### `POST /modules` → `ToolResult` — scan

Finds the modules that answer and asks each for its own name
(service 09 PID 0A).

- **Primary bus** (pins 6 and 14): the legislated broadcast (service 01 PID 00),
  plus a direct check of every address this *vehicle* has answered on before.
  The broadcast only reaches modules obliged to answer it; on a gateway vehicle
  a body controller can sit on the same wires and ignore it, and a rescan used
  to drop it from the answer. Addresses already known cost milliseconds; a
  module nobody has ever seen is what the full scan is for.
- **Secondary bus** (pins 3 and 11), when the adapter can switch buses on
  command: a `TesterPresent` sweep of `700`–`7FF` at whichever bit rate carries
  traffic. Body modules there implement UDS only and answer no OBD-II request.

`data`:

```json
{
  "buses": [
    { "bus": "primary", "label": "primary bus (pins 6 and 14)", "reached": true,
      "modules": 3, "kbits": 500, "note": null },
    { "bus": "secondary", "label": "secondary bus (pins 3 and 11)", "reached": true,
      "modules": 29, "kbits": 500, "note": null }
  ],
  "modules": [
    {
      "id": "mod_b0f42ce4d6ec4808b90d129dac49a47a",
      "session_id": "ses_...",
      "module_key": "ECU_7E8",
      "name": "SIM ENGINE CONTROL",
      "address": "7E8",
      "request_address": "7E0",
      "protocol": "iso15765_can11_500",
      "identity": { "ecu_name": null, "calibration_ids": [], "calibration_verification_numbers": [] },
      "software_version": null,
      "discovered_at": "2026-09-15T19:25:20.4151763Z"
    }
  ],
  "protocol": "iso15765_can11_500",
  "protocol_label": "ISO 15765-4 CAN 11/500"
}
```

(The second bus above is from a 2019 F-250; the simulator has one bus.)

`module_key` is what every other module endpoint takes: `ECU_<address>` on the
primary bus, `BUS2_<address>` on the secondary. The bus is part of the identity
because two buses can each have a module at the same address.

`address` is where the module **answered**; `request_address` is where requests
are **sent**. Outside the legislated `7E0`–`7EF` block there is no formula
relating the two, so the scan records both.

A module that reports no name keeps a fallback such as `"OBD module at 7EA"` —
**which module sits at which address is vehicle-specific and is never guessed.**
Do not label `7EA` as a transmission controller; the API does not know that and
neither should the UI.

`kbits` is `null` for the secondary bus when its rate was not found. A bus with
nothing answering carries a `note`: that can mean no modules there, or a cable
not wired to those pins, and the two are indistinguishable.

#### `GET /modules`

The modules recorded for the active session, without re-scanning, each with
which bus it is on and whether it answers OBD-II at all.

```json
{
  "modules": [
    { "module_key": "ECU_7A8", "name": "Module at 7A8", "address": "7A8", "request_address": "7A0",
      "bus": "primary", "bus_label": "primary bus (pins 6 and 14)", "answers_obd2": true,
      "id": "mod_...", "session_id": "ses_...", "protocol": "iso15765_can11_500",
      "identity": { "...": "..." }, "software_version": null, "discovered_at": "..." }
  ]
}
```

`answers_obd2` is `false` for secondary-bus modules. A live-data picker built on
service 01 has nothing to show for them, and offering one produces a screen of
empty rows. It is derived from the bus, so sessions recorded before there was a
second bus need no migration.

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
      "unit": null, "verification": "verified", "decoder_available": true,
      "kind": "bitfield", "is_measurement": false },
    { "pid": 254, "hex": "FE", "decoder_available": false }
  ]
}
```

`decoder_available: false` means the vehicle supports the PID but this build has
no definition for it — a visible gap rather than a hidden one. Build the
live-data picker from entries with `is_measurement: true`; masks and bitfields
are real but are not gauges.

#### `GET /modules/{key}/dtcs` → `ToolResult`

Fault codes from one module. Primary-bus modules are read with services 03
(stored), 07 (pending) and 0A (permanent); secondary-bus modules with UDS
`0x19 0x02 0xFF`, because they implement nothing else.

```json
{
  "dtcs": [
    {
      "code": "P2463",
      "status": "confirmed",
      "module": "ECU_7E8",
      "description": "Diesel particulate filter restriction, soot accumulation",
      "structural_summary": "Powertrain — SAE standard code",
      "region": "exhaust",
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

`region` is roughly where on a vehicle the code's system lives, when the standard
places it: `engine_bay`, `exhaust`, `fuel_system`, `transmission`, `cabin`,
`wheels`, `electrical`, or `unknown` — which is most manufacturer-specific
codes. A zone, never a part location: a catalyst fault is in the exhaust on every
vehicle ever built, and this project has part locations for none.

A service the module did not answer, or refused, produces
`dtc_service_unanswered` rather than being read as "no codes".

The language is chosen per module. Secondary-bus modules are read with UDS
`0x19`. Primary-bus modules are asked services 03/07/0A first; one that answers
none of them — a body module a gateway forwards onto the main bus, found by
the full scan — is then read with UDS `0x19`, and the result carries
`dtc_read_over_uds` instead of the three `dtc_service_unanswered` notes. UDS
reports `confirmed` and `pending` only. A module that answers neither carries
`module_did_not_report_codes` (caution): its codes were **not read**, which is
not the same as having none.

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
`no_data` — a live graph must never silently flatline. An empty `signals` list
is `400`.

A signal that has timed out three times running on a module stops being asked
for the rest of the session and is reported as such, rather than costing every
sample another six-second wait.

A signal may be a signal id (`engine_rpm`) or a PID number. Number spellings are
fixed and unambiguous:

| Spelling | Base | Resolves to |
|---|---|---|
| `0x0C` | hex | PID 0x0C |
| `12` | decimal | PID 0x0C |
| `0C` | hex (contains a letter) | PID 0x0C |

Prefer signal ids everywhere in the UI.

#### `GET /modules/{key}/monitor-tests` → `ToolResult` — on-board self-tests

Service 06: the results of the tests the engine controller runs on itself, with
the limit each is judged against — which says what is *close* to failing before
any code sets.

```json
{
  "module": "ECU_7E8",
  "supported": true,
  "failing": 1,
  "marginal": 1,
  "marginal_threshold": 0.1,
  "scaling_verification": "unverified",
  "monitors": [
    {
      "mid": 134, "tid": 128,
      "name": "Particulate filter, bank 1", "system": "Emissions (diesel)",
      "value": 38, "min": 0, "max": 40, "unit": "kPa",
      "passed": true, "margin": 0.05,
      "unknown_monitor": false, "unknown_scaling": false,
      "raw": { "uasid": 26, "value": 38, "min": 0, "max": 40 }
    }
  ]
}
```

`passed` and `margin` are exact: both sides of the comparison share the scaling.
**The units are not verified** (`scaling_verification: "unverified"`), so show
them as a best reading. `margin` is how close to failing a test is, as a fraction
of its limit band (`null` when the band is degenerate); a passing test below
`marginal_threshold` counts as `marginal` — the reading service 06 exists to give
and a code reader cannot.
A monitor this build cannot name is reported by number with its verdict intact
(`unknown_monitor: true`).

`supported: false` with a `no_monitor_tests` warning means the module reports no
service 06 monitors; many vehicles built before about 2005 do not implement it.

#### `GET /modules/{key}/capabilities` → `ToolResult` — what a module supports

Measures one module's UDS surface. **Reads only**, and it takes a while — ten
seconds or more — because it walks several identifier ranges.

```json
{
  "module": "ECU_7A8",
  "address": "7A8",
  "sessions": [
    { "session": "default",  "sub_function": "01", "granted": false, "refused_because": null, "detail": "no parsable response" },
    { "session": "extended", "sub_function": "03", "granted": false, "refused_because": null, "detail": "no parsable response" }
  ],
  "security": {
    "implements_security_access": false, "refused_because": null, "detail": "no parsable response",
    "note": "A seed was requested and nothing was sent back. This build has no manufacturer key algorithm and does not attempt one."
  },
  "identifier_count": 2,
  "identifiers": [
    { "did": "DE01", "range": "configuration (manufacturer)", "length": 6, "bytes": "00 11 22 B3 44 55", "text": null }
  ],
  "ranges_probed": [
    { "purpose": "identification",               "from": "F180", "to": "F1FF", "found": 0, "stopped_early_at": null },
    { "purpose": "configuration (manufacturer)", "from": "DE00", "to": "DEFF", "found": 2, "stopped_early_at": null },
    { "purpose": "system supplier",              "from": "FD00", "to": "FDFF", "found": 0, "stopped_early_at": null },
    { "purpose": "manufacturer (sampled)",       "from": "0100", "to": "01FF", "found": 0, "stopped_early_at": null }
  ]
}
```

The programming session (`0x02`) is never requested: entering it can stop a
module behaving normally until it is power-cycled. Security access is probed by
asking for a seed, which changes nothing; no key is ever attempted. The ranges
are a curated set, not exhaustive — 65,536 identifiers would take hours — and
`ranges_probed` says exactly what was covered. `text` is filled in only when a
record plainly is text. A session a module grants shows `granted: true`; one it
refuses carries the reason in `refused_because` (`module_refused_security_required`
is a real lock, `module_refused_not_present` is not). The simulator's body module
implements neither, which is what the example shows.

#### `POST /modules/{key}/write-gate` → `ToolResult`

Whether this module accepts configuration writes. Body:
`{ "confirmation": "..." }`, recorded verbatim.

```json
{
  "module": "ECU_7A8",
  "address_written_to": "7A0",
  "identifier_used": "F1FE",
  "identifier_confirmed_absent": true,
  "extended_session_opened": false,
  "writes_accepted": false,
  "writes_open_without_security": true,
  "refused_because": "module_refused_not_present",
  "verdict": "This module processed the write and refused it because the identifier does not exist - not because of security. Writes are open here in the session that is currently available, so a measured mapping could be applied to this module."
}
```

It writes to an identifier the module has **just confirmed it does not have**,
so there is nowhere for anything to land; the reason for the refusal is the
measurement. `refused_because: "module_refused_security_required"` means writes
are locked behind a seed/key exchange this build does not have. The result is
recorded in vehicle knowledge as `module.<request address>.write_gate` and is
remembered across restarts.

Interfaces usually want [`POST /features/{id}/write-gate`](#post-featuresidwrite-gate--toolresult),
which finds the owning module itself. The assistant can reach neither.

#### `POST /modules/scan-all` → `ToolResult` — every module, every fault

Sweeps the diagnostic address range on the current bus with `TesterPresent` —
asking again with `DiagnosticSessionControl` if nothing at all answers that —
then reads every responder's fault memory with UDS `0x19`. Takes a minute or
more.

```json
{
  "module_count": 5,
  "fault_count": 2,
  "addresses_probed": 255,
  "addresses_already_on_record": 3,
  "addresses_swept_blind": 252,
  "modules": [
    {
      "address": "768", "request_address": "760", "name": "Module at 768",
      "in_legislated_range": false, "fault_count": 2, "note": null,
      "faults": [
        { "code": "C0035-00", "base_code": "C0035", "status": 9,
          "status_summary": "failing right now", "failing_now": true, "confirmed": true,
          "warning_lamp": false, "description": null, "is_generic": true,
          "structural_summary": "Chassis — SAE standard code",
          "next_steps": "C0035-00 has no description in the generic SAE catalog. ..." },
        { "code": "U0121-87", "base_code": "U0121", "status": 8,
          "status_summary": "stored, but not failing at the moment", "failing_now": false,
          "confirmed": true, "warning_lamp": false,
          "description": "Lost communication with anti-lock brake system control module",
          "is_generic": true, "structural_summary": "Network / vehicle integration — SAE standard code",
          "next_steps": null }
      ]
    }
  ]
}
```

This is the difference between a code reader and a scan tool. Service 03 is
emissions diagnostics and reaches the engine and transmission controllers only;
a vehicle with a dead airbag module and a failing body controller comes back
clean from it. `0x19` asks every module, and it is a public standard that works
the same on every manufacturer.

Addresses where this vehicle has answered before are asked first and taken out
of the blind sweep: they answer in milliseconds, while an address with nothing on
it costs the whole deadline. `addresses_swept_blind` versus
`addresses_already_on_record` is the explanation for how long a scan took.

Fault codes carry the UDS failure-type byte (`C0035-00`). `status` is the raw
status byte; `failing_now` and `confirmed` are decoded from it. `note` is set on
a module that answered discovery but not the fault request — normal, since not
every module implements it; the reasons are summarised as `module_refused_*`
warnings.

Every responder is added to the session's module list, so it is reachable by
every other module endpoint afterwards. A module already on that list keeps
the name and identity it reported about itself (service 09); the scan adds only
where it answered. `name` is `Module at <address>` only for a module that has
never said what it is.

#### `GET /readiness` → `ToolResult` — emissions readiness

Readiness from **every** module that keeps it, not one.

```json
{
  "module_count": 3,
  "disagreement_km": null,
  "modules": [
    {
      "module": "ECU_7E8", "address": "7E8", "name": "SIM ENGINE CONTROL",
      "values": [
        { "signal_id": "monitor_status", "value": { "type": "flags", "value": [ { "id": "mil_on", "label": "Malfunction indicator lamp on", "set": false } ] }, "...": "..." },
        { "signal_id": "dtc_count", "value": { "type": "number", "value": 0 }, "...": "..." },
        { "signal_id": "distance_since_cleared", "...": "..." },
        { "signal_id": "warmups_since_cleared", "...": "..." },
        { "signal_id": "distance_with_mil_on", "...": "..." }
      ]
    }
  ]
}
```

Not per-module in the path, deliberately. On a real 2019 truck the engine and
transmission controllers both answer, and disagree — 195 warm-ups against 218,
11,601 km since codes were cleared against 11,605. Neither is faulty; each keeps
its own counters. A card showing one number is picking a winner without saying
so. `disagreement_km` is the spread in distance since codes were cleared, set
when more than one module reports it and they differ by more than a kilometre,
with a `modules_disagree_on_readiness` warning; `null` otherwise. `values` at the
top level holds every module's readings together.

#### `POST /dtcs/clear` → `ToolResult`

Clear stored codes.

```json
{ "confirmation": "user typed CLEAR in the desktop app", "module": null }
```

`confirmation` is required and recorded verbatim; an empty one is `400`.
`module` limits it to one module; absent means every module that answers.
Preconditions: engine off, vehicle stationary, stable connection.

```json
{ "cleared_by": ["7E8", "7EA"] }
```

Its own route rather than the tool dispatch: the assistant reaches the vehicle
only through `/tools`, and must not be able to do this. Clearing is not a repair.
It erases the freeze frames — often the most useful evidence on the scan — and
resets the readiness monitors, so the vehicle fails an emissions test until it
has been driven 50 to 100 miles. Say that before offering the button.

---

### Procedures

Guided procedures: put the vehicle in a stated condition, hold it, and read the
sensors under it. A reading taken without the conditions it needs measures
nothing it exists for.

#### `GET /procedures`

Every procedure this build knows, including any it will refuse, with the reason.

```json
{
  "procedures": [
    {
      "id": "warm_idle",
      "name": "Warm idle",
      "purpose": "The baseline everything else is compared against. Fuel trims at a warm idle say whether the engine is running rich or lean ...",
      "hold_seconds": 15,
      "measures": ["coolant_temp", "short_fuel_trim_b1", "long_fuel_trim_b1", "intake_air_temp", "maf_rate"],
      "instructions": ["Bring the vehicle to a complete stop and leave it stopped.", "Start the engine.", "..."],
      "safety_notes": ["Handbrake on. Do this outdoors or with extraction."],
      "can_run_alone": true,
      "why_not_alone": null
    }
  ]
}
```

Built in: `steady_rpm_2500`, `warm_idle`, `key_on_engine_off`. A procedure that
cannot be done by one person safely — anything needing the vehicle to move —
has `can_run_alone: false` and says why.

#### `GET /procedures/{id}` → `ToolResult` — where the vehicle is

Reads the signals each condition depends on. Poll it while the person gets the
vehicle ready.

```json
{
  "procedure": "warm_idle",
  "name": "Warm idle",
  "state": "waiting",
  "all_met": false,
  "next_step": "Start the engine.",
  "hold_seconds": 15,
  "conditions": [
    { "condition": { "kind": "stationary" }, "instruction": "Bring the vehicle to a complete stop and leave it stopped.",
      "signal": "vehicle_speed", "value": 0, "met": true, "unmeasurable": null },
    { "condition": { "kind": "engine_running", "running": true }, "instruction": "Start the engine.",
      "signal": "engine_rpm", "value": 0, "met": false, "unmeasurable": null },
    { "condition": { "kind": "coolant_at_least", "min": 80 }, "instruction": "Let the engine reach at least 80 °C. ...",
      "signal": "coolant_temp", "value": 31, "met": false, "unmeasurable": null }
  ],
  "measures": ["coolant_temp", "..."],
  "purpose": "...",
  "safety_notes": ["..."]
}
```

`state` is `waiting` (conditions not met — `next_step` says what to do),
`holding` (met, window running), `measured`, `lost` (conditions broke before the
window completed) or `refused`. `unmeasurable` explains a condition that cannot
be read on this vehicle, which is different from one that is not met.

#### `POST /procedures/{id}/run` → `ToolResult`

Checks the conditions, reads what the procedure measures, and checks the
conditions again — the claim is about the whole reading, not either instant.
It does not wait out `hold_seconds` itself: poll `GET /procedures/{id}` until the
state has held, then run. When the conditions are not met it returns the check
above with a `conditions_not_met` warning and **measures nothing**. A procedure
one person cannot do safely is refused with `operation_not_allowed`.

When it runs, `data`:

```json
{
  "procedure": "warm_idle",
  "name": "Warm idle",
  "state": "measured",
  "held_throughout": true,
  "conditions_before": [ /* checks */ ],
  "conditions_after":  [ /* checks */ ],
  "fuel_type": "diesel",
  "declared": ["coolant_temp", "short_fuel_trim_b1", "long_fuel_trim_b1", "intake_air_temp", "maf_rate"],
  "unavailable": ["short_fuel_trim_b1", "long_fuel_trim_b1"],
  "complete": false
}
```

`state` is `measured` when the conditions held across the reading and `lost`
when they broke during it. `values` holds the readings. **`complete` is the headline**, not `state`: the
conditions can hold perfectly while the thing the procedure exists to establish
never gets read — a gasoline-shaped procedure on a diesel has no fuel trims.
`unavailable` lists what could not be read, and `fuel_type` is what the engine
reports (PID `0x51`), `null` when it does not say.

---

### Vehicle configuration

Settings held in a module's configuration rather than repaired: folding mirrors,
a horn chirp, lock behaviour. Where a setting lives is manufacturer-specific and
unpublished, so most entries can be explained and nothing more until someone
measures them.

#### `GET /features` → `ToolResult`

The features that could apply to the connected vehicle.

```json
{
  "catalog_size": 14,
  "narrowed_to_vehicle": true,
  "from_a_similar_vehicle": 0,
  "features": [
    {
      "id": "double_honk_on_leaving",
      "name": "Double horn chirp when you walk away",
      "easy": "The truck honks twice when you leave it locked with the engine running, or when it thinks you have walked away with the key. ...",
      "technical": "Body control module as-built block 41, identifier 0xDE28, byte 6, bit 0. ...",
      "risk": "cosmetic",
      "risk_label": "cosmetic",
      "modules": ["72E"],
      "requires": [],
      "support": "read_only",
      "verification": "verified",
      "source": "...",
      "notes": null,
      "writable_in_principle": true,
      "authority": "owner_supplied_oem_data",
      "authority_explanation": "...",
      "measured_on_this_vehicle": true
    }
  ]
}
```

(The feature is from a 2019 F-250 profile; the list around it is trimmed.)

`support` is how far this build can go:

| `support` | Meaning |
|---|---|
| `described_only` | Known to exist; where it lives is not known. Explained, never read or changed. |
| `read_only` | A mapping exists. It can be read; whether it can be changed is decided by the preview. |
| `writable` | A write through this mapping has been verified on a vehicle. |

`risk` is `read`, `cosmetic`, `convenience`, `service`, `diagnostic_control`,
`safety_critical`, `security` or `programming`. `writable_in_principle: false`
marks a class this product will not change however good the data — anything in
the braking, steering or restraint path, keys and immobilisers, firmware.

`authority` says how much a claim has to do with the vehicle in front of you,
best first: `measured_this_session`, `measured_earlier`,
`owner_supplied_oem_data`, `measured_on_similar_vehicle`, `community_this_model`,
`community_related_model`, `generic_standard`, `model_knowledge`.
`measured_on_this_vehicle: false` is the difference between a fact and a
hypothesis, and the screen must say which it is showing.

#### `GET /features/{id}` → `ToolResult` — the current setting

Reads the record the setting lives in and says what state it is in.

```json
{
  "feature_id": "double_honk_on_leaving",
  "name": "Double horn chirp when you walk away",
  "known": true,
  "readable": true,
  "state": true,
  "record": "04010001030001010101",
  "did": "DE28",
  "module": "726",
  "owning_modules": ["72E"],
  "verification": "verified",
  "writable": false,
  "write_evidence": null,
  "authority": "owner_supplied_oem_data",
  "measured_on_this_vehicle": true,
  "read_from_the_as_built_file": false,
  "mapping_source": "measured on this vehicle 2026-09-12 by diffing body control module as-built backups ...",
  "check_this": null
}
```

(From a 2019 F-250.)

A feature with no executable mapping answers `known: true, readable: false,
state: null` with the steps that would establish it, not a `404` — "can my truck
do this" has a useful answer even when "where does it live" does not.

When the module does not answer — asleep, or on a bus the adapter cannot reach —
an imported as-built file may still hold the record, and
`read_from_the_as_built_file: true` says so. **That is the factory value, not the
current one.** Show it as such.

`check_this` is set for a mapping from another vehicle: a prediction stated
plainly enough to be wrong ("I think this is currently on; does your vehicle
agree?"). If the person's vehicle agrees, that is evidence measured on *their*
vehicle. Nothing is written to find out.

`module` is the request address the mapping names; `owning_modules` are the
addresses modules answer on. `writable` is whether a write has been verified;
the preview is what decides whether one can be made now.

#### `POST /config/capture` → `ToolResult`

Read a set of configuration records off one module — the first half of turning
an unmapped feature into a mapped one. Read-only.

```json
{ "module": "7A0", "identifiers": [56833, 56834], "label": "before" }
```

`module` is the module's **request address** as a hex string. `identifiers` are
data identifiers as numbers (`56833` is `0xDE01`); at most 64 per capture.

```json
{
  "capture_id": "cap_214dd2c88130442c9ff2da0b8c1884f2",
  "records_read": 2,
  "hex": { "DE01": "001122b34455", "DE02": "0100" },
  "capture": {
    "module": "7A0",
    "label": "before",
    "taken_at": "2026-09-15 19:25:41.7923936 +00:00:00",
    "records": { "56833": [0, 17, 34, 179, 68, 85], "56834": [1, 0] }
  },
  "next_step": "Change the setting once, using the vehicle's own controls or a tool already known to do it correctly. Then capture the same identifiers again and compare the two. ..."
}
```

`capture.records` is keyed by identifier in decimal with byte arrays, which is
the form `/config/diff` accepts inline; `hex` is the same content for people.
Captures are stored against the vehicle.

#### `GET /config/captures` → `ToolResult`

Captures stored for this vehicle, newest first — so a baseline taken last week is
findable today without anybody having kept a copy.

```json
{
  "vehicle_id": "veh_36474bcfcd7f4326a1290e466ca5db6a",
  "count": 2,
  "captures": [
    { "id": "cap_2c352b02f2e847a7b8993384d293dc63", "label": "after",  "module": "ECU_7A0",
      "taken_at": "2026-09-15 19:25:42.5422548 +00:00:00", "same_session": true },
    { "id": "cap_214dd2c88130442c9ff2da0b8c1884f2", "label": "before", "module": "ECU_7A0",
      "taken_at": "2026-09-15 19:25:41.7923936 +00:00:00", "same_session": true }
  ]
}
```

#### `POST /config/diff`

Compare two captures and, when exactly one byte moved, propose the mapping.
Touches no vehicle.

```json
{ "before_id": "cap_214dd2c8...", "after_id": "cap_2c352b02...", "after_is_on": true }
```

Each side is given inline (`before`, `after`: a `capture` object as returned
above) or by stored id (`before_id`, `after_id`). The id form is what makes the
loop work across days: baseline, change the setting whenever that happens,
capture again, compare. `after_is_on` is stated by whoever flipped the switch,
because the bytes do not say. A side given neither way is `400`.

```json
{
  "diff": {
    "changes": [ { "did": 56833, "byte": 3, "before": 179, "after": 183, "changed_mask": 4 } ],
    "length_mismatches": [], "only_in_before": [], "only_in_after": []
  },
  "comparable": true,
  "unambiguous": true,
  "proposed_mapping": { "kind": "data_identifier_bits", "module": "7A0", "did": 56833, "byte": 3, "mask": 4, "on": 4, "off": 0 },
  "note": "One byte moved. This mapping describes only the bits that changed. Verify it by reading the feature back, and record where it came from before relying on it."
}
```

`proposed_mapping` is `null` unless exactly one byte moved. `note` explains the
other cases: not comparable (different identifiers or record lengths — usually
different modules or vehicles), nothing moved, or more than one byte moved.

#### `POST /config/compare-to-factory` → `ToolResult`

What on this vehicle is no longer how the factory built it. Needs an imported
as-built file; reads every module the file describes, so it is a `POST`.

```json
{
  "vin": "1FT7W2BT6KEC00001",
  "modules_in_the_file": 1,
  "modules_compared": 1,
  "modules_that_did_not_answer": [],
  "bytes_differing": 1,
  "modules": [
    {
      "module": "7A0",
      "blocks_compared": 2,
      "comparable": true,
      "changes": [ { "did": 56833, "byte": 3, "before": 183, "after": 179, "changed_mask": 4 } ],
      "length_mismatches": [],
      "only_in_the_file": [],
      "only_on_the_vehicle": []
    }
  ]
}
```

`before` is the factory byte and `after` is the vehicle's. Every difference is a
byte somebody changed — a dealer, a previous owner, a workshop, or this
application. It answers what a live read cannot: what has been altered on a
vehicle for sale, whether a setting was ever touched, and which few bytes to look
at when working out where a feature lives.

**It does not say what a changed byte means.** No manufacturer publishes that,
and this project will not invent it; the `a_difference_is_not_a_finding` warning
says so. With no file imported: `success: false`, `precondition_failed`.

#### `GET /catalog/signals` → `ToolResult`

Community signal definitions (OBDb, CC BY-SA 4.0) that might apply to this
vehicle. A lookup; touches no vehicle.

```json
{
  "make": "Ford Motor Company (US, truck)",
  "model": null,
  "count": 114,
  "note": "These are community-recorded claims about what this vehicle answers, not measurements and not a standard. ...",
  "signals": [
    {
      "signal_id": "F150_TP_FL",
      "name": "Tire pressure, front left",
      "group": "Tires",
      "unit": "psi",
      "module": "726",
      "asks": "service 22 parameter 2813 at header 726",
      "from_catalog": "Ford-F-150",
      "authority": "community_related_model",
      "verification": "unverified",
      "licence": "cc_by_sa_4_0",
      "suggested_metric": "frontLeftTirePressure"
    }
  ]
}
```

Everything here is somebody else's recorded claim, and an empty list is the
common case. `authority` says whether it was recorded for this model or a related
one.

#### `POST /catalog/signals/{signal}` → `ToolResult`

Ask the vehicle one community-defined signal. A `POST` because it puts a request
on the bus, even though the request is a read.

The reading is in `data`; `values` is empty for this endpoint.

```json
{
  "signal_id": "F150_TP_FL", "name": "Tire pressure, front left",
  "answered": true, "decoded": true,
  "value": 35.5, "unit": "psi", "label": null, "out_of_stated_range": false,
  "bytes": "01bc", "module": "72E",
  "from_catalog": "Ford-F-150", "authority": "community_related_model",
  "source": "profile_data", "verification": "unverified"
}
```

`answered: false` with `refused_because` distinguishes a module that refused (it
told us something) from one that said nothing. `decoded: false` with `bytes`
means it answered in a shape the definition does not fit. `bytes` is the answer
after the echoed request. Every reading carries
a caution: it is a value to check, never a fact about the vehicle. (Illustrative
values; the simulator has no module at `726`.)

---

### Changing a setting

Three calls, always in this order from the interface, and all addressed by
**feature id**. None of them has anywhere to put a module address, a data
identifier or a byte offset: where a setting lives comes from a vehicle profile,
never from a caller. The examples are from a 2019 F-250.

#### `POST /features/{id}/preview` → `ToolResult` — what a change would do

Body: `{ "desired": "on" | "off" }`. Sends nothing that changes the vehicle. It
does read from it: the record the setting lives in, and vehicle speed, engine
speed and control module voltage, because the preconditions a write is
authorised against are evaluated here too — a preview that skipped them once
said `can_apply: true` and the write that followed was refused.

```json
{
  "feature_id": "double_honk_on_leaving",
  "feature_name": "Double horn chirp when you walk away",
  "risk": "cosmetic",
  "desired": "off",
  "can_apply": false,
  "blocked_reason": "Where this setting lives has been verified, so it can be read. ...",
  "needs_write_gate_probe": true,
  "experiment": false,
  "modules": ["72E"],
  "mapping_source": "measured on this vehicle 2026-09-12 ...",
  "checks": [
    { "id": "mapping_known", "question": "Do we know where this setting lives, and that it can be changed?",
      "passed": false, "detail": "...", "blocking_by_design": false },
    { "id": "engine_off", "question": "Is the engine off?", "passed": true, "detail": null, "blocking_by_design": false }
  ],
  "bytes": {
    "identifier": "DE28",
    "before": "04010001030001010101",
    "after":  "04010001030000010101",
    "byte_index": 6, "byte_before": "01", "byte_after": "00",
    "already_as_asked": false
  }
}
```

- `checks` are ordered as a person should read them. `blocking_by_design: true`
  marks a refusal no situation changes (a risk class this product will not
  touch); everything else is something that could be fixed — the engine, a
  charger, a scan.
- `bytes` is read off the vehicle during the preview, not reconstructed from the
  catalogue. Absent when there is no executable mapping or the record could not
  be read.
- `needs_write_gate_probe` is true only when the one thing in the way is that
  nobody has asked the owning module whether it accepts writes. The interface
  may then ask and change under a single confirmation; for any other blocker it
  must not.
- `experiment` is true when the location of the setting is unverified, so the
  vehicle's behaviour afterwards is the measurement. The confirmation has to say
  so.
- Precondition checks appear alongside the rest with their own ids:
  `ignition_on`, `engine_off`, `vehicle_stationary`, `stable_connection`,
  `battery_voltage`.

#### `POST /features/{id}/write-gate` → `ToolResult`

Does the module that owns this feature accept writes? Body:
`{ "confirmation": "..." }` (required, recorded verbatim; empty is `400`). The same
probe and result as [`POST /modules/{key}/write-gate`](#post-moduleskeywrite-gate--toolresult),
with the module found from the mapping so the interface never has to know a
setting lives at `72E` and is requested at `726`.

```json
{ "module": "ECU_72E", "writes_open_without_security": true, "refused_because": "module_refused_not_present", "verdict": "..." }
```

The result is remembered against the vehicle, so a gate measured open stays open
across restarts. A module that has not answered a scan in this session is
`precondition_failed`.

#### `POST /features/{id}/apply` → `ToolResult` — make the change

Body: `{ "desired": "on" | "off", "confirmation": "..." }`; an empty confirmation
is `400`. Re-evaluates the plan from scratch rather than trusting the preview,
writes, and reads the record back. Never reachable by the assistant.

```json
{
  "feature_id": "double_honk_on_leaving",
  "changed": true, "verified": true, "did": "DE28", "module": "726",
  "before": "04010001030001010101", "after": "04010001030000010101",
  "state_before": true, "state_after": false, "cycle_ignition_to_apply": true
}
```

`verified` means the module read back what was written. It does not mean the
vehicle behaves differently — most modules act on a new value only at power-up,
and only somebody looking at the vehicle after a key cycle can say. The write is
recorded in vehicle knowledge as `observed`, for that reason.

A setting already as asked writes nothing: `{ "changed": false, "reason": "already_set" }`.

A change that cannot go ahead is `success: false` with `precondition_failed`, in
one of two forms. A vehicle precondition (engine running, speed unread) fails
before anything else, with `error.details.failures`:
`[{ "precondition": "engine_off", "reason": "engine is running and must be off" }]`.
Any other failed check names the check ids in the message and carries the whole
plan in `error.details`.

#### From the assistant

`POST /agent/messages` returns `proposed_change` when the model ran
`preview_configuration_change` successfully during the turn:

```json
{ "text": "...", "trace": [], "question": null,
  "proposed_change": { "feature_id": "double_honk_on_leaving", "desired": "off" } }
```

Only the feature and the value. The interface calls `preview` itself before it
draws a button, so nothing the model was shown — a check, a byte — reaches the
person by way of the model, and the model has no route to `write-gate` or
`apply`.

---

### Sessions

Session history is readable whether or not an adapter is connected.

#### `GET /sessions?limit=50`

Newest first. `limit` is capped at 500.

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
  "connections": [ { "id": "con_...", "session_id": "ses_...", "adapter_id": "sim:parked", "transport": "simulated",
                     "connected_at": "...", "disconnected_at": "...", "firmware": "ELM327 v2.1",
                     "capabilities": { "...": "..." } } ],
  "modules": [ /* module records */ ],
  "dtcs": [ { "session_id": "...", "module_id": "mod_...", "code": "P2463", "status": "confirmed",
              "description": "...", "occurrence": 3, "freeze_frame_ref": null, "read_at": "..." } ],
  "test_runs": [],
  "diagnoses": [],
  "agent_traces": [],
  "event_count": 3266
}
```

`occurrence` counts how many times a code was read in the session.
`test_runs`, `diagnoses` and `agent_traces` are always empty in this build —
the tables and the API exist as the seam for later phases. What the assistant
did in a session is in the event log as `tool_invoked` events with an
`agent:`-prefixed initiator.

#### `GET /sessions/{id}/events?after_seq=0&limit=500`

The Flight Recorder. Append-only, gap-free, ordered. `limit` is capped at 5000.

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
means something is wrong, not that nothing happened. An unknown session is
`404`, not an empty page that looks like a session with no history.

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

`initiator` is `user:api` for a person using the interface and `agent:`-prefixed
for the assistant, so a reading the assistant asked for is always
distinguishable from one a person clicked.

`classification` on `adapter_response` is one of `ok`, `data`, `info`,
`no_data`, `not_understood`, `unable_to_connect`, `searching_timed_out`,
`bus_error`, `stopped`, `buffer_full`, `adapter_error`, `timeout`.

#### `GET /sessions/{id}/modules`, `GET /sessions/{id}/dtcs`

Stored modules / DTCs for any session. `{ "modules": [...] }`, `{ "dtcs": [...] }`.

Every code read is stored, whichever way it was read: services 03/07/0A, a
module read over UDS, and the full scan. A UDS code keeps its failure type
(`C0035-00`) and is `confirmed` or `pending` from its status bits, so an
emissions module read both ways has both forms on record. `occurrence` counts
how many times the same code and status were seen in the session.

#### `GET /sessions/{id}/measurements?signal=engine_rpm&limit=500`

Recorded readings, newest first. `signal` is optional; `limit` is capped at
10,000.

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

#### `GET /sessions/{before}/compare/{after}`

What changed between two recorded visits to the same vehicle — the question a
single scan cannot answer. A fuel trim creeping for six months looks exactly
like one that was always there until there is a second reading.

```json
{
  "comparison": {
    "before": "ses_7f800d78c5f545f38fad8d73d9574c7f",
    "after":  "ses_bd5713c81cea4611a71cd5d947e53357",
    "before_at": "2026-09-15T19:25:19.9416811Z",
    "after_at":  "2026-09-15T19:25:43.045407Z",
    "days_apart": 0.00027,
    "same_vehicle": true,
    "faults": [ { "code": "P2463", "module": "ECU_7E8", "description": "...", "change": "gone" } ],
    "signals": [
      { "signal_id": "control_module_voltage", "unit": "V", "before": 12.6335, "after": 12.634,
        "delta": 0.0005, "relative": 0.00004, "samples_before": 4, "samples_after": 1 }
    ]
  },
  "notable_signals": []
}
```

`change` is `appeared`, `gone` or `unchanged`. **`gone` is not "fixed"**: a code
also disappears when somebody clears it, and this cannot tell the difference —
the readiness counters can. A UDS code matches its service 03 form (`P2463-00`
and `P2463` are the same fault); two codes that both carry a failure type must
match exactly. Signals are means over each session's readings, most
changed first, with sample counts so a single reading is never mistaken for an
average. Conditions are not controlled; a cold engine and a warm one differ for
honest reasons, so a difference is context, not a verdict.

`same_vehicle` is `null` when either session did not read a VIN; comparing two
different vehicles produces confident nonsense, so it is checked.
`notable_signals` is the store's judgement of what is worth attention (a 20%
relative change, or any change from zero) — sent alongside rather than used to
filter, so nothing is silently dropped.

---

### Assistant

The assistant is a model driven through the same tool registry as everything
else, with **read-only tools**. It can read the vehicle, explain, ask the person
something, and preview a change; it cannot write.

The response examples in this section are illustrative rather than captured:
producing one means calling a model. The shapes are the server's.

Every call below needs a configured model. Without one: `409 precondition_failed`
with `details.agent_code: "no_provider"` and `details.hint`. Other failures keep
the distinction a person cares about — `operation_not_allowed` for a rejected key
or a refusal, `transport_open_failed` for an endpoint that cannot be reached,
`internal` for anything else — with `details.agent_code` naming it.

#### `GET /agent`

Whether the assistant is usable, and why not.

```json
{
  "ready": true,
  "provider": { "id": "prov_34aa5bd86d73e83f", "label": "Workshop GPU", "model": "qwen2.5:14b", "speed": "quality", "max_steps": null },
  "reason": null,
  "purpose": "owner",
  "tone": "practical"
}
```

`ready: false` carries a `reason`. `purpose` (`owner` | `buyer`) is who the
assistant is talking to, which decides whether a verdict is framed as a purchase
decision or a maintenance one.

#### `POST /agent/messages` — one conversational turn

```json
{
  "messages": [
    { "role": "user", "content": "Turn off the double honk after I leave the cabin" }
  ]
}
```

The conversation so far, oldest first, ending with the new message. An empty
list is `400`.

**Only prose crosses this boundary.** Tool traffic from earlier turns is not
replayed from the client: the client is not the authority on what the vehicle
said, and accepting tool results from it would be a way to put words in the
core's mouth. The model re-reads what it needs.

```json
{
  "text": "That setting lives in your body control module. The change is below.",
  "steps": 3,
  "truncated": false,
  "usage": { "input_tokens": 5210, "output_tokens": 180, "cache_read_tokens": 0 },
  "trace": [
    { "type": "tool", "name": "list_vehicle_features", "arguments": {} },
    { "type": "tool_done", "name": "list_vehicle_features", "success": true, "evidence_ref": null },
    { "type": "tool", "name": "preview_configuration_change", "arguments": { "feature_id": "double_honk_on_leaving", "desired": "off" } },
    { "type": "tool_done", "name": "preview_configuration_change", "success": true, "evidence_ref": 912 }
  ],
  "question": null,
  "proposed_change": { "feature_id": "double_honk_on_leaving", "desired": "off" }
}
```

- `trace` is what the assistant actually did. Entries are `thinking` (`text`),
  `tool` (`name`, `arguments`) and `tool_done` (`name`, `success`,
  `evidence_ref`). Build follow-up suggestions from it, not from the prose.
- `truncated: true` means the step limit stopped the run before the model
  finished.
- `question` is set when the model asked the person something only they can
  answer — what the dash menu shows, whether a noise happens cold:
  `{ "question": "...", "options": ["...", "..."], "why": "..." }`, two to five
  options. Answering is an ordinary next message; a button is a shortcut to
  typing, never an action taken on the person's behalf.
- `proposed_change` — see [Changing a setting](#from-the-assistant).

#### `POST /agent/inspect` — a whole-vehicle report

```json
{ "request": "I am thinking about buying this vehicle. Inspect it and tell me what is wrong with it ..." }
```

`request` is optional and defaults to that buyer's question. Takes minutes: the
model decides which checks are worth running on this vehicle, runs them, and
must finish by submitting a structured report.

```json
{
  "report": {
    "verdict": "negotiate",
    "headline": "Sound engine, but the particulate filter needs attention soon.",
    "summary": "...",
    "findings": [
      {
        "title": "Particulate filter close to its limit",
        "severity": "serious",
        "plain_english": "...",
        "source": "measured",
        "evidence": ["Self-test 86/80 at 38 of 40 kPa"],
        "evidence_refs": [412],
        "what_to_do": "...",
        "estimated_cost": { "low": 300, "high": 900, "currency": "USD", "basis": "..." }
      }
    ],
    "watch_items": [],
    "not_checked": ["The second CAN bus: this adapter cannot reach it."],
    "next_steps": ["..."]
  },
  "text": "",
  "steps": 14,
  "truncated": false,
  "usage": { "...": "..." },
  "trace": [ /* as above */ ]
}
```

`verdict` is `walk_away`, `negotiate`, `looks_sound` or `inconclusive`.
`severity` is `critical`, `serious`, `caution` or `info`. `source` is where the
finding came from: `measured` (read from this vehicle, traceable to a raw
exchange), `indirectly_measured` (worked out by this build from something
measured), `profile` (a vehicle profile's mapping), `catalog` (the shipped SAE
description) or `model_knowledge` (the model's general knowledge, not verified
against this vehicle). **The report has nowhere for the model to state its
own confidence**; confidence is derived from what a finding cites, and a
"measured" finding with no `evidence_refs` is not treated as measured, because
that word means there is a raw exchange you can go and look at. `not_checked` is
as important as the findings. `report` is `null` if the run ended without one.

---

### Model and privacy settings

Stored in a settings file with the user, not in the session database: that file
holds VINs and vehicle history, and an API key has no business travelling with
it. Keys go to the operating system's credential store where there is one.

#### `GET /settings/providers`

```json
{
  "providers": [
    {
      "id": "prov_34aa5bd86d73e83f", "kind": "ollama", "label": "Workshop GPU",
      "base_url": "http://127.0.0.1:11434", "model": "qwen2.5:14b", "selected": true,
      "speed": "quality", "speed_supported": true,
      "max_steps": null, "max_tokens": null, "context_tokens": null, "context_supported": true,
      "has_key": false, "key_hint": null,
      "key_source": "none", "key_source_explanation": "No key is stored for this provider."
    }
  ],
  "purpose": "owner",
  "tone": "practical",
  "settings_path": "C:/Users/you/AppData/Roaming/ai-mechanic/data/providers.json",
  "storage_note": "The settings themselves are kept in this file. Keys are not — they go to your operating system's credential store. ...",
  "kinds": [
    { "id": "anthropic", "label": "Anthropic (Claude)", "requires_key": true, "default_base_url": "https://api.anthropic.com",
      "help": "Strongest reasoning. Needs an API key; your data leaves this machine." },
    { "id": "ollama", "label": "Ollama", "requires_key": false, "default_base_url": "http://127.0.0.1:11434", "help": "..." },
    { "id": "xai", "label": "xAI (Grok)", "requires_key": true, "default_base_url": "...", "help": "..." },
    { "id": "openai_compatible", "label": "OpenAI-compatible", "requires_key": false, "default_base_url": null, "help": "..." }
  ]
}
```

A key is never returned. `key_hint` shows just enough to recognise it.
`key_source` is `environment`, `operating_system`, `plain_file` or `none`, and
`key_source_explanation` says it against that key — on a machine with no
credential store the key stays in the settings file in plain text, and the
person is told so rather than left to assume otherwise.

#### `POST /settings/providers`

```json
{
  "kind": "anthropic",
  "label": "Claude",
  "model": "claude-opus-5",
  "api_key": "sk-ant-...",
  "base_url": null,
  "speed": "quality",
  "max_steps": null,
  "max_tokens": null,
  "context_tokens": null,
  "select": false
}
```

`kind`, `label` and `model` are required (empty label or model is `400`).
`speed` is `quality` | `fast`. Limits are clamped: `max_steps` 2–40,
`max_tokens` 512–64,000, `context_tokens` 2,048–131,072 (Ollama). The first
provider added is selected automatically.

→ `{ "providers": [ /* as above */ ], "added": "prov_..." }`

#### `PUT /settings/providers/{id}`

Same body. **Omit `api_key` to keep the stored key; send `""` to remove it.**
Without that distinction, editing the model name would silently wipe the key.
→ `{ "providers": [...] }`. Unknown id: `404`.

#### `DELETE /settings/providers/{id}`

Removes the provider and its key from the credential store; if it was selected,
the first remaining provider is selected. → `{ "providers": [...] }`.

#### `POST /settings/providers/{id}/select`

→ `{ "providers": [...] }`.

#### `POST /settings/providers/{id}/test`

Contacts the provider. **`200` either way** — being unable to reach a model is a
result of the test, not a failure of the request.

```json
{ "reachable": true, "models": ["qwen2.5:14b", "llama3.1:8b"], "elapsed_ms": 84, "detail": null }
{ "reachable": false, "error": { "code": "provider_unreachable", "message": "cannot reach Workshop GPU at http://127.0.0.1:11434/api/tags: ..." } }
```

#### `POST /settings/voice`

Who the assistant is talking to and how bluntly. Separate from providers because
it is a property of the person, and it should survive switching models.

```json
{ "purpose": "buyer", "tone": "blunt" }
```

Both optional. `purpose` is `owner` | `buyer`; `tone` is `practical` | `neutral` |
`blunt`. → `{ "purpose": "buyer", "tone": "blunt" }`

#### `GET /settings/privacy`

Whether the VIN may be sent to the model, stated as the current situation rather
than as an abstract setting — a person cannot answer "share identifiers with
hosted models?" without knowing which situation they are in, and the app knows.

```json
{
  "share_identifiers": "not_beyond_your_network",
  "provider": "Workshop GPU",
  "runs_on": "this_machine",
  "runs_on_explanation": "This model runs on this computer. Nothing you ask it leaves the machine.",
  "vin_is_sent": true,
  "summary": "The VIN is included, and the model runs on this computer, so it does not leave the machine.",
  "options": [
    { "id": "not_beyond_your_network", "label": "Keep it on my own network", "help": "... The default." },
    { "id": "never", "label": "Never send it", "help": "..." },
    { "id": "always", "label": "Always send it", "help": "..." }
  ]
}
```

`runs_on` is `this_machine`, `your_network` or `somebody_else`. `vin_is_sent`
answers the only question that matters outright. `null` fields mean no model is
configured. When the VIN is withheld the model is told the vehicle is identified
and the number is not being shared, so it does not report the VIN as unreadable.

#### `POST /settings/privacy`

```json
{ "share_identifiers": "never" }
```

→ the same body as `GET`, reflecting the new choice.

---

### Updates

Releases come from the project's GitHub releases, and the download URL is
checked against an allowlist of GitHub hosts and this repository's path.

#### `GET /update/check`

Harmless, so it needs no confirmation.

```json
{
  "current": "0.4.2",
  "latest": "v0.4.1",
  "update_available": false,
  "notes": "Finding the second bus turned a seven-module vehicle into a thirty-six module one, ...",
  "url": "https://github.com/christianOrona/wtfault-scanner/releases/tag/v0.4.1",
  "size": 5224441,
  "error": null
}
```

**Never a failure.** "We could not reach GitHub" is a status with `error` set, to
show a person calmly rather than colour the screen red. `error` is also set when
the newest release has no Windows installer attached.

#### `POST /update/download` · `GET /update/download`

Fetch the installer in the background without installing it. The `POST` returns
immediately and asking twice is harmless; poll the `GET`.

```json
{ "stage": "downloading", "version": "0.4.3", "downloaded": 2097152, "total": 5224441, "path": null, "error": null }
```

`stage` is `idle`, `downloading`, `ready` (on disk, size checked, waiting for
someone to say when) or `failed` (`error` says why).

#### `POST /update/apply`

Install the downloaded version, downloading first if needed. **This starts an
executable**, so it sits behind an explicit button.

```json
{ "started": true, "installer": "C:/Users/you/AppData/Local/Temp/WTFault_Scanner_0.4.3_x64-setup.exe",
  "note": "The installer is running silently. This app closes and comes back updated." }
```

The installer runs silently (`/S /UPDATE /R`): no pages, the previous version
removed as quietly, and the app relaunched afterwards. A running app cannot
overwrite its own executable, so it hands over and exits rather than pretending
to update in place. Failure is `400` with the reason.

---

### Problem reports and exports

#### `GET /support/report`

Everything a person would be asked for when reporting a problem, gathered in one
place. Reads local state and **sends nothing anywhere**; what happens to the text
is the person's decision.

```json
{
  "generated": "2026-09-15T19:25:46.0464721Z",
  "app_version": "0.4.2",
  "os": "windows x86_64",
  "previous_run_ended_badly": null,
  "log_dir": "C:/Users/you/AppData/Roaming/ai-mechanic/logs",
  "log_file": "C:/Users/you/AppData/Roaming/ai-mechanic/logs/wtfault.2026-09-15.log",
  "has_log": true,
  "text": "WTFault Scanner problem report\nGenerated : 2026-09-15T19:25:46.0464721Z\nVersion   : 0.4.2\n..."
}
```

`previous_run_ended_badly` is `{ "version", "started", "pid" }` when the last run
never removed its marker — a crash or a forced close. `text` is the whole report,
assembled here so what is copied and what is saved cannot drift apart. It can
contain VINs, fault codes and paths with the user's name in them: show it before
anything is done with it.

#### `POST /support/reveal`

Opens the log folder in the desktop's file manager. A `POST` because it starts a
program; it takes no path, since the folder is this process's own.
→ `{ "opened": "C:/Users/you/AppData/Roaming/ai-mechanic/logs" }`

#### `POST /export`

Write a file the user asked to keep into their Downloads folder.

```json
{ "filename": "inspection-1FT7W2BT6KEC00001.md", "content": "# Inspection\n..." }
```

→ `{ "path": "C:/Users/you/Downloads/inspection-1FT7W2BT6KEC00001.md", "directory": "C:/Users/you/Downloads", "filename": "inspection-1FT7W2BT6KEC00001.md" }`

Server-side because a Tauri webview has no download manager: a blob link does
nothing there, silently, and this can say where the file went. The name is
reduced to a bare filename (letters, digits, `-`, `_`, `.`, space; at most 120
characters) and the content is capped at 256 KB, so a bug upstream can neither
write outside Downloads nor fill a disk. Empty content or an unusable name is
`400`.

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
GET  /adapter   (poll)                    → connection state; `busy` for anything long
GET  /modules/ECU_7E8/dtcs                → codes
GET  /modules/ECU_7E8/signals             → build the live-data picker
WS   /live  → subscribe                   → charts
POST /modules/scan-all                    → every module's faults, when asked
GET  /features → POST /features/{id}/preview → …/apply   → change a setting
POST /agent/messages                      → ask; render `question` and `proposed_change`
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
- **Show `busy` while anything long runs.** A scan that is working and a scan
  that has hung look identical without it.

---

## Not in this build

These endpoints exist and answer `501 not_implemented` with an explanation, so
the UI can render "not in this build" rather than inferring it from a 404.

| Endpoint | Why |
|---|---|
| `POST /modules/{key}/tests/{test_id}/run` | No active (L1) test is implemented. Active tests need confirmation and precondition UX, scheduled for the bidirectional-diagnostics phase. |

Clearing codes (`POST /dtcs/clear`) and changing a setting (see
[Changing a setting](#changing-a-setting)) are implemented, and both need a typed
confirmation from a person. Programming (L3) is compiled out.
