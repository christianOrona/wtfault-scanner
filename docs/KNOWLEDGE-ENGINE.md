# Becoming a knowledge engine, not a Ford scanner

An architecture report, written against an external research mandate that asked
WTFault to grow from an OBD-II reader into a manufacturer-agnostic diagnostic
platform.

The mandate's central argument is right and this report adopts it. Several of
its specific recommendations are not actionable for this codebase, and one would
introduce a licensing violation; those are corrected here rather than quietly
skipped, because the reasoning matters more than the verdict.

Nothing here is implemented yet. This is the report the mandate asked for before
code.

---

## Summary

**The core idea is correct and we should follow it.** A diagnostic core that
knows nothing about manufacturers, fed by pluggable knowledge providers, is a
better architecture than what we have. The direction is adopted.

**Three of its recommendations must be rejected**, for reasons of fact rather
than taste:

1. Every library it says to "prefer" is Python or C++. This is a Rust project.
2. It recommends replacing an ISO-TP and UDS implementation that is working on
   real vehicles today with libraries we cannot link against.
3. It recommends `autodiag2/database`, which has **no licence at all**.

**Much of what it asks us to build, we already have** — provenance,
verification status, a safety ladder, offline-first operation, data-driven
decoders, and a structurally-enforced rule that a model cannot name an address.
Building those again would be paying twice.

---

## A. Repository audit

Verified from the GitHub API on 2026-09-10, not from memory.

| Project | Licence | Language | Stars | Verdict |
|---|---|---|---|---|
| **OBDb** | CC BY-SA 4.0 | data | ~740 repos | **Adopted.** Already parsing and shipping one signalset |
| commaai/opendbc | MIT | Python | — | **Rejected for now** — different problem, see below |
| hardbyte/python-can | **LGPL-3.0** | Python | 1592 | Not adoptable: language, and LGPL |
| cantools/cantools | MIT | Python | 2284 | Not adoptable: language. Format list worth copying |
| pylessard/python-can-isotp | MIT | Python | 315 | Not adoptable: language. We have ISO-TP working |
| pylessard/python-udsoncan | MIT | Python | 727 | Not adoptable: language. We have UDS working |
| ecubus/EcuBus-Pro | Apache-2.0 | C++ | 865 | **Study only** — architecture reference, as the mandate itself says |
| autodiag2/database | **NOASSERTION** | Python | **5** | **Reject.** No licence means all rights reserved |
| collin80/SavvyCAN | MIT | C++ | 1826 | Study only, and only if a capture mode is ever built |
| brendan-w/python-OBD | GPL-2.0 | Python | 1310 | Already studied (the maintained Ircama fork). Copyleft: behaviour only, never code |
| CyanLabs As-Built | site terms | data | — | Ford knowledge provider. Terms need reading before any bundling |
| mercedes-benz/odxtools | MIT | Python | — | Data model worth copying. ODX files themselves are manufacturer-restricted |

### The language problem

The mandate's closing recommendation is to "prefer existing mature libraries
for: CAN → python-can, ISO-TP → python-can-isotp, UDS → udsoncan, DBC →
cantools, ELM327 → python-OBD."

WTFault is twelve Rust crates behind a Tauri shell that ships as a 7.5 MB
installer. Adopting those means embedding a Python runtime in a desktop
application, which costs bundle size, install complexity, cross-platform
packaging, and a whole class of failure modes on a garage laptop. The trade is
not close.

What transfers from those projects is *knowledge*, and we have already taken it:
the adapter-resilience work in this repository came from reading python-OBD and
AndrOBD for behaviour and reimplementing independently.

### But "reimplement natively" was only half an answer

Rejecting the Python libraries and stopping there was a gap in the first draft
of this report. The obvious next question — *what exists natively in Rust?* —
went unasked, and the answer turns out to matter.

Verified from crates.io on 2026-09-10:

| Crate | Licence | Downloads | Updated | Verdict |
|---|---|---|---|---|
| `automotive_diag` | **MIT OR Apache-2.0** | 208,986 | 2026-08 | **Evaluate.** `no_std` definitions for UDS, KWP2000, OBD-II, DoIP |
| `can-dbc` | **MIT OR Apache-2.0** | 5,369,533 | 2026-07 | Usable if DBC is ever needed |
| `iso13400-2` | MIT OR LGPL-3.0 | 1,298,815 | 2026-05 | Usable under the MIT option. DoIP, future |
| `docan` | MIT OR LGPL-3.0 | 6,032 | 2026-05 | Usable under the MIT option |
| `automotive` | MIT | 10,727 | 2026-03 | Worth a look |
| `can_adapter` | MIT | 6,126 | 2026-08 | Worth a look if J2534 ever lands |
| `ecu_diagnostics` | **GPL-3.0-only** | 85,750 | 2026-08 | **Cannot link.** Would relicense this project |
| `isotp-rs` | **GPL-3.0** | 15,939 | 2024-10 | Cannot link |
| `ecu-uds` | **GPL-3.0** | 8,390 | 2024-10 | Cannot link |
| `j1939` | **GPL-3.0-only** | 31,211 | 2026-02 | Cannot link. Relevant if heavy diesel is ever supported |
| `socketcan` | MIT | 9,621,591 | 2026-09 | Linux CAN interfaces. Wrong layer for an ELM327 |

The standout is **`automotive_diag`**: `no_std`, definitions-only, under exactly
this project's licence, actively maintained, and covering KWP2000 and DoIP as
well as UDS. Our own UDS tables are hand-written and cover the subset we needed.

That is worth evaluating rather than adopting on sight — the tables work today —
but "we wrote our own service and NRC tables" is a weaker position than "we use
the maintained ones and keep our own semantics on top". See #40.

Note also that four of the most relevant crates are GPL. In Rust a GPL
dependency is linked into the binary and relicenses the whole application, which
is a harder constraint than the same library would be in Python. That is the
one place where the language difference cuts *against* us.

### python-can does not solve our problem

python-can abstracts **CAN interfaces** — SocketCAN, PCAN, Vector, Kvaser.
WTFault talks to an **ELM327 over a serial port**, and the ELM327 *is* the CAN
controller. There is no layer for python-can to sit in. This is a category
error in the mandate rather than a judgement call.

### opendbc is a different problem, and unreachable on this hardware

DBC files decode **passive broadcast CAN traffic**. Our stack is
request/response. Worse, it is unreachable with the adapters this project is
built for: measured on 2026-09-10, both a Bluetooth clone and a USB cable
returned **zero bytes** to `ATMA` monitor mode on all four CAN speeds, while
`0100` on the same protocol answered from two ECUs seconds later. These clones
do not implement monitoring at all.

The transferable idea is **fingerprinting** — identifying a vehicle from what
its ECUs report — which belongs in section E and needs none of opendbc's code.

### autodiag2/database

`NOASSERTION` means no licence, which under copyright means all rights reserved.
Five stars. The mandate describes it as "specifically designed as an open
automotive diagnostic database"; it may be intended as one, but it is not
licensed as one. Using its data would be a violation. Rejected until it carries
a licence.

---

## B. What we already have

Listed so that nobody rebuilds it. Each of these is asked for by the mandate and
already exists in this repository.

| Mandate asks for | Already present |
|---|---|
| Provenance on every definition | `SourceKind` + `VerificationStatus`, travelling with each decoded value |
| Evidence/confidence system | `Confidence` **derived from citations**, never asserted |
| L0–L3 safety ladder | `PermissionLevel` L0–L3, plus an independent `RiskClass` ceiling |
| Model must not construct traffic | Structural: a tool takes a *feature id*, and no request type has a field for an address |
| Offline operation | Already fully offline; no feature requires a network |
| Data-driven decoders | PIDs, DTCs, monitors and features are all YAML, evaluated by a real expression parser |
| Community knowledge with scoping | OBDb signalsets, matched by make/model/year, labelled `related_model` when inexact |
| Unknown data is not invented | An unknown code keeps its structural decoding and gets no description |

Two places where what we have is **stronger** than what the mandate proposes:

**Numeric confidence scores are worse than what we do.** The mandate suggests
`"confidence": 0.87`. A number like that is unfalsifiable and invites exactly
the false precision this project exists to avoid — nobody can say why it is not
0.85. Our model derives a confidence *class* from whether citations exist, and
records the basis. `ClaimedMeasuredWithoutEvidence` is a more useful thing to
show a person than `0.72`.

**Our safety ladder already has the second axis the mandate lacks.** Permission
(who may) and risk (what happens if it goes wrong) are independent ceilings.
A configuration write and a DTC clear can sit at the same permission level and
still be governed differently, which a single L0–L4 scale cannot express.

---

## C. Architecture

The mandate's diagram, corrected for what actually exists here:

```
        Adapter (ELM327 / future J2534)
                    │
              Transport  ── serial, Bluetooth SPP
                    │
              ISO-TP reassembly            ← ours, working on 2 vehicles
                    │
        OBD-II  │  UDS  │  (KWP, J1850)     ← ours, working
                    │
            Diagnostic operation
                    │
        ┌───────────┴────────────┐
        │                        │
  Knowledge Resolver      Vehicle Identity
        │                        │
   ┌────┼─────┬──────┬───────┐   │
  OBDb  ours  as-built  user  observed
        │                        │
        └───────────┬────────────┘
                    │
            Normalised finding
              + provenance
                    │
              Safety engine          ← permission × risk, deterministic
                    │
                    AI               ← explains, correlates, prioritises
```

The load-bearing line is the same one the mandate draws, and this project
already enforces it: **the AI never decides which bytes go on the bus.** It
names a feature or an intent; deterministic code resolves that to a request.

---

## D. Knowledge provider interface

The one genuinely new abstraction this report proposes. Today the OBDb
catalogue, the feature catalogue and the as-built parser are three unrelated
things that answer overlapping questions.

```rust
/// Something that knows facts about vehicles.
trait KnowledgeProvider {
    fn id(&self) -> &str;              // "obdb", "measured", "as-built", "user"
    fn source_kind(&self) -> SourceKind;
    fn licence(&self) -> &str;

    /// What this provider can say about a vehicle, and how relevant it is.
    fn signals(&self, v: &VehicleIdentity) -> Vec<Candidate<SignalDef>>;
    fn features(&self, v: &VehicleIdentity) -> Vec<Candidate<FeatureDef>>;
    fn dtc(&self, code: &str, v: &VehicleIdentity) -> Option<Candidate<DtcInfo>>;
}
```

Every answer carries its provider, its licence and its relevance. The resolver
orders candidates; it does not merge them, because merging is how two sources'
disagreement becomes invisible.

**Authority order**, which we should state and enforce:

1. Measured on this vehicle, this session
2. Measured on this vehicle, earlier
3. OEM data the user supplied for this VIN (as-built)
4. Measured on a closely similar vehicle
5. Community catalogue for this model
6. Community catalogue for a related model
7. Generic standard (SAE)
8. Model knowledge — **never** promoted above this

---

## E. Vehicle identity

Currently a VIN decode and nothing else. Should become evidence-based, with the
pieces we already collect:

| Evidence | Have it? |
|---|---|
| VIN, and its structural decode | yes |
| Calibration IDs / CVN | yes |
| ECU software/part numbers (`F188`, `F1F3`) | yes — `EDC17CP65` identified a 6.7L Power Stroke |
| Which module addresses answered | yes |
| Which protocol and addressing width | yes |
| Supported PID bitmaps | yes |
| User statement | no |

That is already enough for a fingerprint. It needs assembling into one
`VehicleIdentity` carrying candidates and what each rests on, rather than a
single guessed answer.

---

## F. Safety integration

Unchanged, and deliberately. The mandate says "knowledge discovery should never
automatically grant write permission", which is exactly the existing rule:
a definition may say `writable: true` while the gate independently refuses.

One addition worth making: a provider's **relevance** must feed the gate. A
mapping measured on a *similar* vehicle should be readable and never writable
on that basis alone, however confident it looks.

---

## G. Implementation plan

PR-sized, ordered so each step is useful alone.

| # | Step | Why first |
|---|---|---|
| 1 | Normalise `VehicleIdentity` from evidence we already collect | Everything else keys off it |
| 2 | Extract `KnowledgeProvider`, port the three existing sources to it | No new data, pure refactor, behaviour unchanged |
| 3 | Resolver with the authority order above, candidates never merged | Makes conflicts visible |
| 4 | As-built import: VIN check, private storage, block↔DID bridge | Bridge already measured and committed |
| 5 | Candidate mappings + predict-and-check verification | Half-built already |
| 6 | Second manufacturer end to end, on a vehicle we can borrow | Proves the core is not Ford-shaped |
| 7 | Unknown-data capture and contribution workflow | Needs 1–3 to have somewhere to put it |
| 8 | Structured diagnostic context to the AI | Needs the normalised model |

Deliberately **not** on this list: replacing ISO-TP, UDS, or the transport
layer. They work, on real vehicles, and the proposed replacements cannot be
linked into a Rust binary.

---

## H. What this changes about the product

The mandate's closing framing is the right one and worth keeping:

> an open diagnostic reasoning platform, rather than an OBD-II app with an AI
> attached

The test is whether somebody with a Kia downloads this and finds a tool that
happens to have less knowledge about their car — not a Ford tool that tolerates
them. The core is close to that already; the knowledge is not, and knowledge is
the part that grows by being used.
