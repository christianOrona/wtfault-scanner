# Calibration identity and calibration files

What software a control module is running, and whether a file on your computer
is that software.

**This is read-only.** The app reads what a module says about its software. It
does not flash, program, patch, convert or tune, and there is no code in it
that could. See [What it does not do](#what-it-does-not-do).

## What it does

1. **Identify the module's software.** *Inspect* → pick a module →
   *Software on this module* → **Read software identity**. The app asks the
   module what it runs and shows each answer with where it came from.
2. **Find a calibration file.** **Find calibration file** looks through a
   folder on your computer and says, for each file there, whether it is that
   module's calibration: exact, partial, no, or cannot tell.

The second step never touches the vehicle and never uses the network.
**Finding nothing is the usual answer**, and the app reports it as an answer
(`NO_ARTIFACT_FOUND`), not as a failure. Calibration files are the
manufacturer's and mostly reach nobody outside a dealership.

## What the module is asked

Only requests that read, in whatever session the module is already in:

| Request | What it gives |
|---|---|
| OBD-II service 09, type 0A | The module's name |
| OBD-II service 09, type 04 | Calibration identification |
| OBD-II service 09, type 06 | Calibration verification number (the module's own checksum of its calibration) |
| UDS `22 F180`, `F181`, `F182` | Boot software, application software and application data identification |
| UDS `22 F187`, `F188`, `F189` | Part number, software number, software version |
| UDS `22 F18A` | Supplier |
| UDS `22 F191`, `F192`, `F193` | Hardware number and version |
| UDS `22 F194`, `F195` | Supplier's software number and version |
| UDS `22 F197` | System name |

The UDS identifiers are the ones ISO 14229-1 defines for every manufacturer.
**No manufacturer-specific identifier is asked for**, because none has been
verified: an identifier this project has not seen answered is not guessed at.
`F18C`, the module's serial number, is deliberately not read.

No diagnostic session is opened, no security access is requested, and nothing
is written, reset or programmed. A test reads back every request a session
sent and fails if any of them is not a read.

## The evidence

Every identifier is kept with:

- the request it came from,
- the bytes it was decoded from,
- the flight-recorder row holding the module's whole reply (the *evidence*
  link on screen).

Every identifier that was **asked for and not given** is kept too, with what
the module said: refused (and the refusal's bytes), did not answer, or
answered with something that is not text. Nothing is filled in. If a reply
cannot be parsed, the reply is kept and the value stays unknown.

The calibration identification is also stored on the module's record and as a
finding against the vehicle, so it travels with a database export.

## What a 2023 Honda Odyssey reports

Measured on 2026-10-04, and what the simulated Odyssey reproduces:

| Module | Calibration identification | Verification number |
|---|---|---|
| Engine, `18DAF110` | `37805-5MR-C120` | `16B6A354` |
| Transmission, `18DAF11E` | `28102-5MX-A200` | `550C681C` |

All thirteen modules refused every standard UDS identifier tried that day
(`7F 22 31`, request out of range). So on this vehicle the calibration
identification is known for the engine and transmission, and **no hardware
number, part number or software version is known for any module**. The other
eleven modules report nothing that identifies their software.

Honda status, plainly:

| | |
|---|---|
| Honda module identification | **Partial.** Calibration identification and verification number through the legislated service. No Honda-specific identifier is read. |
| Honda calibration discovery | **Partial.** A file you supply is matched. No source of Honda files is built in. |
| Honda `.rwd` files | **Partial.** Recognised by name, hashed, cached and matched on what is declared about them. Not opened: no parser ships. |
| 2023 Odyssey | **Partial.** Both calibration identifications above were read from the real vehicle by 0.6.0's ordinary identification. The identity read described here has run on the simulated Odyssey only. |

## Calibration files

### Where they come from

A folder on your computer: `calibrations\files` beside the session database
(the *Find* result names the full path, and a `README.txt` there explains it).
Put files there that you are entitled to have.

**The app downloads nothing.** No calibration source that uses a network is
built in, because this project knows of no public source it is entitled to
fetch manufacturers' files from. The source is an interface
(`CalibrationSource`), so one can be added without changing the matching; until
then, a file nobody supplied is a file not found.

### Kinds of file

`.rwd`, `.bin`, `.gz`, `.hex`, `.s19`. They are not the same kind of thing:

- `.hex` and `.s19` are addressed memory as text, and the app checks the
  content really is that.
- `.gz` is checked for the gzip signature. What is inside is not asserted.
- `.bin` is bytes with no description. Nothing in it can be checked.
- `.rwd` is Honda's update package. **The app does not open it.** It is
  recognised by its name, hashed and kept exactly as it is.

No file is ever modified, converted or rewritten.

### Saying what a file is

A file's name proves nothing: it is whatever the last person to rename it
typed. To let the app say more than "partial", put a metadata file beside the
calibration file, named `<the whole file name>.json`:

```json
{
  "sha256": "the SHA-256 the file should have",
  "calibration_id": "37805-5MR-C120",
  "hardware_number": ["..."],
  "make": "Honda", "model": "Odyssey", "model_year": [2023],
  "engine": "3.5L V6"
}
```

Every key is optional. The keys are the field names in
`core/calibration/src/identity.rs`; a key that is not one is ignored. You are
stating these things, and the app records that they were *declared*, by that
file.

### The cache

A file that matches (exactly or partially) and is not invalid is copied to
`calibrations\kept\artifacts\<sha256>.<ext>`, with its record in
`calibrations\kept\metadata\<sha256>.json`. The SHA-256 is the file's identity.
The same file found twice, under any two names, is one file with one record
listing both places. Kept files are searched too, so a file found once is
still found after the folder is emptied.

## Matching

Four answers and no percentage. Each is a rule applied to a list of
comparisons you can read on screen.

| Answer | Rule |
|---|---|
| `EXACT_MATCH` | The module's own calibration identification equals one **declared** for the file, and nothing both sides state disagrees. |
| `PARTIAL_MATCH` | Something agrees, and the calibration identification is not among the things confirmed. |
| `NO_MATCH` | Anything both sides state disagrees. One conflict is enough, however much else agrees. |
| `UNKNOWN` | Nothing could be compared. |

Each comparison is one of:

- **YES**: the module reports it and the file is declared to be the same.
- **CONFLICT**: both state it and they differ.
- **name only**: the file's *name* agrees. Recorded, and it can make a match
  partial. It can never make one exact.
- **name differs**: the file's name suggests something else. Not a conflict.
- **UNKNOWN**: one side does not state it.

An exact match says what it could not establish. On the Odyssey a file can
match the engine's calibration identification exactly while its hardware
number stays unknown, because the module never reported one, and the result
says so in words.

Makes, models and engines are compared as words: `Honda (US)` from a VIN and
`Honda` in a file's metadata agree. A make or model sharing no word with the
other side's is a conflict. An engine worded differently is only unknown.

The model that explains things in the *Ask* tab can describe a result. It has
no way to change one: the status is computed from the comparisons, and nothing
a model says is an input to it.

## Validation

A separate question from matching: is the file intact and consistent with
itself? A valid file can still be for a different car.

| Answer | Meaning |
|---|---|
| `VALID` | The content matches a SHA-256 its source stated beforehand, is a kind of file its name can be, and nothing declared about it contradicts anything else. |
| `PARTIALLY_VALIDATED` | Nothing is wrong, and nothing vouches for it: no source stated what its hash should be. |
| `INVALID` | Missing, empty, not what its stated hash says, changed since it was kept, not what its name says, or declared to be two calibrations. |
| `UNKNOWN` | Not a kind of file treated as a calibration. |

A hash existing is not a file being verified. An invalid file is reported and
never copied to the cache.

## What it does not do

- No flashing, programming or reprogramming. Service `0x34`, `0x36`, `0x31`
  programming routines and programming sessions are not sent by anything in
  this feature.
- No writing of any kind: no `0x2E`, no reset, no session change.
- No security access, no key algorithm, no bypass of anything.
- No downloading of calibration files.
- No modifying, converting, patching or generating calibration files.
- No tuning, and nothing that touches an emissions control.
- No claim that a file is the manufacturer's own. The app can say a file
  matches a hash somebody stated. Who that somebody is, is yours to judge.

## Checking it on the real Odyssey

Not yet done with this feature. Reads only, ignition on:

1. Connect with the OBDLink MX+, identify, *Rescan*.
2. *Inspect* → select **ECM-EngineControl** → *Software on this module* →
   **Read software identity**.
3. Expect calibration `37805-5MR-C120` with verification number `16B6A354`,
   each with an *evidence* link that opens the reply it came from, and a list
   of thirteen identifiers asked for and not given.
4. Do the same for **TCM-TransmisCtrl**: `28102-5MX-A200`, `550C681C`.
5. **Find calibration file**. With nothing in the folder: *none found*.
6. Optional: put any file in the folder named `37805-5MR-C120.bin` and look
   again. Expect a **partial** match by name only, never exact.

Bring back anything that differs, and what the other eleven modules show.

## Limits

- One vehicle has been measured, and the identity read itself has only run
  against the simulator.
- On vehicles whose modules refuse the standard identifiers, only the engine
  and transmission can be identified.
- No manufacturer-specific identifier is read, for any make.
- `.rwd` and `.bin` content is not inspected, so everything known about such a
  file is what somebody declared.
- An exact match is only as good as the declaration it rests on.

## Where it lives

| | |
|---|---|
| `core/calibration` | The whole subsystem: identity, artifacts, formats, cache, sources, validation, matching. Cannot talk to a vehicle. |
| `core/diagnostics/src/service.rs` | `read_calibration_identity`: the vehicle read, behind the same safety gate as every other read. |
| `apps/api/src/calibration_routes.rs` | `GET /modules/{key}/calibration`, `GET /calibration`, `POST /calibration/find`. |
| `apps/desktop/src/components/CalibrationCard.tsx` | The card on *Inspect*. |
| `core/calibration/tests/fixtures.rs` | The seven fixtures: exact, wrong hardware, partial, corrupt, duplicate, none found, missing identifier. |
| `core/diagnostics/tests/calibration_identity.rs` | The Odyssey acceptance test, including that only reads are sent. |
