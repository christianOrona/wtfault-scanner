**Engineering Handoff & Architecture Specification**

A laptop-first, agentic vehicle diagnostics platform that starts with a
cheap ELM327 Mini for read-only experimentation and is designed to grow
into a serious multi-protocol diagnostic tool with an AI mechanic,
structured vehicle knowledge, safe bidirectional testing, and eventually
a mobile client.

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr class="header">
<th><strong>Build philosophy</strong><br />
The LLM is the reasoning and interaction layer. It is NOT the CAN bus
driver, NOT the safety boundary, and NOT allowed to invent or emit
arbitrary vehicle commands. Vehicle communication, decoding, validation,
permissions, and execution live in deterministic software.</th>
</tr>
</thead>
<tbody>
</tbody>
</table>

**Project status:** Prototype / greenfield

**Primary development vehicle:** 2019 Ford F-250 King Ranch (initial
target; exact configuration should be discovered from VIN rather than
hard-coded)

**Current adapter:** Cheap ELM327 Mini Bluetooth adapter — testing only

**Planned upgrade:** USB OBDLink EX or capable J2534-class interface for
deeper Ford work

**Desktop:** Laptop-first; Windows is the initial priority because of
Ford/J2534 ecosystem considerations

**Future:** Potential iOS/Android client sharing the same vehicle/agent
backend contracts

# 1. Product Definition

Working concept: a modern diagnostic application that makes
professional-style vehicle diagnostics understandable and agent-driven.
The user can plug in an adapter, identify the vehicle, scan modules,
inspect codes/live data, ask natural-language questions, and
progressively delegate safe diagnostic tests to an AI mechanic.

The product should feel closer to a modern developer tool or
observability console than a 2005-era scan utility: dense data when
needed, but with clear explanations, timelines, confidence, provenance,
and a conversational diagnostic workspace.

## Core user stories

- “Scan my truck and tell me what is wrong.”

- “What the hell does P2463 mean?”

- “Is this code serious, and can I drive home?”

- “Show me the live data related to this problem.”

- “Run the safest test that will distinguish these two likely causes.”

- “Explain this sensor like I am a normal person.”

- “Give me a mechanic-style report I can send to a shop.”

## Non-goals for the first prototype

- Full OEM replacement of FORScan or dealer programming tools.

- ECU flashing/module programming.

- Unrestricted AI control over CAN traffic.

- Supporting every manufacturer at once.

- Training a custom automotive foundation model before data and
  evaluation infrastructure exist.

# 2. System Architecture

DESKTOP UI (Tauri + React/TypeScript)  
│ localhost IPC / HTTP / WebSocket  
▼  
APPLICATION API / SESSION ORCHESTRATOR  
┌──────────────┴──────────────┐  
▼ ▼  
VEHICLE DIAGNOSTIC CORE AGENT RUNTIME  
transport • protocols plan • reason • explain  
decoding • modules typed tool calls  
tests • safety gate confidence • report  
│ │  
▼ ▼  
ADAPTER ABSTRACTION KNOWLEDGE / RAG  
ELM327 • STN • J2534 DTCs • PIDs • docs • cases  
│ │  
└──────────────┬──────────────┘  
▼  
VEHICLE

## Architectural principles

**1. Deterministic core —** Vehicle communication must be implemented in
normal software. The AI may request an operation through typed tools,
but the core validates and executes it.

**2. Local-first —** The app should remain useful without a cloud
account. Store vehicle sessions locally; cloud AI should be optional.

**3. Protocol-agnostic core —** Keep OBD-II, ISO-TP, UDS, CAN, J2534,
and OEM-specific behavior behind interfaces/adapters.

**4. Vehicle plugins —** Manufacturer/model/platform knowledge should be
data-driven, versioned, testable, and independently updatable.

**5. Evidence-first AI —** Every diagnosis should show the evidence
used: codes, readings, tests, documents, and timestamps.

**6. Safety by construction —** Read operations are the default. Active
tests require explicit capability declarations and confirmation.
Writes/programming are separated and initially disabled.

**7. Mobile-ready contracts —** Do not put UI state or vehicle logic
directly inside desktop components. Use service contracts that a future
mobile client can reuse.

# 3. Recommended Technology Stack

| **Layer**              | **Recommendation**                                                          | **Reason**                                                                                                                               |
|------------------------|-----------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------|
| Desktop shell          | Tauri 2 + React + TypeScript                                                | Small modern desktop footprint; reusable web UI; easy path toward shared frontend concepts later.                                        |
| Core diagnostic engine | Rust (preferred) or Kotlin/Java if team velocity wins                       | Rust is a good fit for low-level transport/parsing and safety-sensitive boundaries. A clean IPC API hides language choice from UI/agent. |
| Local API              | HTTP + WebSocket over localhost                                             | Simple debugging and future mobile/backend compatibility.                                                                                |
| Data                   | SQLite + migrations                                                         | Portable local session store; excellent for offline use and searchable diagnostic history.                                               |
| Agent orchestration    | Typed tool-calling layer independent of any model vendor                    | Lets the project switch between local Ollama models and hosted APIs.                                                                     |
| Local models           | Ollama-compatible models                                                    | Useful for privacy/offline development; model choice remains configurable.                                                               |
| Retrieval              | SQLite FTS + vector index initially; upgrade as needed                      | Keep prototype simple; support citations/provenance from day one.                                                                        |
| Testing                | Rust/TypeScript unit tests + golden CAN/OBD transcripts + simulated vehicle | Most of the hard logic should be testable without a car physically connected.                                                            |
| Packaging              | Windows installer first                                                     | Practical for laptop/Ford tooling development; architecture should not make this permanent.                                              |

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr class="header">
<th><strong>Important</strong><br />
Do not couple the agent directly to serial/Bluetooth APIs. The agent
talks to an application-level tool registry; the registry calls the
diagnostic core.</th>
</tr>
</thead>
<tbody>
</tbody>
</table>

# 4. Hardware Strategy

| **Stage**           | **Hardware**                         | **Use**                                                        | **Expected capability**                                                                                                  |
|---------------------|--------------------------------------|----------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------|
| Prototype now       | Cheap ELM327 Mini Bluetooth          | Read-only exploration; protocol/API development; basic OBD-II  | Assume unreliable, slow, incomplete, or clone behavior until proven otherwise.                                           |
| Development upgrade | OBDLink EX (USB)                     | Serious Ford development and stable wired diagnostics          | Built for FORScan/Ford workflows; supports Ford-specific CAN access and standard OBD protocols. See references.          |
| Advanced            | J2534-class interface                | OEM-style pass-through / deeper programming-oriented workflows | Allows a standardized PC-to-vehicle pass-through architecture; OEM software still controls actual programming sequences. |
| Future mobile       | Bluetooth LE adapter (capable model) | Convenient live-data/read workflows                            | Must be treated as transport option, not trusted execution environment.                                                  |

The current ELM327 Mini is intentionally kept in the plan. It is useful
as the “lowest common denominator” adapter for early development. Design
the adapter layer so its limitations produce capability flags rather
than app-wide assumptions.

The ELM327 ecosystem is known to be an imperfect fit for advanced ECU
work; FORScan itself has described ELM327-class adapters as
amateur-level and highlighted J2534 interfaces for professional
diagnostics. Use the cheap adapter for safe reads and protocol
scaffolding, not as proof that a feature works on all vehicles.
\[1\]\[2\]

## Adapter capability model

AdapterCapabilities {  
transport: Usb \| Bluetooth \| BluetoothLE \| Wifi;  
elm327Compatible: bool;  
can11Bit: bool;  
can29Bit: bool;  
isoTp: bool;  
multipleCanBuses: bool;  
j2534: bool;  
supportsTransmit: bool;  
supportsLongMessages: bool;  
maxReliableThroughput: number;  
vendor: string;  
model: string;  
firmware: string \| null;  
}

# 5. Vehicle Communication & Protocol Layer

Separate the stack into transport, framing, session protocol, diagnostic
service, and semantic decoding. Never mix raw bytes with user-facing
meanings.

Transport  
SerialTransport / BluetoothTransport / J2534Transport  
↓  
CAN / ISO-TP / OBD framing  
↓  
UDS / OBD-II / OEM diagnostic services  
↓  
ECU / module addressing  
↓  
Decoder  
↓  
Typed Domain Object  
↓  
Agent/UI

## Protocol responsibilities

| **Component**    | **Responsibility**                                                                                                                 |
|------------------|------------------------------------------------------------------------------------------------------------------------------------|
| Transport        | Connect/disconnect, bytes in/out, timeouts, reconnection, adapter health.                                                          |
| CAN              | Frames, arbitration IDs, bus selection, filters where supported.                                                                   |
| ISO-TP           | Segmentation/reassembly for diagnostic payloads larger than a single CAN frame.                                                    |
| OBD-II services  | Standardized mode/service requests and response parsing where supported.                                                           |
| UDS layer        | Diagnostic sessions, service requests, negative responses, timing, security state representation. Do not bypass security controls. |
| OEM layer        | Ford-specific module addressing, service definitions, PIDs, tests, configuration capabilities, and metadata.                       |
| Semantic decoder | Turns raw responses into named values with units, scaling, valid ranges, and provenance.                                           |

# 6. Vehicle Knowledge Model

The system should treat vehicle knowledge as versioned data rather than
hard-coded if/else logic. A vehicle profile is selected by decoded VIN +
ECU identification + model/platform metadata.

vehicle-profiles/  
ford/  
f-series/  
super-duty/  
2019/  
f250/  
powertrain/  
pcm.yaml  
tcm.yaml  
chassis/  
abs.yaml  
pscm.yaml  
body/  
bcm.yaml  
rcm.yaml  
infotainment/  
apim.yaml  
procedures/  
dtc/  
pids/  
tests/  
capabilities.yaml

## Example capability definition

capability: turbo_actuator_test  
module: PCM  
risk_level: active_test  
requires:  
- ignition_on  
- engine_off  
steps:  
- verify_engine_state  
- read_preconditions  
- request_actuator_test  
- collect_response  
- restore_normal_state  
outputs:  
- commanded_position_pct  
- actual_position_pct  
- pass_fail  
confirmation_required: true

# 7. Mechanic Agent Architecture

Do not seek a “trained mechanic model” as the first milestone. Start
with a strong general reasoning model plus structured automotive tools
and retrieval. Fine-tune later only after the project has enough
high-quality diagnostic traces and an evaluation suite.

## Agent responsibilities

- Understand user intent and symptoms.

- Create a diagnostic plan.

- Select tools based on declared capabilities.

- Ask for clarification when required.

- Interpret measurements and tests.

- Retrieve authoritative/vehicle-specific knowledge.

- Communicate uncertainty and alternatives.

- Produce a repair-oriented explanation and evidence-backed report.

## Agent must NOT

- Generate arbitrary raw CAN frames and send them directly.

- Invent PIDs, module addresses, test procedures, or expected values.

- Treat a single DTC as proof of a failed component.

- Silently clear codes.

- Perform configuration writes or programming by default.

- Claim a repair is confirmed when evidence is only suggestive.

## Tool registry

READ TOOLS  
identify_vehicle()  
scan_modules()  
get_module_identity(module)  
read_dtcs(module)  
read_freeze_frame(module, dtc)  
read_pid(module, pid)  
read_live_data(module, signals\[\])  
read_supported_pids(module)  
read_vehicle_configuration()  
  
DIAGNOSTIC TOOLS  
run_self_test(module, test)  
run_actuator_test(module, test)  
run_calibration(module, procedure)  
  
WRITE / HIGH RISK (initially gated)  
write_configuration(module, change)  
clear_dtcs(module)  
program_module(module, package)

## Tool result envelope

ToolResult {  
tool: string;  
timestamp: string;  
vehicle_session_id: string;  
module: string \| null;  
success: bool;  
values: \[...\];  
raw_evidence_ref: string \| null;  
warnings: \[...\];  
capability_used: string;  
execution_time_ms: number;  
}

## Agent loop

User request  
↓  
Intent + constraints  
↓  
Vehicle context check  
↓  
Build diagnostic hypothesis set  
↓  
Select lowest-risk evidence gathering  
↓  
Tool call  
↓  
Validate result + update hypothesis scores  
↓  
Need more evidence? ── yes → next safe tool  
│  
no  
↓  
Diagnosis + confidence + evidence  
↓  
Recommended repair / next action

# 8. Automotive Knowledge / RAG Strategy

Use retrieval for facts that are better represented as documents or
structured records than model memory: DTC definitions, service
procedures, specifications, OEM documents that are legally and
legitimately available, TSBs, public technical standards, user-provided
manuals, and validated project notes.

| **Data type**           | **Storage**             | **AI use**                                           |
|-------------------------|-------------------------|------------------------------------------------------|
| DTC definitions         | Structured DB           | Fast lookup; normalized descriptions.                |
| PID definitions         | Structured DB           | Units, scaling, formulas, expected ranges.           |
| Service procedures      | Document store + chunks | Step-by-step reference with citations.               |
| TSBs / technical docs   | Document store          | Pattern matching and supporting evidence.            |
| Vehicle session history | SQLite                  | Compare “before/after” and recurring faults.         |
| Diagnostic cases        | Structured case store   | Later evaluation and optional fine-tuning dataset.   |
| User-provided documents | Separate local corpus   | Personalized vehicle knowledge; preserve provenance. |

## Evidence requirements

Every knowledge-derived claim exposed by the agent should carry
provenance internally, even if the UI only shows a simple “Sources”
disclosure. The model should know whether a statement came from a raw
ECU value, structured decoder metadata, a technical document, a user
statement, or inference.

# 9. UI / UX Direction

Modern, dark-capable, high-contrast, technical but approachable. Avoid
copying the visual language of old scan utilities. Think: VS Code +
modern vehicle dashboard + observability platform.

## Primary desktop layout

┌──────────────────────────────────────────────────────────────┐  
│ AI MECHANIC 2019 F-250 • Connected ● \[Scan\] \[Export\] │  
├──────────────┬───────────────────────────────────────────────┤  
│ VEHICLE │ DIAGNOSTIC WORKSPACE │  
│ Modules │ “Why is my truck showing a DPF warning?” │  
│ DTCs │ │  
│ Live Data │ AI answer • evidence • proposed next test │  
│ Tests │ \[Run Safe Test\] \[Show Data\] \[Explain\] │  
│ Sessions │ │  
│ │ Timeline • live values • test results │  
└──────────────┴───────────────────────────────────────────────┘

## Key screens

- Vehicle Overview — VIN, connection state, adapter health, module
  count, battery voltage, session timer.

- Module Explorer — ECU identity, protocol, address, supported services,
  DTCs, live data.

- Diagnostic Workspace — chat + evidence + proposed plan + tool
  execution timeline.

- Live Data Lab — graph signals, compare related sensors, markers for
  tests/events.

- Test Center — available safe tests with prerequisites and risk level.

- Session Replay — the “Diagnostic Flight Recorder.” Rewind exactly what
  the app read, what it asked, what tests ran, and what the model
  concluded.

- Vehicle History — previous scans, recurring issues, repaired/not
  repaired state, notes, exported reports.

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr class="header">
<th><strong>Signature feature: Diagnostic Flight Recorder</strong><br />
Every session is an immutable-ish chronological trace: connection events
→ VIN → module scan → codes → live readings → agent messages → tool
calls → results → user confirmations → final diagnosis. Allow export as
a shareable diagnostic package. This becomes both a killer UX feature
and a dataset/evaluation source.</th>
</tr>
</thead>
<tbody>
</tbody>
</table>

# 10. Safety & Permission Model

The app is software that can communicate with physical machinery. Safety
is an architectural requirement, not just a UI warning.

| **Level**            | **Examples**                                         | **Default**        | **Policy**                                                                                                     |
|----------------------|------------------------------------------------------|--------------------|----------------------------------------------------------------------------------------------------------------|
| L0 Read-only         | VIN, ECU IDs, DTCs, freeze frame, live data          | Allowed            | No confirmation required after connection.                                                                     |
| L1 Non-invasive test | Self tests, sensor checks, some actuator diagnostics | Confirm            | Show prerequisites, expected behavior, abort/restore behavior.                                                 |
| L2 Configuration     | Feature configuration, module settings, adaptations  | Disabled initially | Require explicit user confirmation and a clear change preview.                                                 |
| L3 Programming       | ECU/module firmware or programming                   | Disabled initially | Separate workflow; no AI autonomy; robust external power/connection requirements and OEM/toolchain compliance. |

## Mandatory guardrails

- Capability allowlist: only explicitly defined tools can execute.

- Precondition checks before active tests.

- Timeouts and cancellation at every transport/test boundary.

- Never hide a negative response or adapter error from the agent.

- Never automatically clear DTCs after a test.

- Require stable power/connection checks before high-risk operations.

- Record who/what initiated every write-capable operation.

- Fail closed: an unknown command is rejected, not “best effort”
  executed.

# 11. Ford / FORScan-Inspired Expansion Strategy

The goal is not to clone proprietary software blindly. The engineering
target is a modular Ford capability layer that exposes legitimate
diagnostic functions supported by the adapter, vehicle, and software
knowledge we have actually implemented and validated.

OBDLink EX is a sensible future Ford development adapter because its
manufacturer explicitly positions it for FORScan and Ford-specific CAN
access, including electronic HS-CAN/MS-CAN switching. \[1\]

SAE J2534 should also be treated as an architectural integration point.
SAE describes J2534 as a standardized PC-to-vehicle pass-through
interface for vehicle module reprogramming workflows, while OEM software
controls the actual programming sequence. \[3\]\[4\]

## Candidate Ford feature buckets

- Full module scan and identification.

- DTC scan/read/clear with dependency checks.

- Live PID dashboards.

- Module self-tests.

- Actuator tests.

- Calibration/initialization routines where the capability is understood
  and validated.

- Vehicle configuration read/write (later and tightly gated).

- Service procedures and maintenance functions.

- J2534 pass-through integration (later).

- Programming/firmware flows only as a separate, expert-only subsystem.

# 12. Core Data Model

Vehicle  
id, vin, make, model, year, trim, engine, transmission, discovered_at  
  
Connection  
id, adapter_id, transport, connected_at, disconnected_at, firmware  
  
Module  
session_id, module_id, name, address, protocol, identity,
software_version  
  
DTC  
session_id, module_id, code, status, occurrence, freeze_frame_ref  
  
Measurement  
session_id, module_id, timestamp, signal_id, value, unit, raw_value  
  
TestRun  
id, module_id, test_id, risk_level, requested_by, confirmed_by_user,  
started_at, ended_at, result, evidence_ref  
  
AgentTrace  
session_id, message_id, role, content, tool_name, tool_args_ref,  
tool_result_ref, model, prompt_version, timestamp  
  
Diagnosis  
session_id, hypothesis, confidence, evidence_refs\[\],
alternatives\[\],  
recommendation, unresolved_questions\[\], created_at

# 13. Internal API Contract Examples

POST /api/v1/vehicles/identify  
GET /api/v1/sessions/{id}/modules  
GET /api/v1/modules/{id}/dtcs  
GET /api/v1/modules/{id}/signals  
POST /api/v1/modules/{id}/tests/{testId}/run  
POST /api/v1/agent/messages  
WS /api/v1/sessions/{id}/events

Keep the contract versioned. The desktop UI and eventual mobile client
should consume the same domain API. The API should return structured
errors, capability state, and provenance rather than UI-specific
messages.

# 14. Suggested Repository Layout

ai-mechanic/  
├─ apps/  
│ ├─ desktop/ \# Tauri shell + React UI  
│ └─ mobile/ \# future client; keep scaffold separate until needed  
├─ core/  
│ ├─ transport/ \# serial, Bluetooth, J2534 adapters  
│ ├─ protocols/ \# OBD-II, CAN, ISO-TP, UDS  
│ ├─ diagnostics/ \# services, scanners, test runner  
│ ├─ decoders/ \# PIDs, DTCs, response parsers  
│ ├─ safety/ \# permissions, preconditions, confirmations  
│ ├─ vehicle-model/ \# normalized vehicle/module domain model  
│ └─ session/ \# session/event/trace storage  
├─ vehicle-profiles/  
│ ├─ ford/  
│ └─ generic-obd/  
├─ agent/  
│ ├─ tools/ \# typed tool schemas  
│ ├─ policies/ \# agent safety policy  
│ ├─ prompts/ \# versioned prompts  
│ └─ orchestration/ \# planner/tool loop  
├─ knowledge/  
│ ├─ dtc/  
│ ├─ pids/  
│ ├─ procedures/  
│ └─ ingestion/  
├─ simulator/  
│ ├─ replay/ \# recorded adapter traces  
│ └─ virtual-vehicle/ \# deterministic fake ECU responses  
├─ tests/  
│ ├─ protocol/  
│ ├─ decoders/  
│ ├─ agent-evals/  
│ └─ end-to-end/  
├─ docs/  
│ ├─ architecture.md  
│ ├─ capabilities.md  
│ └─ contributing.md  
└─ scripts/

# 15. Testing Strategy

| **Test layer**     | **What to test**                                  | **Rule**                                                            |
|--------------------|---------------------------------------------------|---------------------------------------------------------------------|
| Unit               | Frame parsing, ISO-TP, decoders, scaling formulas | High coverage; no hardware required.                                |
| Golden transcript  | Known adapter request/response logs               | Byte-for-byte or semantically equivalent regression checks.         |
| Simulator          | Virtual ECUs with faults and dynamic values       | Every agent tool must be testable without a real vehicle.           |
| Agent eval         | Diagnosis questions with known evidence           | Measure tool choice, hallucination rate, evidence use, uncertainty. |
| Safety eval        | Attempted unsafe/unknown tool calls               | Must reject deterministically.                                      |
| Hardware-in-loop   | Real ELM327 and later OBDLink/J2534 devices       | Run only after simulator tests pass.                                |
| Vehicle validation | Real Ford test vehicle                            | One capability at a time; capture the entire Flight Recorder trace. |

## Critical evaluation metrics

- Tool-call correctness: did the agent request the right operation?

- Evidence grounding: is every diagnosis tied to observed evidence?

- False certainty rate: how often does the agent overstate a conclusion?

- Unsafe action rejection rate: 100% target for disallowed operations.

- Diagnostic efficiency: number of tool calls to reach a useful answer.

- User comprehension: can a non-mechanic understand the explanation?

# 16. Implementation Roadmap

| **Phase**               | **Goal**                                  | **Deliverables**                                                                                |
|-------------------------|-------------------------------------------|-------------------------------------------------------------------------------------------------|
| 0 — Skeleton            | Prove app boots and architecture is clean | Tauri UI, core service, SQLite, adapter interface, simulator, CI.                               |
| 1 — Cheap ELM327        | Useful read-only scanner                  | Connect, VIN, supported PIDs, generic DTC scan/read, live data, session recording.              |
| 2 — Mechanic v0         | Make the app intelligent                  | Agent chat, tool calling, explanations, DTC lookup, diagnosis workspace, evidence display.      |
| 3 — Flight Recorder     | Make every session reproducible           | Event log, replay, exports, bug-report bundles, agent trace.                                    |
| 4 — Ford foundation     | Move beyond generic OBD-II                | Ford profile format, module discovery research, validated Ford read operations.                 |
| 5 — Better adapter      | Support serious Ford diagnostics          | OBDLink EX/J2534 adapter abstraction, multi-CAN where supported, hardware capability detection. |
| 6 — Active diagnostics  | Controlled bidirectional tests            | Test registry, prerequisites, confirmation UX, simulator coverage, real-vehicle validation.     |
| 7 — Power-user features | FORScan-adjacent workflows                | Service functions, advanced module exploration, configuration reads/writes where validated.     |
| 8 — Mobile exploration  | Evaluate companion client                 | Shared API, mobile live-data view, session viewer, remote/near-car workflows as feasible.       |

# 17. “Start Coding Now” Task List

**1.** Create monorepo and choose TypeScript UI + Rust diagnostic core
boundaries.

**2.** Implement Adapter interface and ELM327 Bluetooth adapter stub.

**3.** Implement fake adapter + virtual vehicle before deep hardware
work.

**4.** Implement connection state machine with timeouts/reconnects.

**5.** Implement generic OBD-II VIN + DTC + common PID reads.

**6.** Persist every session event in SQLite.

**7.** Build first polished desktop shell with Vehicle / Modules / DTCs
/ Live Data / Chat / Sessions.

**8.** Create typed Tool Registry and Safety Gate.

**9.** Connect an LLM only through the tool registry.

**10.** Build the first diagnosis flow: “scan my vehicle and explain the
codes.”

**11.** Add provenance and evidence cards to every agent answer.

**12.** Add Flight Recorder timeline and export.

**13.** Only after the generic flow works, begin Ford-specific reverse
engineering/validation and adapter upgrades.

# 18. Prototype Definition of Done

The first compelling milestone is not “replace FORScan.” It is this
exact demo:

1\. Plug laptop into vehicle with the cheap ELM327 Mini.  
2. App connects and identifies the vehicle where possible.  
3. App performs a read-only scan.  
4. User sees codes/modules/live data in a modern UI.  
5. User asks: “What the fuck is wrong with my truck?”  
6. Agent retrieves the evidence and explains likely causes in plain
English.  
7. Agent proposes the next safest diagnostic step.  
8. User sees exactly what the app will read/test before execution.  
9. Result appears in the timeline with raw + decoded evidence.  
10. Agent updates its conclusion and produces a clean report.  
11. Entire session can be replayed/exported.

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr class="header">
<th><strong>Success criterion</strong><br />
A user should feel that the software is actively helping diagnose the
vehicle, not merely translating OBD codes into English.</th>
</tr>
</thead>
<tbody>
</tbody>
</table>

# 19. Extra “Cool Shit” Worth Designing For

**Mechanic Copilot Mode —** Live conversational diagnosis while
physically working on the car. The user can say “I just unplugged the
sensor” and the session records the event as context.

**Before/After Comparison —** Capture a baseline, repair, then rerun the
same measurements and show what changed statistically and visually.

**Symptom Wizard —** Turn vague symptoms (“rough idle when hot”) into
structured questions and evidence requests.

**Repair Confidence Meter —** Separate “confirmed fault,” “highly
likely,” “plausible,” and “needs testing.” Never collapse these into one
diagnosis.

**Shop Handoff —** Generate a professional report with VIN, codes,
freeze frames, measurements, tests, results, and unresolved
questions—without exposing private chat history.

**Vehicle Health Snapshot —** A dashboard that tracks battery voltage,
recurring DTCs, sensor anomalies, and maintenance-relevant trends across
sessions.

**Community Case Library (later) —** Opt-in anonymized diagnostic cases
that improve evaluation and retrieval. Do not build this before privacy
and provenance rules exist.

**Offline Garage Mode —** No internet required: local model + local
knowledge + local session history. Particularly valuable in garages or
remote locations.

# 20. LLM Model Strategy

| **Stage**             | **Model approach**                                            | **Reason**                                                             |
|-----------------------|---------------------------------------------------------------|------------------------------------------------------------------------|
| Prototype             | Strong general-purpose model, cloud or local                  | Fastest route to reliable reasoning and tool use.                      |
| Local-first           | Ollama-compatible instruct/reasoning model                    | Privacy and offline capability; useful when network access is poor.    |
| Domain specialization | RAG + structured tools + strong prompt/evals                  | Usually higher leverage than early fine-tuning.                        |
| Later                 | Fine-tune a smaller automotive model on validated case traces | Only after enough high-quality examples and clear failure modes exist. |

The important asset is not a magic “mechanic model.” It is the
combination of a tested vehicle knowledge layer, deterministic
diagnostic tools, a rich case/evaluation dataset, and a controlled agent
runtime.

# 21. Security, Licensing & Legal Engineering Notes

- Do not assume that knowing a protocol means you have unrestricted
  rights to copy OEM software/data. Track provenance and licensing for
  every knowledge source.

- Do not ship proprietary firmware, dealer credentials, or protected OEM
  content unless the project has the rights to do so.

- Respect vehicle security mechanisms; do not design around bypassing
  immobilizer/security protections.

- Keep logs free of secrets, credentials, or unnecessary VIN/owner
  information when sharing telemetry externally.

- Treat third-party documents and databases as replaceable sources;
  build ingestion interfaces rather than hard-coding one provider.

# 22. References & Technical Starting Points

- \[1\] OBDLink EX product documentation — Ford/FORScan focus,
  HS-CAN/MS-CAN, standard OBD protocols:
  https://www.obdlink.com/products/obdlink-ex/

- \[2\] FORScan forum — J2534/ELM327 adapter discussion and limitations:
  https://forum.forscan.org/viewtopic.php?sid=d55743eb09e94540cf532d4b23a27243&t=867

- \[3\] SAE J2534-1_0500_202201 — Pass-Thru Vehicle Programming:
  https://saemobilus.sae.org/standards/j2534-1_5_00-recommended-practice-pass-thru-vehicle-programming

- \[4\] SAE J2534-2/BA_0500_202201 — Pass-Thru Extended Feature Base
  Document:
  https://saemobilus.sae.org/standards/j2534-2ba_5_00-pass-thru-extended-feature-base-document

- \[5\] Project note: validate every specific protocol/module capability
  against actual vehicle behavior and recorded traces; this document
  intentionally avoids pretending that an unknown Ford module map is
  already verified.

# 23. Handoff Instruction to the Coding Agent

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr class="header">
<th><strong>Instruction</strong><br />
Treat this document as the engineering north star, but keep the
implementation incremental. Do not “shortcut” the architecture by
letting the LLM speak raw CAN or by hard-coding Ford magic bytes into
the UI. First make a simulator-backed diagnostic core; then connect the
cheap ELM327; then add the AI; then deepen Ford capabilities with a
better adapter. Every new capability must be represented as a typed,
testable tool with explicit safety metadata.</th>
</tr>
</thead>
<tbody>
</tbody>
</table>
