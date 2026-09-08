# Changelog

Notable changes, newest first. Versions follow [semantic versioning](https://semver.org),
with the caveat that everything below 1.0 is allowed to move.

## [0.1.0] — 2026-09-08

First release. It reads a real vehicle, and it has been run against one.

### Reading the vehicle

- **OBD-II (SAE J1979)** services 01, 02, 03, 04, 06, 07, 09 and 0A — live data,
  freeze frame, stored and pending codes, on-board monitor test results, vehicle
  information and permanent codes.
- **UDS (ISO 14229)** services 0x10, 0x19, 0x22 and 0x3E, which is what reaches
  the modules the emissions services cannot address.
- **Full-vehicle scan** across the 0x700–0x7EF diagnostic address range, asking
  every module that answers for its fault memory — and distinguishing a fault
  failing *now* from one merely stored.
- **Mode 06 self-test measurements**, showing each monitor's measured value next
  to the limit it is judged by. Pass/fail and margin are exact even where the
  unit scaling is unverified, because both sides share the scaling.
- **Emissions readiness**, per module, with the used-car reading: no stored codes
  plus unfinished self-tests usually means the codes were cleared recently.
- **Session comparison** — two sessions, what appeared, what cleared, what moved.

### The agent

- Bring your own model: Anthropic, xAI, Ollama, or any OpenAI-shaped endpoint.
  The tool layer is vendor-neutral JSON Schema, so the choice is a dropdown.
- Typed tool registry behind a capability gate that refuses anything not on the
  list, including anything the build compiled off.
- Provenance on every claim: `measured` from your vehicle, `catalog` from the
  standard, or `model_knowledge` — never blended, and repair costs are always
  the third one and always a range.
- **Owner or buyer**, and **practical / neutral / blunt** tone, both of which
  change what the report leads with.

### The app

- **Easy and Advanced** overlays over the entire UI. Easy puts a plain sentence
  where you are already looking and hides identifiers and hex; Advanced shows
  everything. Neither hides the evidence link.
- **Flight recorder**: every command and reply in an append-only log, reachable
  from any number on screen.
- Live data with a signal budget measured from what your adapter actually
  achieves rather than a hardcoded assumption.
- Vehicle knowledge as versioned data, not code — YAML in a folder, loaded at
  startup, labelled on screen with the file it came from.
- A **virtual vehicle** — a 2019 F-250 with five modules, realistic sensor drift
  and injectable faults — so all of the above works with no car present.

### Hardware

- Finds the adapter's line speed by sweeping, rather than assuming 38400. A
  wired FTDI cable at 500000 baud works.
- Recognises STN-series hardware and uses the extra throughput.
- Tries each protocol explicitly instead of trusting automatic detection.

### Known at the time of release

See [Status](README.md#status). The short version: Windows-only in practice,
Mode 06 unit scalings unverified, no manufacturer-specific decoding yet,
configuration writes compiled off by design.
