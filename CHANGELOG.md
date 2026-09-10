# Changelog

Notable changes, newest first. Versions follow [semantic versioning](https://semver.org),
with the caveat that everything below 1.0 is allowed to move.

## [0.2.0] — 2026-09-10

Configuration writes, and the machinery that makes them safe to have.

### Changing a setting, not just reading one

- **Configuration writes**, behind a typed confirmation, a full precondition
  list, and mandatory read-back verification. A module answering "accepted" is
  not treated as a changed setting: the record is read again and compared, and
  a write that cannot be verified is reported as unverified with the final
  state unknown.
- **Reading a feature current setting**, including the case where no mapping
  exists - which reports what is known, what is not, and the four steps that
  would establish the rest, rather than a refusal.
- **Configuration capture and diff**: the procedure that turns an unmapped
  feature into a mapped one. Capture, change the setting with a tool that
  already knows how, capture again, and the bits that moved are the mapping.
- **Per-operation verification.** Knowing where a setting lives is not knowing
  that it can be changed. Reading and writing carry separate evidence, and
  write evidence is absent by default.
- **One complete capability ships**, scoped by exact VIN to the built-in
  virtual vehicle, so the whole flow works out of the box with no car.

### Honesty about what is known

- **Confidence is derived from evidence, never asserted by the model.** A
  finding claiming to be measured with nothing to cite does not get measured
  confidence.
- **Severity and confidence are separate.** A serious problem the evidence does
  not establish now says both things.
- Two permanent ceilings: a permission level, and a risk class that refuses the
  braking, steering and throttle path, immobilisers, keys and firmware as
  policy rather than as an unfinished feature.

### Fixed from real driving

- **A failed bus init no longer passes as a working protocol.** On a CAN-only
  vehicle the app used to settle on ISO 9141-2 because `BUS INIT: ...ERROR`
  classified as informational.
- **Parameters that time out stop being asked.** Three PIDs were each hitting a
  six-second ceiling across roughly 1,350 requests.
- **The cylinder contribution range is catalogued**, after P0269 was read from
  a real vehicle with no description.
- **A database is no longer reported as malformed when it is not.** The message
  now names the file and says the history is probably not lost.
- **A second CAN bus is detected rather than assumed absent.**

### Safety

- The assistant cannot change a vehicle setting or clear codes. That is an
  explicit capability flag with a registry-wide test, not a side effect of a
  build ceiling - which is what it used to be, and which would have silently
  handed a model those operations when writes were enabled.

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
