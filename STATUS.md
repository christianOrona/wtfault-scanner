# Status

Where the project actually is, updated when something significant changes.
Not a changelog — see `CHANGELOG.md` for releases — and not a diary.

Last reviewed: **2026-09-10**

---

## Verified on real hardware

Evidence, not intention. Everything here has run against a vehicle.

| | |
|---|---|
| Vehicles seen | 3 real (2019 F-250 ×2, 2012 F-250), plus the virtual truck |
| Adapters | FTDI USB at 500000 baud, Bluetooth ELM327 clone at 38400 |
| Sessions recorded | 34 |
| Adapter exchanges logged | ~29,900 |

OBD-II services 01–0A, UDS 0x10/0x19/0x22/0x2E/0x3E, full-bus module sweep,
Mode 06, readiness, live data, session comparison, flight recorder.

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

## What is not built

Honest gaps, in the order they matter.

- **No verified configuration mapping ships for any real vehicle.** Every
  catalogue entry has `mapping: null`. The write path is complete and proven
  against the virtual vehicle; nobody has measured a real one. This closes with
  a profile file, not a release.
- **Windows only in practice.** The core is portable and CI builds it on Linux;
  the desktop shell has only ever been built and run on Windows.
- **Mode 06 unit scalings unverified.** Pass/fail and margin are exact because
  both sides share the scaling; the units are a best guess and labelled as one.
- **No manufacturer-specific decoding.** Everything is the public standard,
  which is why it works across brands and also why a module can answer with a
  code nobody has a description for.
- **Profile import is folder-only.** Dropping a YAML file in works; importing
  one from a URL with a verification count does not exist.
- **MS-CAN is detected but never used.** The adapter can be switched to a second
  bus; nothing in the app asks it to yet.
- **Provider API keys are stored in plain text** in the user profile. The OS
  credential store would be better and is not implemented.

## Permanently out of scope

Not gaps. Decisions, and no amount of evidence changes them.

Anything in the braking, steering or throttle path. Immobilisers and keys.
Firmware. Anything that defeats an emissions control. See `docs/SAFETY.md`.
