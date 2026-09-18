# Status

Where the project actually is, updated when something significant changes.
Not a changelog — see `CHANGELOG.md` for releases — and not a diary.

Last reviewed: **2026-09-18**

---

## Verified on real hardware

Evidence, not intention. Everything here has run against a vehicle.

| | |
|---|---|
| Vehicles seen | 3 real (2019 F-250 ×2, 2012 F-250), plus the virtual truck |
| Adapters | FTDI USB at 500000 baud, Bluetooth ELM327 clone at 38400, OBDLink MX+ (STN) |
| Sessions recorded | 34 |
| Adapter exchanges logged | ~29,900 |

OBD-II services 01–0A, UDS 0x10/0x19/0x22/0x27/0x2E/0x3E, full-bus module sweep,
Mode 06, readiness, live data, session comparison, flight recorder.

A guided procedure reached its measured state on a real engine for the first
time on 2026-09-11: `warm_idle` went `waiting` → `holding` → `measured` at 86 °C
coolant and 600 rpm. It also proved the procedure was measuring nothing it
existed for — see below.

**A setting was changed on a real vehicle and the vehicle behaved differently,
for the first time, on 2026-09-13.** "Turn off the double honk after I leave the
cabin": `DE28` byte 6 on the body control module at `72E`, `01` → `00`, written,
read back, ignition cycled, and the owner confirmed the horn no longer chirps.
The confirmation is the claim, not the read-back — this project has already
measured a case where four bytes read back correctly and nothing moved.

## Findings from real sessions

Mined from the local session database. These are measured counts, not guesses.

### Certain PIDs time out repeatedly and we keep asking

`0121`, `011C` and `011F` each hit the 6-second request ceiling, across ~1,350
requests. A PID that has timed out several times in a session is not going to
start answering, and each retry costs six seconds of a person's time.

**Fixed.** A parameter that times out three times running on a module stops
being asked for the rest of the session. Reported as "stopped being asked",
not as unsupported: a timeout is not a negative response and the vehicle never
said no.

### A real code with no catalogue entry

`P0269` was read from a vehicle — confirmed, pending and permanent — and has no
description. The structural decoding is shown and no description is invented,
which is the designed behaviour, but this is a generic SAE code that belongs in
the shipped catalogue.

**Fixed.** The whole cylinder contribution/balance range is catalogued, not
just the code that was reported - the next one to turn up will be its
neighbour.

### Adapter-level failures are more common than expected

Across all sessions: `stopped` 611, `not_understood` 264, `timeout` 345,
`bus_error` 73, `unable_to_connect` 12. `stopped` in particular is high enough
to be worth understanding rather than absorbing.

**Open.** Needs investigation before it is worth acting on.

### Protocol detection accepted a failed bus init

Fixed. On a CAN-only vehicle the app settled on ISO 9141-2 because
`BUS INIT: ...ERROR` classified as informational, and informational counted as
success. The classifier now treats a failed bus init as a bus error, and the
protocol sweep requires actual data before declaring a protocol found.

### Session database reported as malformed

Fixed, and it was never corrupt. A healthy database paired with a write-ahead
log belonging to a different database produces exactly that error; the
mechanism was reproduced deliberately. The failure message now names the file
and says the history is probably not lost.

### UDS fault reads returned hundreds of records that were not faults

Every `59 02` reply recorded from a 2019 F-250 across four sessions — about
4,000 records, ~480 per full scan — had status `0x40` or `0x50`: self-test not
completed. None had a failing, pending or confirmed bit. The full scan counted
them as faults and the per-module read called them pending. A 2012 F-250
returned 543 records, of which 5 had fault bits set.

**Fixed.** Only records with one of the low four status bits set are listed,
counted or stored; the rest are counted in a `uds_codes_not_faults` note. Not yet
re-run on the vehicle.

### A long UDS fault reply was cut off by a timeout

One `59 02` reply on the 2012 F-250 announced 407 bytes and ended in `timeout`
partway through. The decoder drops the partial tail without saying so, so codes
past the cut are silently missing.

**Open.**

---

## What is built

- **Configuration writes**, behind a typed confirmation, the full precondition
  list, and mandatory read-back verification. A module answering "accepted" is
  not treated as a changed setting.
- **Reading a feature's current setting**, including the case where no mapping
  exists — which reports what is known, what is not, and the four steps that
  would establish the rest.
- **Configuration capture and diff**, the procedure that turns an unmapped
  feature into a mapped one.
- **Per-operation verification.** Reading and writing carry separate evidence,
  and write evidence is absent by default.
- **Evidence-derived confidence.** The report type has nowhere for a model to
  rate itself; confidence is computed from what a finding cites.
- **Adversarial replay tests.** A scripted provider replays a badly-behaved
  model so the guarantees can be tested without spending API credit.
- **Two outside sources, on request only.** NHTSA vPIC decodes a VIN to make,
  model, year, engine and fuel — the things the VIN standard does not encode and
  this project will not guess at. OBDb supplies community signal definitions for
  that make and model. Each is a button that says what it sends before it sends
  it, and each reply is kept, so the same question is never asked twice.
- **Knowledge kept against the VIN, and a number for it.** What a visit
  establishes — the protocol it reached the vehicle on, the PIDs each module
  supports, what each module reports about itself, a module that refused a read
  and why — is stored against the VIN. The scorecard turns that into counts, so
  whether a second visit starts ahead of the first is measurable rather than a
  feeling.
- **Sessions replay without the vehicle.** A recorded session exports as a
  transcript with the VIN anonymised, and CI replays recorded sessions and fails
  when a replay discovers less than the original did.

## What is not built

Honest gaps, in the order they matter.

- **Neither outside source has been used from a driveway.** The vPIC lookup and
  the OBDb fetch were built against the simulator and recorded fixtures. Both
  reach a real service over the network on a real VIN, and neither has been run
  with a truck plugged in.
- **An OBDb signal set is a list of claims, not measurements.** A fetched set
  says what somebody recorded for this make and model; nothing in it is verified
  until it has been read on the vehicle in front of you, which is what the
  finding it records says. It also does not shorten the mapping problem below —
  OBDb documents signals, never configuration.
- **One verified configuration mapping ships, and one is not a catalogue.**
  AutoLock on the 2019 F-250 was written to a real truck on 2026-09-11 — DID
  `DE0E`, byte 4, `01` → `00` — and confirmed by the owner seeing it change on
  the dash after an ignition cycle. That is the whole of it. Every other
  catalogue entry still has `mapping: null`, and the honest reading of one
  success is that the mechanism works, not that the catalogue does. This closes
  with profile files, not releases.
- **The write needed a key cycle before it showed.** The module accepted the
  write and reported the new value immediately; the dash kept the old behaviour
  until the ignition was cycled. Any feature whose verification does not include
  a key cycle has not actually been verified.
- **Windows only in practice.** The core is portable and CI builds it on Linux;
  the desktop shell has only ever been built and run on Windows.
- **Mode 06 unit scalings unverified.** Pass/fail and margin are exact because
  both sides share the scaling; the units are a best guess and labelled as one.
  A 2019 F-250 returned zero monitor tests, which is plausible for a diesel and
  has not been confirmed as correctly-none rather than silently-none.
- **Nothing knows what fuel the engine burns.** PID `0x51` reports it and is
  never asked. A gasoline-shaped procedure therefore runs on a diesel and
  measures nothing: `warm_idle` exists for fuel trims, a diesel has none, and
  until 2026-09-11 it reported success anyway. It now says what it could not
  measure, but a procedure still cannot declare which engines it applies to.
- **No diesel-specific signals.** DPF load, regeneration state, DEF level and
  SCR temperatures are all absent. Most are manufacturer-specific rather than
  legislated, so they belong in profile data rather than the core catalogue.
- **No manufacturer-specific decoding.** Everything is the public standard,
  which is why it works across brands and also why a module can answer with a
  code nobody has a description for.
- **Profile import is folder-only.** Dropping a YAML file in works; importing
  one from a URL with a verification count does not exist.
- **The second bus is measured, and the code that reaches it is not yet
  proven.** On 2026-09-11 a 2019 F-250's secondary bus was mapped by hand
  through the adapter: 500 kbit/s on pins 3 and 11, 29 modules answering
  TesterPresent across `700-7FF`, 22 of them holding as-built configuration
  blocks. The app had been seeing two modules and calling that the vehicle.

  Everything that measurement exposed has been fixed — the bus type no longer
  names a speed, the capability flag is actually set, the switch uses the
  commands measured to work, discovery sweeps by address instead of
  broadcasting, and the range reaches the `7F1` where a real module answered. A
  full scan now sweeps every bus the adapter can reach rather than only the one
  it started on, and puts the adapter back where it found it.
  **None of it has been run against a vehicle.** The hand measurement says what
  is there; it does not say the code now finds it.
- **Silence on the second bus stays silence.** An adapter that accepts every
  bit-rate command is not necessarily wired to pins 3 and 11, and nothing on the
  wire distinguishes "this vehicle has nothing there" from "this cable cannot
  hear it".
- **No configuration mapping can be obtained without a vehicle.** There is no
  open, redistributable dataset of as-built bit meanings for any manufacturer:
  OBDb documents signals rather than configuration, the one open Ford decoder
  found covers a single infotainment module and keeps its definitions in code,
  and the rest is forum prose. A mapping therefore comes from measuring a
  vehicle — capture, change the setting by some other means, capture, diff — and
  that middle step is not a software problem. Candidates from community
  documentation can be carried, and this build will read one and refuse to write
  it, which is the correct behaviour and also a hard ceiling on how much can be
  finished at a desk.
- **Provider API keys go to the OS credential store** where there is one. On a
  machine without one the key stays in the settings file in plain text, and the
  Settings screen says so against that key rather than leaving anyone to assume
  otherwise.
- **Nothing the app writes has ever been reported to anybody.** It keeps a log
  and can assemble a problem report; moving one anywhere is still a person
  copying text.

## Permanently out of scope

Not gaps. Decisions, and no amount of evidence changes them.

Anything in the braking, steering or throttle path. Immobilisers and keys.
Firmware. Anything that defeats an emissions control. See `docs/SAFETY.md`.
