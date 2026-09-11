# Changelog

Notable changes, newest first. Versions follow [semantic versioning](https://semver.org),
with the caveat that everything below 1.0 is allowed to move.

## [0.3.1] — 2026-09-11

### Fixed

- **The update prompt never appeared.** The core could check for a newer
  release and install one; nothing in the interface ever asked it, so a 0.2.0
  install sat beside a published 0.3.0 and said nothing. An endpoint with tests
  is not a feature until something calls it.

  Note that this fix cannot reach an install older than 0.3.1: the code that
  does the asking ships *in* the update. One manual install, and after that
  updates announce themselves.

- The version comparison is now pinned by test, including the case that
  matters here — a release tag carries a `v` and a package version does not.

## [0.3.0] — 2026-09-11

Talk to any vehicle, not just the one this was built against.

### The bug that shaped this release

- **A hardcoded 11-bit broadcast header made every 29-bit vehicle look absent.**
  Fixed in five places: broadcast headers, the full-vehicle scan, module
  capture, the configuration write path, and the community catalogue. On a 2023
  Odyssey this turned "every protocol returns NO DATA" into a VIN, two modules,
  readiness and live data — and auto-detection then succeeded on the *first*
  probe, because the protocol sweep had only ever been compensating for our own
  malformed request.
- **29-bit addressing throughout**: `ATCP`/`ATSH` splitting for adapters that
  reject the one-command form, 29-bit response parsing, 29-bit address sweeping,
  and module addresses stored as text so `18DA10F1` is expressible.

### Knowing what a vehicle can do

- **Read-only capability probing**: which identifiers a module holds, which
  diagnostic sessions it grants, whether it implements security access. Never
  requests a programming session.
- **Write-gate probing**: establishes whether a module accepts writes at all, by
  asking it to write to an identifier it has just reported as absent. Nothing
  can land, and the refusal is the measurement.
- **A negative response code now carries its consequence.** `securityAccessDenied`
  and `requestOutOfRange` used to reach a person as the same shrug.
- **Uncatalogued codes say what can be measured next** instead of ending there.

### Knowledge from outside this project

- **Community signal definitions** in the OBDb format, matched to the vehicle,
  labelled by how closely, and never presented as measurement.
- **Factory as-built import**, with the block↔identifier correspondence measured
  on a vehicle rather than assumed.
- **One configuration mapping measured on a real vehicle**, scoped by exact VIN.

### Adapters that misbehave

- Bluetooth pairing creates two COM ports and only one reaches the adapter; the
  dead one is now named as such rather than listed beside the live one.
- A Bluetooth port no longer gets a baud sweep it cannot need — six open/close
  cycles thrashing an RFCOMM link was the "it connects and then disconnects".
- Line speed and protocol are remembered per adapter; connect went from 8519 ms
  to 2890 ms on a cable that answers at 500000.
- Errors are recognised when they arrive garbled, including truncated ones.
- Recovery from adapter glitches, and a learned response window rather than an
  assumed one.
- The socket is checked for power before nine protocols are tried on it.

### Interface

- The splash stays long enough to be seen, and a click or key skips it.
- The flight recorder can be exported, with the raw adapter lines intact.

### Known limits

- **A cheap ELM327 clone cannot perform a configuration write.** A 13-byte
  request comes back `?` from the adapter in 11 ms and never reaches the
  vehicle. STN hardware — OBDLink EX or MX+ — is what this needs.
- The interface lags the core: it can find a failing part without showing where
  it is, and buries past sessions below the fold.

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
