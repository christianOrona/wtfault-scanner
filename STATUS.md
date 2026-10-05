# Status

Where the project actually is, updated when something significant changes.
Not a changelog — see `CHANGELOG.md` for releases — and not a diary.

Last reviewed: **2026-10-04**

---

## Verified on real hardware

Evidence, not intention. Everything here has run against a vehicle.

| | |
|---|---|
| Vehicles seen | 4 real (2019 F-250 ×2, 2012 F-250, 2023 Honda Odyssey), plus three virtual ones: the truck, a 29-bit petrol Honda, and a 2004 Toyota on the K-line |
| Adapters | FTDI USB at 500000 baud, Bluetooth ELM327 clone at 38400, OBDLink MX+ (STN) |
| Sessions recorded | 41 on a real adapter |
| Adapter exchanges logged | ~54,500 |

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

### The first vehicle that is not a Ford

A 2023 Honda Odyssey, 3.5 L petrol, through the OBDLink MX+ on 2026-10-04: one
session, 13,885 exchanges, now a CI replay. It connected on 29-bit CAN at
500 kbit/s with no bus error on the main bus, identified from its VIN, and a
full scan found 13 modules with no fault that had a fault bit set. Self-test
results were read from the engine. What it showed that the truck could not:

- **Nothing is on its second-bus pins, and the scans asked every address
  there anyway.** Listening heard nothing at 500, 250 or 125 kbit/s, and every
  request then came back `CAN ERROR`: 3,738 of them across one module scan and
  three full scans, about a minute of each full scan. **Fixed** in the simulator
  it now reproduces exactly (534 per module scan); a sweep stops after three in
  a row. Not yet re-run on the vehicle.
- **Its engine has no PID 05.** It lists 59 service 01 PIDs, and coolant
  temperature, intake air temperature and air flow are only on 67, 68 and 66.
  This build decoded none of them, so the car had no coolant temperature at
  all and *Warm idle* could never see a warm engine. Decoders for 66, 67, 68
  and A6 (odometer) are **added, unverified**; the guided tests still ask for
  PID 05. Eighteen of its PIDs have no decoder.
- **Every module refused every standard identification identifier**
  (`7F 22 31` to F187, F188, F18A, F191, F195, F197 and Ford's two), so 11 of
  13 are still listed by address. The engine and transmission name themselves
  and report a calibration identification through service 09.
- **Those calibration identifications were read and kept nowhere.** **Fixed**:
  the software identity read stores them on the module.
- **One module's fault list was cut off** at 97 of 163 bytes, twice: it
  answers "pending" first and the full scan stopped listening at 1.17 s.
  **Fixed, unverified**: it is given 2.5 s.
- **The Full scan screen lost a finished scan** when another tab was opened,
  and the readiness card blamed the vehicle for being asked before the first
  scan. Both **fixed**.
- **Its recording could not be exported.** The VIN anonymiser did not read a
  29-bit address printed as four bytes, left the VIN in place, and the export
  refused, as designed. **Fixed.**

The owner read 27,224 on the odometer that day, which is what PID A6 will be
checked against.

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

**Not seen in the latest session.** The `STOPPED` replies were traced to the
address sweep sending the next header while the adapter was still listening
for the last probe, and the adapter now resends the command it never got. The
2026-09-28 full scan of the same truck, now a CI replay fixture, has 326
header changes and no `STOPPED` at all, no bus errors, one `?` (the
multi-PID request a v1.4b clone does not support) and two `BUFFER FULL`.
Whether the resend or the bus-selection fix ended them cannot be told apart
from one session.

Counted again on 2026-10-04 from the session database, which does record
timeouts: across every session on a real adapter, `stopped` 1,921, `timeout`
1,657, `not_understood` 258, `bus_error` 141, `unable_to_connect` 19. Every
`stopped` and every `timeout` is from 2026-09-13 or earlier. The 2026-09-28
visit was five sessions and 12,794 replies with neither.

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
counted or stored; the rest are counted in a `uds_codes_not_faults` note.
Re-run on the 2019 F-250 on 2026-09-28: 725 records set aside, 8 real faults
in 6 modules.

### A full scan after a module scan found nothing

Measured 2026-09-28 on the 2019 F-250. The module scan went back to the
primary bus with `ATSP0`; the adapter then searched again on the next request,
and a search that starts with the non-CAN protocols outlasts every discovery
deadline. All 510 addresses were silent, and each header change answered
`STOPPED`, which may account for part of the `stopped` count above.

**Fixed.** The primary bus is selected by the protocol already negotiated. Same
truck, same sequence: 35 modules, 6 primary and 29 secondary.

### Mode 06 called passing tests failed, and most of the rest marginal

The same truck returned 54 service 06 results — not the zero recorded before —
and the app called 19 failing with the MIL off. Every one used a unit id of
`0x80` or above, which J1979 defines as signed, compared unsigned. Of the rest,
23 were "within ten percent of failing" because a limit of 0 or 65535 (no limit
at all) was measured against.

**Fixed.** Signed ids compare signed; a limit at the end of the range is no
limit. Same truck: 0 failing, 1 marginal, a genuinely two-sided boost test.

### Community signals were never asked on the bus their module is on

Every OBDb definition was sent from the primary bus. On the F-250 the body
modules are on the second bus, so all 349 definitions tried came back `NO DATA`
and were reported as not describing the vehicle.

**Fixed.** 74 now decode on the truck, from OBDb's make-level `Ford` set (the
`Ford-F-250` set is empty; the fetch now falls back to the make's). Two agree
with standard readings from the same visit: coolant 29.9 °C against PID 05's
30 °C, battery 12.3 V against `ATRV`. The odometer (157,473.8 km) and all four
tire pressures (52.8 / 54.1 / 60.3 / 60.6 psi) were confirmed by the owner
against the dash. These are the first community signals verified on a real
vehicle, and they are recorded against the VIN as established. The rear inner
pair read 150 psi on a single-rear-wheel truck: a no-sensor value, recorded as
ruled out.

### Ford modules refused every standard name, and answered Ford's own

Measured 2026-09-28 on the 2019 F-250: all 36 modules refused F197, F187, F18A
and F191 (`requestOutOfRange`), so 34 stayed "Module at ...". 32 of 35 answered
F188, F113 and F111 with Ford part numbers; 744, 776 and 7F1 answered none.

**Fixed, in part.** Those are read, and a module is named when its part-number
base is one of twelve in `vehicle-profiles/ford/modules.yaml`: 12 of 36 named.
The other bases (19H423 at 7B1, 7H417 at 761, 2C006 at 757 and more) are
recorded and not guessed at. Two more module paths found not switching buses on
the way — the capability probe, and the identity view, which also wiped the
scan's identity — are fixed.

### Diesel exhaust fluid is a legislated PID, not manufacturer data

The truck supports about 40 standard PIDs this build had no decoder for,
including DPF (7A), NOx reagent (85), aftertreatment status (8B) and DEF sensor
(9B). DEF level is now decoded from 9B and 85, which agree at 41.2 %. The
others wait for a documented byte layout; raw engine-off bytes for all of them
are in the session scratchpad of 2026-09-28.

### A long UDS fault reply was cut off by a timeout

One `59 02` reply on the 2012 F-250 announced 407 bytes and ended in `timeout`
partway through. Every code in it was lost, and the module was reported as not
answering.

**Fixed, without the vehicle.** Both fault reads keep the whole records that
arrived and say the list stopped: how many bytes were announced, how many came,
and that it is not the module's whole fault memory. Only these reads accept a
partial reply; anything read back to verify a write still refuses one. Tested by
replaying the simulator's session with the last frame of a three-record list
removed, not yet against the 2012 truck.

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
- **An Android build of the same app.** The core, the API and the interface are
  shared; a Kotlin plugin reaches a paired adapter over Bluetooth Classic and is
  listed and opened like a port. CI builds a debug APK on every push and a
  signed one for every release.
- **What one device records, another can take in.** A database export from
  one install merges into another's: sessions are filed under the vehicle by
  its VIN, the newer finding wins, and a session exported while still open is
  finished by a later export. Measured on a copy of the development database:
  78 sessions and 177,235 events came across identical, and a second import of
  the same file added nothing. No file from a real phone has been imported.
- **Which software a module runs, and whether a file is that software.** A
  module's calibration identification and verification number are read with
  their evidence, every identifier it refuses is kept as a refusal, and a
  calibration file on the computer is judged against them: exact only when the
  module's own identification equals one declared for the file. Nothing is
  downloaded and nothing is written. Run against the simulated Odyssey only.
- **Sessions replay without the vehicle.** A recorded session exports as a
  transcript with the VIN anonymised, and CI replays recorded sessions and fails
  when a replay discovers less than the original did.
- **A second virtual vehicle that is not a Ford in any way the Ford was
  convenient.** A 2023 Honda Odyssey: petrol, 29-bit CAN, its engine at
  `18DAF110` and transmission at `18DAF11E` as measured on the real one, and a
  freeze-frame answer the Ford never gives. Running every flow on it found
  five things that were only right on the Ford: procedures that looked for
  `7E8`, 29-bit replies with no request address, a missing freeze frame read
  as an adapter failure (and a `0000` one as a frame for P0000), an empty
  settings list with no reason, and screens that called every vehicle a
  truck. A simulator is not a car: #39 still needs the real one.
- **A third, from before CAN.** A 2004 Toyota on ISO 9141-2. It found that
  every pre-CAN reply had its checksum read as data, and multi-frame replies
  joined wrongly (the VIN and fault lists came out garbled). It also found
  that the full scan and the module probes asked CAN questions down a
  K-line and reported the silence as the vehicle's. The frame format now
  follows the ELM327's documented output; no pre-CAN vehicle has been
  connected.
- **Updates are checked before they are kept.** A downloaded installer must
  match the SHA-256 its release states, in `SHA256SUMS.txt` and in GitHub's
  record of the file. That catches a damaged or swapped download; it does not
  replace signing, which is what would catch a replaced release.

## Pending, as of 2026-10-04

Desk work that is ready to pick up:
- Wideband oxygen sensors: PIDs 24-2B (voltage, `(256C+D)*8/65535` V) and
  34-3B (current, `(256C+D)/256 - 128` mA), both checked against python-OBD.
  Lambda from bytes A-B needs a second output for numeric PIDs, which the file
  format does not have yet.
- PID 13 (which O2 sensors are present): the two sources disagree on which
  nibble is bank 1. Needs a third source before it is decoded.

Needs the vehicle or the owner (steps in `docs/AT-THE-CAR.md`):
- #54 cold-start baseline on the F-250 (`dev-core.ps1 -Db` is ready).
- The 2023 Odyssey again, to check what was fixed after its first scan and
  the four new readings (`docs/AT-THE-CAR.md`, 3a). Its first scan is done and
  its replay is in CI (#39, #59).
- #64 the Android build against the paired adapter, then the screen staying
  on and a database export through the share sheet. A recorded drive waits on
  all three.
- #65 one inspection on a real OpenRouter key.
- #50 host the report endpoint, then build a release with `AIM_REPORT_ENDPOINT`.
- #62 SignPath approval, then #48's unattended install.
- Pre-CAN support has only met the simulator; the first K-line or J1850 car
  is its real test.

## What is not built

Honest gaps, in the order they matter.

- **The Android app has never met a real adapter.** It builds in CI, installs,
  and runs the whole core in the emulator against the virtual vehicle. The
  Bluetooth link to a paired ELM327 is written but unverified until a phone
  connects to one in the driveway (#64). The same goes for the two things a
  drive needs from the phone: the screen staying on while an adapter is
  connected, and a session leaving through the share sheet. The export itself
  is tested and has been run on the desktop; its Android half has not run
  anywhere yet.

- **Both outside sources have been used from a driveway once** (2026-09-28,
  the 2019 F-250). vPIC decoded it as a 6.7 L V8 diesel F-250 and settled the
  model on the scorecard; OBDb had nothing for the model and the make-level set
  was fetched instead. One vehicle, one make.
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
  A 2019 F-250 returned 54 on 2026-09-28; earlier visits recorded none, and why
  is not known.
- **Procedures know which engines they apply to, from one kind of signal.**
  Each procedure declares its engines, and the engine's own PID `0x51` is read
  before anyone is asked to do anything: `warm_idle` (fuel trims) is not
  offered to a diesel, and a diesel holding 2500 rpm is measured without the
  fuel trims it cannot have. Fuel trims are the only signals this build knows
  to be engine-specific; an engine that does not answer `0x51` is given every
  procedure, as before.
- **Few diesel-specific signals.** DEF level is read; DPF pressure,
  regeneration state and SCR data are carried by legislated PIDs the truck
  supports (7A, 8B, 85, 83) and are not decoded, for want of a documented
  byte layout.
- **Pre-CAN vehicles are read from the documentation, not from a car.** J1850
  and K-line replies follow the ELM327's documented frame format and are
  tested against a simulated 2004 Toyota; no such vehicle has been connected.
  On them the full scan reaches the emissions modules only, self-test results
  (whose pre-CAN layout differs) are not decoded, and nothing that needs UDS
  is offered.
- **A calibration file has to be supplied.** No source of manufacturers'
  files is built in, so for nearly every module the answer is that none was
  found. A Honda `.rwd` is hashed and matched on what is declared about it and
  is not opened. On the 2023 Odyssey only the engine and transmission say what
  they run; its other eleven modules refuse every standard identifier.
- **No manufacturer-specific decoding.** Everything is the public standard,
  which is why it works across brands and also why a module can answer with a
  code nobody has a description for.
- **A setting from a profile cannot be changed until it has been verified on
  the vehicle.** A profile can be added from the Settings screen and is listed
  on the next connection, but its mappings arrive unverified whatever the file
  claims, and only a verified mapping permits a write. Verifying one is still
  the hand procedure: capture, change the setting some other way, capture,
  compare. Nothing finds a mapping for you, online or otherwise.
- **Profile import has never been used with a real shared profile.** A YAML
  file can be dropped in the folder, pasted, or fetched from an `https://`
  address after a preview of what it would change; nobody has published one.
- **The second bus is reached by the app, on one truck.** On 2026-09-11 a 2019 F-250's secondary bus was mapped by hand
  through the adapter: 500 kbit/s on pins 3 and 11, 29 modules answering
  TesterPresent across `700-7FF`, 22 of them holding as-built configuration
  blocks. The app had been seeing two modules and calling that the vehicle.

  Everything that measurement exposed has been fixed — the bus type no longer
  names a speed, the capability flag is actually set, the switch uses the
  commands measured to work, discovery sweeps by address instead of
  broadcasting, and the range reaches the `7F1` where a real module answered. A
  full scan now sweeps every bus the adapter can reach rather than only the one
  it started on, and puts the adapter back where it found it.
  Run on 2026-09-28: the module scan and full scan both found the 29 modules
  the hand measurement found, at 500 kbit/s. One vehicle, one adapter.
- **Silence on the second bus stays silence.** An adapter that accepts every
  bit-rate command is not necessarily wired to pins 3 and 11, and nothing on the
  wire distinguishes "this vehicle has nothing there" from "this cable cannot
  hear it".
- **Security gateways are known from a table, not measured.** One entry: the
  FCA / Stellantis Secure Gateway, from public documentation. A listed vehicle
  is told about it and its write plans say so; a module measured accepting
  writes overrules the listing. No gateway vehicle has been connected.
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
- **Problem reports can be sent, but nowhere receives them yet.** Send posts
  the report to an address compiled into the build, with identifiers withheld
  by default, and the endpoint that receives it is in `apps/report-endpoint`.
  No release has been given an address, because no endpoint is running.

## Permanently out of scope

Not gaps. Decisions, and no amount of evidence changes them.

Anything in the braking, steering or throttle path. Immobilisers and keys.
Writing firmware: programming, flashing or modifying a calibration. Anything
that defeats an emissions control. See `docs/SAFETY.md`.

Reading which software a module runs is not writing it, and is built; see
`docs/CALIBRATION.md`.
