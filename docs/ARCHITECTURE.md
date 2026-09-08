# AI Mechanic — Architecture

How the crates fit together, why the boundaries sit where they do, and where
the agent runtime plugs in when it is built.

`docs/HANDOFF.md` is the spec. This document explains the implementation of
Phase 0 + Phase 1: a simulator-backed, read-only diagnostic core with a
localhost API.

---

## The shape of it

```
┌──────────────────────────────────────────────────────────────────────┐
│  Tauri 2 + React UI          (separate agent, builds against API.md) │
└───────────────────────────────┬──────────────────────────────────────┘
                                │ HTTP + WebSocket, 127.0.0.1 only
┌───────────────────────────────▼──────────────────────────────────────┐
│  apps/api            axum server, /api/v1, ToolResult + error envelope│
└───────────────────────────────┬──────────────────────────────────────┘
                                │
        ┌───────────────────────┴────────────────────┐
        │                                            │
┌───────▼──────────┐                    ┌────────────▼─────────────────┐
│ core/tools       │                    │  ⟨ agent runtime — NOT BUILT ⟩│
│ typed schemas    │◀───────────────────│  planner, model client,       │
│ arg validation   │   same seam        │  prompts, evaluation          │
└───────┬──────────┘                    └───────────────────────────────┘
        │
┌───────▼──────────────────────────────────────────────────────────────┐
│  core/diagnostics — DiagnosticService                                 │
│  the single choke point to the vehicle                                │
│    record intent → authorize → talk → decode → persist → ToolResult   │
└───┬─────────────┬──────────────┬───────────────┬─────────────────────┘
    │             │              │               │
┌───▼──────┐ ┌────▼─────┐ ┌──────▼──────┐ ┌──────▼────────┐
│core/     │ │core/     │ │core/        │ │core/session   │
│safety    │ │decoders  │ │adapter      │ │SQLite +       │
│L0–L3 gate│ │data-     │ │ELM327 driver│ │append-only    │
│allowlist │ │driven    │ │capabilities │ │event log      │
└──────────┘ └────┬─────┘ └──────┬──────┘ └───────────────┘
                  │              │
        ┌─────────▼──────┐ ┌─────▼──────────┐
        │vehicle-profiles│ │core/protocols  │
        │YAML: PIDs, DTCs│ │CAN, ISO-TP,    │
        │(versioned data)│ │OBD-II, UDS     │
        └────────────────┘ └─────┬──────────┘
                                 │
                          ┌──────▼──────────┐
                          │core/transport   │
                          │Transport trait  │
                          └──┬───────────┬──┘
                             │           │
                   ┌─────────▼──┐  ┌─────▼─────────────┐
                   │SerialTransport│ │simulator/        │
                   │COM5, rfcomm0 │ │virtual F-250,     │
                   │(real hardware)│ │ELM327 emulator    │
                   └──────────────┘ └───────────────────┘
```

Dependencies point downward only. Nothing below a line knows what is above it:
the transport does not know about ELM327 commands, the adapter does not know
about PIDs, the decoders do not know about SQLite, and the session store does
not know a vehicle exists.

---

## Crates

| Crate | Path | Owns | Depends on |
|---|---|---|---|
| `aim-types` | `core/types` | The shared vocabulary: ids, timestamps, provenance, decoded values, adapter capabilities, the §12 persistence model, the `ToolResult` envelope, `AimError`. No I/O. | — |
| `aim-transport` | `core/transport` | Bytes in and out. `Transport` trait, `SerialTransport`, `LoopbackTransport`, port enumeration. | types |
| `aim-protocols` | `core/protocols` | CAN framing, ISO-TP segmentation and reassembly, OBD-II services, a UDS scaffold with correct negative-response semantics. | types |
| `aim-decoders` | `core/decoders` | Raw bytes → named values with units, ranges and provenance. An expression evaluator, not a match arm per PID. DTC and VIN decoding. | types, protocols |
| `aim-adapter` | `core/adapter` | The `DiagnosticAdapter` seam and the ELM327 implementation: AT init, identification, capability derivation, per-ECU ISO-TP reassembly, connection state machine, port probing. | types, transport, protocols |
| `aim-safety` | `core/safety` | The capability allowlist and the L0–L3 gate. Fail-closed, audited. | types |
| `aim-session` | `core/session` | SQLite schema, migrations, the append-only event log, and query methods. Storage only. | types |
| `aim-diagnostics` | `core/diagnostics` | `DiagnosticService`: the one path to the vehicle. `SessionRecorder`: the flight-recorder observer. | all of the above |
| `aim-tools` | `core/tools` | Vendor-neutral JSON Schema per tool, fail-closed argument validation, dispatch. | diagnostics, safety, types |
| `aim-simulator` | `simulator` | A deterministic virtual 2019 F-250 that speaks the ELM327 wire protocol, plus transcript replay. | types, transport, protocols, decoders |
| `aim-api` | `apps/api` | The axum HTTP + WebSocket server. lib + bin. | everything |

`vehicle-profiles/` is data, not code: PID definitions, scaling formulas and
the DTC catalog as versioned YAML, embedded at build time.

---

## The boundaries that matter

### Raw bytes never carry meaning

Handoff §5. A `DecodedValue` always ships with a `Provenance`: the hex it came
from, which decoder produced it, that decoder's version, and whether the
definition is `verified` or `unverified`. Nothing above the decoder layer sees
a number without knowing how much to trust it — and an unverified definition
produces a warning that survives all the way out to the websocket.

This is why the diesel particulate-filter PIDs are usable but never presented
as measurements: they are marked `unverified` in the profile because this
project has not validated their byte layout against a real truck.

### Decoding is data, not code

Handoff §6. PID scaling lives in `vehicle-profiles/generic-obd/pids/*.yaml` and
is evaluated by a real expression parser. Adding a PID is a data change with no
recompile-and-hope; a Ford profile, when one is validated, is another data file
rather than a new `match` arm.

### One choke point to the vehicle

`DiagnosticService` is the only thing that calls the adapter. The API does not
reach past it, and neither will the agent. That is what makes the safety gate
meaningful — there is no second path to bypass — and what makes the flight
recorder complete: if it happened, it is in the log.

### The safety gate is architecture, not a dialog

Handoff §10. `MAX_ENABLED_LEVEL` is a compile-time constant, not a setting.
L2 and L3 are refused regardless of confirmation, by anyone, including a future
model. `clear_dtcs` is *implemented end to end* and permanently refused,
because a capability that is registered and denied is auditable, while one that
is merely absent invites a workaround. Every decision — allowed or refused —
writes a `safety_decision` event naming the initiator.

### Errors are never swallowed

An ELM327 has many ways of saying no, and they mean different things. `NO DATA`
(the request was valid, nothing answered), `?` (the adapter did not understand),
`UNABLE TO CONNECT` (the adapter is fine, the vehicle is not), `BUFFER FULL`,
`STOPPED`, `BUS ERROR` each map to a distinct `ErrorCode` that reaches the
caller with the adapter's capability snapshot attached.

A related rule: a truncated multi-frame response is an error, not a partial
payload. Half a VIN decodes into something that looks plausible and is wrong.

### Capabilities are observed, not assumed

Handoff §4. The adapter reports what the device *demonstrated*.
`supports_transmit` becomes true only after the vehicle answered a request that
adapter sent. `max_reliable_throughput` is measured from real round trips.
Everything the device claims but has not proven — 29-bit CAN support inferred
from the ELM327 command set, a clone's `v2.1` banner — becomes a caveat string
rather than a silent assumption. The cheap clone's limits show up as five
caveats, not as a wrong belief elsewhere in the app.

### The flight recorder cannot be edited

Handoff §9. `session_events` has SQL triggers rejecting `UPDATE` and `DELETE`.
Enforcing it in the database rather than in Rust means it holds even against
someone poking at the file with the `sqlite3` CLI. Sequence numbers are
per-session and gap-free, so a replay is exact and a missing event is
detectable.

---

## Where the agent runtime plugs in

The seam is built and tested; the model is not wired.

```
      ┌──────────────────────────────────────────┐
      │  agent runtime  ⟨ to build ⟩             │
      │  • model client (Anthropic API / Ollama) │
      │  • planner / tool loop                   │
      │  • prompts, versioned                    │
      │  • evaluation harness                    │
      └───────────────┬──────────────────────────┘
                      │  ToolCall { tool, arguments, initiator, confirmation }
                      ▼
      ┌──────────────────────────────────────────┐
      │  aim_tools::execute()          ⟨ BUILT ⟩ │
      │  1. unknown tool name → refused          │
      │  2. arguments validated against schema   │
      │  3. dispatch to DiagnosticService        │
      └───────────────┬──────────────────────────┘
                      │  ToolResult { values, data, warnings,
                      ▼                raw_evidence_ref, ... }
      ┌──────────────────────────────────────────┐
      │  DiagnosticService → SafetyGate → vehicle│
      └──────────────────────────────────────────┘
```

What exists today:

- **`ToolRegistry::phase1()`** — every READ tool from handoff §7, each with a
  plain JSON Schema. No vendor wrapper: the same schema feeds an Anthropic
  tool-use request or an Ollama function call, which is what keeps the model
  choice a configuration decision.
- **`aim_tools::execute()`** — validates and dispatches. Two independent gates:
  this crate checks argument *shape*, the safety gate checks *permission*.
  Neither substitutes for the other.
- **`ToolResult`** — the §7 envelope, with `warnings`, `raw_evidence_ref` and
  per-value provenance already populated. The agent's evidence requirements are
  satisfied by the data it already receives.
- **Storage** — `agent_traces` and `diagnoses` tables, with
  `record_agent_trace` / `record_diagnosis` on the store.
- **API** — `GET /api/v1/tools` publishes the schemas;
  `POST /api/v1/tools/{name}` is the dispatch endpoint;
  `POST /api/v1/agent/messages` is reserved and answers `not_implemented`.

What to build, and what not to change while building it:

1. Add an `agent/` crate with a vendor-neutral `ModelClient` trait; implement it
   for the Anthropic API and for Ollama.
2. Hand the model `registry.enabled()` as its tool list.
3. Loop: model → `ToolCall` → `aim_tools::execute` → `ToolResult` → model.
4. Record every step with `record_agent_trace`, and the conclusion with
   `record_diagnosis`, citing `raw_evidence_ref` values as evidence.

The agent must not get a direct handle to `DiagnosticService`, the adapter, or
a transport. If a future capability needs a new vehicle operation, it is added
as a registered tool with explicit safety metadata — never as an escape hatch.

---

## Runtime model

One adapter, one vehicle, one operator. Vehicle I/O is blocking by nature: an
ELM327 exchange is a write followed by a wait for a `>` prompt, and two
overlapping requests on one serial port interleave into garbage.

So the diagnostic core is **synchronous**, and the API wraps it:

```rust
Arc<Mutex<Option<DiagnosticService>>>   // one conversation at a time
        ↑
   spawn_blocking                        // async runtime never blocked
```

Async would buy nothing here and cost clarity. The one place tokio genuinely
earns its place is the API: HTTP, websockets, and the broadcast channel the
session store publishes events on.

---

## Platform notes

**Windows is the target.** A paired Bluetooth-SPP ELM327 appears as an outgoing
COM port, so on Windows the Bluetooth transport *is* the serial transport.
Nothing in the code is Windows-specific; `SerialTransport` opens `COM5`,
`/dev/rfcomm0` and `/dev/tty.*` identically.

**The `serial` feature** gates `serialport` (which needs libudev on Linux)
across `aim-transport`, `aim-adapter`, `aim-diagnostics`, `aim-tools` and
`aim-api`. It is on by default. Building with `--no-default-features` drops
real hardware support and keeps the simulator, so the whole workspace builds
and tests on a machine with no libudev. Port enumeration then returns an empty
list with a note rather than pretending no adapters exist.

**`rusqlite` uses `bundled`** — SQLite is compiled in, so there is no system
dependency on any platform.

---

## Testing

| Layer | Where | What |
|---|---|---|
| Unit | in each crate | Frame parsing, ISO-TP flow control, scaling formulas, DTC decoding, safety decisions, schema validation. |
| Wire protocol | `simulator/tests/` | The real `Elm327Adapter` against the ELM327 emulator: init sequence, capability caveats, multi-frame reassembly, injected faults. |
| Service | `core/diagnostics/tests/` | The full stack below the API against the virtual vehicle. |
| Tool dispatch | `core/tools/tests/` | The path the agent will take. |
| End to end | `apps/api/tests/` | The real axum server on a real socket: connect → VIN → PIDs → DTCs → live-data websocket → asserted in SQLite. |

The load-bearing tests, the ones worth keeping green above all others:

- **Every PID a module advertises can actually be read.** The supported-PID mask
  is a promise; this is the test that it is kept.
- **`clear_dtcs` is refused even when confirmed**, and the virtual vehicle's
  `dtcs_cleared` flag proves no request reached the bus.
- **A malformed tool call puts zero requests on the bus.**
- **A truncated multi-frame response is an error**, not a partial payload.
- **The event log is gap-free**, and a value's `evidence_ref` resolves to the
  exact adapter exchange that produced it.

Everything is verifiable with no hardware. Hardware-in-the-loop against the
real ELM327, and then the real truck, comes after the simulator tests pass —
one capability at a time, capturing the whole flight-recorder trace.

---

## Deliberate omissions

- **No Ford-specific anything.** No module addresses, no OEM PIDs, no magic
  bytes. `7EA` and `7EB` are named by address because which module answers
  where is vehicle-specific and unverified. Ford work begins with validation
  against a real truck, not with guesses in a data file.
- **No VIN-to-model lookup.** Manufacturer, region and model year are decoded
  from the standard's own fields. Model, trim and engine stay `null`.
- **No writes, no configuration, no programming.** Separated and disabled.
- **No agent, no model, no prompts.** The seam, not the runtime.
- **No Tauri shell.** A separate agent builds it against `docs/API.md`.
