# Calibration identity and calibration files

What software a control module is running, and whether a file on your computer
is that software.

**This is read-only.** The app reads what a module says about its software. It
does not flash, program, patch, convert or tune, and there is no code in it
that could. See [What it does not do](#what-it-does-not-do).

**It never says who made a file.** Whether a file matches a module, whose word
that match rests on, and whether the manufacturer made the file are three
separate answers. The third is always *not established*. See
[Who made a file](#who-made-a-file).

## What it does

1. **Identify the module's software.** *Inspect* → pick a module →
   *Software on this module* → **Read software identity**. The app asks the
   module what it runs and shows each answer with where it came from.
2. **Find a calibration file.** **Find calibration file** looks through the
   calibration files on your computer and says, for each one, whether it is
   that module's calibration: exact, partial, conflicting, no, or cannot tell.
3. **Add a file you have.** **Add a file you have** copies a file into your
   calibration folder and looks again.

Steps 2 and 3 never touch the vehicle and never use the network.
**Finding nothing is the usual answer**, and the app reports it as an answer
(`NO_ARTIFACT_FOUND`), not as a failure. Calibration files are the
manufacturer's and mostly reach nobody outside a dealership.

The AI model in the *Ask* tab can do steps 1 and 2 with the `find_calibration`
tool. It runs the same search and gets the same answer. See
[The AI model](#the-ai-model).

## What the module is asked

Only requests that read, in whatever session the module is already in:

| Request | What it gives |
|---|---|
| OBD-II service 09, type 0A | The module's name |
| OBD-II service 09, type 04 | Calibration identification |
| OBD-II service 09, type 06 | Calibration verification number (the module's own checksum of its calibration) |
| UDS `22 F187`, `F188`, `F191` | Part number, software number, hardware number |
| UDS `22 F18A`, `F195`, `F197` | Supplier, software version, system name |
| UDS `22 F180`, `F181`, `F182` | Boot software, application software and application data identification |
| UDS `22 F189`, `F192`, `F193`, `F194` | Software version, hardware number and version, supplier's software number |

Asked in that order, so a module that stops answering has been asked for the
most useful ones first.

The service 09 requests are sent to the module alone. If that goes unanswered
they are sent to every module at once, which is the way the legislation obliges
a module to answer, and this module's reply is picked out by its address.

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

Nothing is filled in. If a reply cannot be parsed, the reply is kept and the
value stays unknown.

### What was not given, and why

Every identifier that was asked for and not given is kept too, with one of
these. They are different facts and are never merged:

| State | Meaning |
|---|---|
| `not_supported` | The module answered that it has no such identifier (`7F 22 31`, `11` or `12`). |
| `refused` | The module refused for another reason. It may have the identifier. |
| `no_answer` | Nothing came back. |
| `unreadable` | Something came back and could not be read. The reply is kept whole. |
| `empty` | The module answered with nothing in it. |
| `not_asked` | Not asked, with the reason (asking stops after a module goes silent). |

The result also carries `availability`: one word for **every** field about the
module, including the ones nothing asks for.

| Word | Meaning |
|---|---|
| `AVAILABLE` | The module reported it. |
| `NOT_SUPPORTED` | Every request that could read it was answered "not supported". |
| `REFUSED` | The module refused to give it. |
| `READ_FAILED` | Asked for, and the reading failed. |
| `NOT_READ` | Not read. No request in this app reads it, or not every request that could was made. Nothing is known either way. |

Program ID, strategy ID and ROM ID are always `NOT_READ`: no standard request
reads them and no manufacturer's own is known.

The calibration identification is also stored on the module's record and as a
finding against the vehicle, so it travels with a database export.

## What a 2023 Honda Odyssey reports

Measured on the real vehicle on 2026-10-04:

| Module | Calibration identification | Verification number |
|---|---|---|
| Engine, `18DAF110` | `37805-5MR-C120` | `16B6A354` |
| Transmission, `18DAF11E` | `28102-5MX-A200` | `550C681C` |

All thirteen modules refused every standard UDS identifier tried that day
(`7F 22 31`, request out of range). So on this vehicle the calibration
identification is known for the engine and transmission, and **no hardware
number, part number, software version, program ID or strategy ID is known for
any module**. None is made up to fill the gap.

### How far each part has been checked

| | Checked against |
|---|---|
| Calibration identification, verification number and name decode correctly | **The real vehicle's recorded bytes** (the replay of its first scan) |
| `F187`, `F188`, `F18A`, `F191`, `F195`, `F197` are read as "not supported" | **The real vehicle's recorded bytes** |
| `F180`, `F181`, `F182`, `F189`, `F192`, `F193`, `F194` | **Simulator only.** The real vehicle has never been asked for these. |
| Service 09 types 04 and 06 sent to one module alone | **Simulator only.** The real vehicle was only ever asked all modules at once. It does answer type 0A alone. |
| The whole *Read software identity* button, on the vehicle | **Not yet run.** |
| Finding a file for the real vehicle | Run on the recorded identity: none found, as expected. |

## Calibration files

### Where they are looked for

Three places, all on your computer:

| Source | What it is |
|---|---|
| **Your folder** | `calibrations\files` beside the session database. The *Find* result names the full path, and a `README.txt` there explains it. Files you add go here. |
| **A service tool's folder** | The folder Honda's J2534 Rewrite application installs its files in, when that application is on this computer. |
| **Kept files** | Files that matched before, kept by their SHA-256. |

**The app downloads nothing.** No source uses a network, because this project
knows of no public source it is entitled to fetch manufacturers' files from.
A file being reachable on the internet does not make it anyone's to pass on.
A source is an interface (`CalibrationSource`), so one can be added without
changing the matching.

Each source reports how its search ended:

| Status | Meaning |
|---|---|
| `matched` | Searched, and a file it offered matched. |
| `no_match` | Searched, and nothing in it matched. It may be empty. |
| `failed` | It was there and could not be searched. **Not** "nothing found". |
| `unavailable` | It has a location and that location is not there. |
| `not_configured` | Nobody has said where it is and it was not found. |
| `switched_off` | Switched off, and not searched. |

When a source `failed`, the result also says `incomplete: true`: a search that
could not look everywhere has not shown there is nothing to find.

### The service tool's folder

- It is looked for at `Honda\J2534 Pass Thru\CalibFiles` under the 32-bit
  Program Files folder. If it is not there, the source is `not_configured`.
- To point at it somewhere else, write `calibrations\sources.json`:

  ```json
  { "tool_folders": [
      { "id": "honda-j2534-rewrite", "path": "D:\\Honda\\CalibFiles", "enabled": true }
  ] }
  ```

- Only files whose names begin like the module's own identifier are opened
  (`37805-5MR-` for `37805-5MR-C120`). The rest are counted as passed over.
- Nothing in that folder is changed, moved or deleted.
- A file found there is a file found there. It is **not** thereby established
  as the manufacturer's: nothing checks a signature, and a file can be put in
  any folder.

No Honda service tool has been installed on any machine this was tested on.
The folder's location and the naming rule come from a public description.

### Kinds of file

`.rwd`, `.rwd.gz`, `.bin`, `.gz`, `.hex`, `.s19`.

A name can state two layers, and they are recorded as two facts:
`37805-5MR-C120.rwd.gz` is **gzip** holding **RWD**, not "a gzip file".

- gzip is unpacked, in memory only, to look at what is inside. It stops at
  64 MB, so a file built to exhaust memory is refused. Only one layer is opened.
- `.hex` and `.s19` are addressed memory as text, and the app checks the
  content really is that.
- `.bin` is bytes with no description. Nothing in it can be checked.
- `.rwd` is Honda's update package. Its **header** is read. See below.

Two hashes are kept: the SHA-256 of the file as it arrived, which is its
identity, and the SHA-256 of what is inside the packing.

No file is ever modified, converted or rewritten. A packed file is kept packed.

### What is read from an RWD package

Honda does not publish this format. The reader follows one public description
of it (the `rwd-xray` project), which calls itself a work in progress.

| | |
|---|---|
| Read | Which layout the file is, by its first byte. For the two layouts whose header is described (`Z` and `1`), the header's groups and the values in them that are plain text. |
| Not read | The software after the header. It is encoded, and the result says `PAYLOAD_OPAQUE`. |
| Not read out | The header group described as holding the encoding key. It is counted, never shown. |
| Not claimed | What a header value **means**. The description does not say whether a value is the calibration the file contains or one it replaces. |

That last line decides how a header is used. If the module's identifier is
written in a file's header, that is recorded as *in the file's own header* and
makes the match **partial**. It can never make it exact, and the result warns
that the file may be a different version for the same module.

**No real RWD file has been through this code.** It is tested against files
built to the described shape, with made-up identifiers.

### Saying what a file is

A file's name proves nothing: it is whatever the last person to rename it
typed. To let the app say more than "partial", put a metadata file beside the
calibration file in your folder, named `<the whole file name>.json`:

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
`core/calibration/src/identity.rs`; a key that is not one is ignored.

What you write there is recorded as **declared by you**. It is evidence of what
you say the file is. There is no key that makes the app say the manufacturer
made the file. A metadata file in a service tool's folder is not read at all:
you did not write it.

### The cache

A file that matches (exactly or partially) and is not invalid is copied to
`calibrations\kept\artifacts\<sha256>.<ext>`, with its record in
`calibrations\kept\metadata\<sha256>.json`. The SHA-256 is the file's identity.
The same file found twice, under any two names, is one file with one record
listing both places, each with what kind of place it was.

## Matching

Five answers and no percentage. Each is a rule applied to a list of
comparisons you can read on screen.

| Answer | Rule |
|---|---|
| `EXACT_MATCH` | The module's own calibration identification equals one **declared** for the file, and nothing disagrees. |
| `PARTIAL_MATCH` | Something agrees, and the calibration identification is not confirmed by a declaration. |
| `CONFLICTING_EVIDENCE` | What is said about the file disagrees with itself. Not judged either way. |
| `NO_MATCH` | A declaration about the file disagrees with the module, and nothing in the file says otherwise. |
| `UNKNOWN` | Nothing could be compared. |

Every result also says **what the match rests on**: `user_declared`,
`source_declared`, `file_header` or `filename`. An exact match always rests on
a declaration. Read the two together.

Each comparison is one of:

- **YES** (`confirmed`): the module reports it and it is declared for the file.
- **CONFLICT**: it is declared for the file and the module reports otherwise.
- **IN DOUBT** (`conflicting_evidence`): see below.
- **in the file** (`in_file_header`): the module's identifier is written in the
  file's own header.
- **name only**: the file's *name* agrees. It can make a match partial. It can
  never make one exact.
- **name differs**: the file's name suggests something else. Not a conflict.
- **UNKNOWN**: one side does not state it.

Each thing said of a file is shown with whose word it is.

### What outranks what

- A declaration outranks the file's name. The declaration decides, and the
  name's disagreement is still shown.
- Two declarations that disagree are **not** ranked against each other. A file
  declared to be two calibrations is `CONFLICTING_EVIDENCE`, even when one of
  the two is the module's.
- A header can agree with the module. It cannot be made to disagree, because
  nobody knows what it means by naming other identifiers.
- A declaration that says one thing, on a file whose own header names what the
  module reports, is `CONFLICTING_EVIDENCE`. Neither is simply believed.

An exact match says what it could not establish. On the Odyssey a file can
match the engine's calibration identification exactly while its hardware
number stays unknown, because the module never reported one.

Makes, models and engines are compared as words: `Honda (US)` from a VIN and
`Honda` in a file's metadata agree. A make or model sharing no word with the
other side's is a conflict. An engine worded differently is only unknown.

## Who made a file

Every result carries `origin.manufacturer`, and it is always
`NOT_ESTABLISHED`. There is no other value in the program for it to take.

A matching calibration identification, a matching verification number, a
matching hash, the folder a file was in, its name and anybody's declaration
are all evidence about **which calibration** a file is described as. None is
evidence about **who made it**. That would take a signature the app can check,
and it has none. Nothing in a metadata file can change this.

So a file can be an exact match and of unestablished origin at the same time.
That is a complete, correct result, and both halves are shown.

## Validation

A separate question from matching: is the file intact and consistent with
itself? A valid file can still be for a different car.

| Answer | Meaning |
|---|---|
| `VALID` | The content matches a SHA-256 its source stated beforehand, is a kind of file its name can be, and nothing declared about it contradicts anything else. |
| `PARTIALLY_VALIDATED` | Nothing is wrong, and nothing vouches for it: no source stated what its hash should be. |
| `INVALID` | Missing, empty, not what its stated hash says, changed since it was kept, named as gzip and does not unpack, not what its name says is inside, or declared to be two calibrations. |
| `UNKNOWN` | Not a kind of file treated as a calibration. |

A hash existing is not a file being verified. An invalid file is reported, is
marked beside its match on screen, and is never copied to the cache.

## The AI model

The model in the *Ask* tab has two read-only tools:

| Tool | What it does |
|---|---|
| `read_calibration_identity` | Reads a module's software identity. |
| `find_calibration` | Reads the identity, then runs the search described here. |

- Both take a module and nothing else. The model cannot supply a path, a web
  address, a file, or an identity of its own to match against.
- `find_calibration` goes through the same code as the *Find calibration file*
  button (`aim_calibration::Library`). A test asks both and compares the answers.
- The result is structured: every source and its status, every file with its
  match, what it rests on, its validation, and `NOT_ESTABLISHED`.
- The model can describe a result. It has no way to change one: the status is
  computed from the comparisons, and nothing a model says is an input to it.
- There is no tool that downloads a file, and none that writes to a module.

## What it does not do

- No flashing, programming or reprogramming. Service `0x34`, `0x36`, `0x31`
  programming routines and programming sessions are not sent by anything in
  this feature.
- No writing of any kind: no `0x2E`, no reset, no session change.
- No security access, no key algorithm, no bypass of anything.
- No downloading of calibration files, and no searching of any network.
- No opening, decoding or decrypting of the software inside a file.
- No modifying, converting, patching or generating calibration files.
- No tuning, and nothing that touches an emissions control.
- No claim that a file is the manufacturer's own.

## Checking it on the real Odyssey

Not yet done with this feature. Reads only, ignition on:

1. Connect with the OBDLink MX+, identify, *Rescan*.
2. *Inspect* → select **ECM-EngineControl** → *Software on this module* →
   **Read software identity**.
3. Expect calibration `37805-5MR-C120` with verification number `16B6A354`,
   each with an *evidence* link that opens the reply it came from.
4. Under *Not known for this module*, note what each line says. The simulator
   says every standard identifier is "not supported". Seven of them have never
   been asked of the real vehicle, so this is new information.
5. Do the same for **TCM-TransmisCtrl**: `28102-5MX-A200`, `550C681C`.
6. **Find calibration file**. With nothing in the folder: *none found*, and
   Honda's folder *not installed, so not searched*.
7. Optional: **Add a file you have** with any file renamed
   `37805-5MR-C120.bin`. Expect a **partial** match by name only, never exact.

Bring back anything that differs, and what the other eleven modules show.

## Limits

- One vehicle has been measured, and the *Read software identity* button has
  not been pressed on it.
- On vehicles whose modules refuse the standard identifiers, only the engine
  and transmission can be identified.
- No manufacturer-specific identifier is read, for any make.
- The RWD reader has never seen a real file. If Honda's files differ from the
  public description, a real file would be reported as invalid or its header
  as unread. It would not be reported as a match it is not.
- An identifier in an RWD header cannot make a match exact, because what the
  header means is not known.
- `.bin` content is not inspected, so everything known about such a file is
  what somebody declared.
- An exact match is only as good as the declaration it rests on, and says so.
- A service tool's folder can only be set by editing `sources.json`. There is
  no screen for it.
- Adding a file from the screen cannot declare what it is. That still takes a
  metadata file beside it.

## Where it lives

| | |
|---|---|
| `core/calibration` | The whole subsystem. Cannot talk to a vehicle. |
| `core/calibration/src/library.rs` | `Library`: the one search every caller goes through, the sources, and adding a file. |
| `core/calibration/src/resolve.rs` | The matching rules, source statuses and `ManufacturerOrigin`. |
| `core/calibration/src/format.rs` | Layered formats, the bounded gzip unpack, `inspect`. |
| `core/calibration/src/rwd.rs` | The RWD header reader, and what it rests on. |
| `core/diagnostics/src/service.rs` | `read_calibration_identity` and `find_calibration`, behind the same safety gate as every other read. |
| `core/tools/src/lib.rs` | The `find_calibration` tool a model is handed. |
| `apps/api/src/calibration_routes.rs` | `GET /modules/{key}/calibration`, `GET /calibration`, `POST /calibration/find`, `POST /calibration/files`. |
| `apps/desktop/src/components/CalibrationCard.tsx` | The card on *Inspect*. |
| `core/calibration/tests/fixtures.rs` | The fixtures, A to O. |
| `core/diagnostics/tests/calibration_identity.rs` | The acceptance tests, including the replay of the real vehicle and that only reads are sent. |
