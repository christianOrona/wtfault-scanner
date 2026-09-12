# Changelog

Notable changes, newest first. Versions follow [semantic versioning](https://semver.org),
with the caveat that everything below 1.0 is allowed to move.

## [0.4.1] — 2026-09-11

Finding the second bus turned a seven-module vehicle into a thirty-six module
one, and every screen still assumed all of them were emissions modules. This is
the catch-up.

### Fixed

- **The assistant told an owner his app could not do something it had already
  done.** Asked whether his mirrors could be set to fold, it answered "not with
  this app, and not with that adapter" — on a truck this app had written a
  configuration change to, through an adapter that had just read the bus in
  question.

  Its prompt said "This build is permanently read-only". That is not true: the
  application writes configuration and clears codes behind a typed confirmation.
  It is the *agent* that cannot, and stating that as a property of the
  application sends people off to buy software they already own. The prompt now
  draws the boundary where it actually is, and says who can.

  The same prompt described every adapter as a cheap ELM327 clone. It now
  receives the measured capabilities of the adapter actually connected.

- **Live data offered modules that can never produce it.** 29 of the 36 modules
  on a 2019 F-250 are body and comfort modules: they implement UDS and owe
  OBD-II service 01 nothing. Asking one for its supported PIDs returns nothing,
  and the picker rendered empty — which reads as a broken screen rather than as
  a question that was never going to have an answer.

  It now says what is true: this module works, it does not report live sensor
  data, and here is what it does hold.

- **The module list was thirty-six rows with nothing to separate them.** Split
  by bus, with the body group labelled and explained.

- **`scan_all_modules` appeared as a raw identifier** in the middle of plain
  English while an inspection ran. Eight tool names were missing from the
  dictionary that turns them into sentences, and every one of them would have
  done the same thing when called.

- **The report ended with hundreds of lines of the assistant's working.** Every
  call, its arguments as raw JSON, every result — pinned below the answer,
  growing with the number of modules. Collapsed behind a disclosure. Nothing is
  deleted: it is one click away, and the flight recorder and the log file both
  hold the same record in full.

### Known

An inspection interrupted by a provider running out of credit, then restarted,
showed both runs' steps concatenated in one progress panel. Filed as #52 with
the evidence and three candidate causes, none of them confirmed. Not fixed:
one screenshot is not a diagnosis.

**Upgrading from 0.3.2 or earlier still needs a manual install.**

## [0.4.0] — 2026-09-11

Measured on a 2019 F-250, where the app had been seeing two modules and calling
that the vehicle. Mapping its second bus by hand found **29 more**, 22 of them
holding configuration blocks. Everything below is what that measurement exposed.

### Changed

- **A bus no longer claims to know its own speed.** `VehicleBus` named two:
  `HighSpeed` at 500 kbit/s and `MediumSpeed` at 125. That truck's second bus
  runs at **500**. At 125 and 250 the adapter reported nothing but `CAN ERROR`;
  at 500 the same pins carried 239 frames in five seconds. Ford called the old
  one MS-CAN at 125 and the newer one HS-CAN2 at 500, so a type encoding either
  number is wrong on half the fleet.

  The type now names the pins — `Primary` for 6 and 14, `Secondary` for 3 and
  11 — and the rate is searched for, fastest first, by listening for traffic.

- **Discovery asks each bus a question it might answer.** The legislated
  broadcast is right on the primary bus and useless on the secondary one: with
  that bus demonstrably alive, `7DF 0100`, `3E00` and `0902` every one returned
  NO DATA. Body modules are not emissions modules and owe service 01 nothing.

  The secondary bus is swept by address with TesterPresent instead, and a
  negative response counts as presence — three of those 29 modules answered
  `7F 3E 12`, and treating a refusal as silence would have lost them.

- **The sweep reaches `7FF`.** It stopped at `7EF`, the tidy end of the
  conventional block. A module answered at `7F1`.

- Module keys carry the bus they answered on, so two buses with a module at the
  same address stay two modules.

### Fixed

- **`multiple_can_buses` was never set true by anything, anywhere.** Every
  bus-switching path was unreachable on every adapter ever connected, including
  the ones that could plainly do it. A capability nothing grants is a feature
  nobody has.

- **Bus switching used a command never confirmed to reach the second channel.**
  `ATPB` reconfigures a protocol; on STN hardware the second channel is a
  different thing. `STP53` selects it and `STPBR` sets its rate, both measured
  working.

- **A procedure that measured nothing reported success.** `warm_idle` exists to
  read fuel trims at operating temperature. It named `short_term_fuel_trim_1`
  and `long_term_fuel_trim_1`; the catalogue calls them `short_fuel_trim_b1` and
  `long_fuel_trim_b1`. Neither resolved, so it ran on a warm engine, measured no
  fuel trims, and said it had succeeded.

  Correct ids are not enough — the truck is a diesel and has no fuel trims to
  report. The result now carries what was declared, what was unavailable, and
  whether it was complete, and says plainly that a missing signal can be a fact
  about the engine rather than a fault. Two tests now compare every procedure's
  declared signals against the signals that exist.

### Known

**None of the second-bus code has been run against a vehicle.** The hand
measurement says what is there; it does not say this finds it.

Silence on the secondary bus is still reported as silence. An adapter that
accepts every bit-rate command is not necessarily wired to pins 3 and 11, and
nothing on the wire tells the two apart.

**Upgrading from 0.3.2 or earlier still needs a manual install.**

## [0.3.9] — 2026-09-11

### Added

- **A module scan sweeps the second bus.** Body, comfort and instrument modules
  — the ones owning nearly everything configurable — commonly sit on a slower
  body bus, and a tool that only asks the fast one sees the powertrain and calls
  it the vehicle. When the adapter accepts the bit-rate commands, the scan now
  switches, sweeps, and switches back.

  Every bus is reported separately, including ones where nothing answered. That
  distinction is the point: an adapter can accept every command and still not be
  wired to the pins the second bus uses, and nothing on the wire tells the two
  apart. So silence is recorded as silence, never as "this vehicle has no second
  bus".

  A module key now carries the bus it answered on. Two buses can each have a
  module at `7E8`; a key built from the address alone would file the second on
  top of the first, and the scan would report fewer modules the more buses it
  swept.

  Nothing here has met a real body module yet.

### Changed

- **Updates download in the background and install without windows.** The
  download starts as soon as a release is found, with progress, and the button
  only offers to install once the bytes are on disk — the wait happens before
  the decision instead of after it. The installer then runs silently: no wizard,
  no uninstaller dialog, and the app is relaunched when it finishes.

  This uses flags the generated installer already had. In silent mode it also
  closes the running application itself rather than asking, which is what makes
  it safe to start from inside the app being replaced.

  The crash marker is cleared before the installer starts rather than on the way
  out, because the installer does not wait for this process to finish tidying up.

## [0.3.8] — 2026-09-11

### Added

- **The app keeps a log.** Daily files under the application's own data folder,
  seven kept. Until now logging went to stdout, and a release build is a Windows
  GUI binary with no stdout — so it went nowhere. An installed build ran for
  eleven minutes, froze, and left a hash in the Windows event log and nothing
  else. The file writer is unbuffered on purpose: buffering loses whatever had
  not been flushed when the process died, which is the part worth reading.

- **A run that ends badly is noticed.** A marker file is written at startup and
  removed on a clean exit. A process that freezes, is killed, or loses power
  never removes its own, so finding one at the next launch is proof the last run
  did not end properly — the one failure that cannot report itself while it is
  happening. The updater clears the marker before replacing the app, because
  leaving on purpose is not crashing.

- **"Reporting a problem" in Settings, and a banner after a bad run.** One piece
  of text with the version, the machine, how the last run ended, and the end of
  the log, over three buttons: copy it, save it to a file, open the log folder.
  No account, no form, nothing to sign up for.

  Nothing is sent anywhere. A log carries VINs, fault codes and file paths with
  somebody's own name in them, so it moves only when a person moves it.
  [#50](https://github.com/christianOrona/wtfault-scanner/issues/50) covers
  sending one to an endpoint the project hosts, with its own consent.

- Panics are written to the log before they reach a stderr nobody can see.

## [0.3.7] — 2026-09-11

### Changed

- **The splash spinner turns the whole time the card is up.** It used to stop
  and fill once the boot steps finished, which on a fast machine is nearly
  immediately — so the ring was a static dot for almost the entire time anyone
  was looking at it. The card has a floor on how long it stays; the indicator
  now matches it.

- The corner no longer reads "Ready". It says what is happening, or
  "Finishing up…" once the steps are through.

The card's timing is unchanged and is worth stating plainly: it stays up for at
least eight seconds, and longer if the core is still starting. Brand flourish.

## [0.3.6] — 2026-09-11

### Changed

- **The splash shows the artwork it was always meant to show.** The banner —
  the truck, the logo, the tagline — instead of the square app-icon art. The
  file being imported was named `splash.png` and had been the icon all along,
  so every request to "use my image" was already satisfied by the code and
  still wrong on screen. Re-encoded at 1400px: 1.6 MB of PNG became 238 KB.

- **Something moves the whole time the app is starting.** A ring beside the
  step name, turning continuously. The progress bar underneath only advances
  when a step completes, which on a slow core is several seconds of a screen
  with nothing happening on it — indistinguishable from a screen that has
  hung. The ring stops and fills once everything is done; a spinner next to
  the word "Ready" claims work that is over.

- **"What changed?" is no longer a wall of monospace.** It rendered the raw
  markdown in a `<pre>`: asterisks, headings, and the hard line breaks at
  column 78, which made every release look like log output. The notes are the
  same published words, now read for their shape — the claim of each entry,
  its explanation underneath in smaller type and capped at two lines, over a
  link to the full text. Notes written for one medium do not survive being
  pasted into another.

### Noted

- Updating without closing the app is [#48](https://github.com/christianOrona/wtfault-scanner/issues/48).
  A running Windows process holds its own executable open, so it cannot be
  replaced in place — that needs staged folders and a helper that swaps them
  after exit. Doing that quietly with unsigned installers is also precisely
  what a compromised app would do, so signing comes first.

## [0.3.5] — 2026-09-11

### Fixed

- **The app would not get out of its own installer's way.** Pressing "Update
  now" downloaded the installer, checked its size, and started it — and then
  kept running. The installer's first act is to remove the version already on
  the machine, which it cannot do while the app is holding those files open.

  It failed badly rather than harmlessly. The uninstall entry was removed
  before the file deletion was attempted, so the machine was left with an older
  build installed and nothing in Add/Remove Programs pointing at it. Measured
  on a real machine: a running 0.3.3 pressed the button and came back as
  **0.3.1**, unregistered. A failed update that downgrades you is worse than
  one that changes nothing.

  The core now quits itself 750 ms after starting the installer — long enough
  for the response to reach the window, short enough that nobody can click
  through the installer's first page before the files are free.

- The button no longer lies while this happens. Its success branch was empty,
  on the theory that the window was about to disappear on its own; it did not,
  so a spinner reading "Downloading" sat on top of a failed install. It now
  says it is closing for the installer.

This is the bug that 0.3.4 existed to find. 0.3.4 shipped with nothing in it so
that exactly one thing could be blamed if the install failed, and it was.

## [0.3.4] — 2026-09-11

Nothing in this release changes how the scanner talks to a vehicle. It exists
so that the last untested step of the updater — the one that actually replaces
the running application — gets run once with someone watching.

0.3.3 proved the parts either side of it: the check finds a newer tag, the
banner appears, the button is wired, and the download address is now accepted.
What has never happened in one continuous go is: download the installer,
verify its size, launch it, and come back as a newer version. Every earlier
attempt stopped before that point, for a different reason each time.

So the payload is deliberately empty. If this install lands, the only thing
that changed is the number, and the update path is proven. If it doesn't, the
failure belongs to the updater and nothing else — which is the entire point of
shipping it with nothing else in it.

## [0.3.3] — 2026-09-11

### Fixed

- **The updater refused to download its own installer.** The allowlist of hosts
  a release asset may come from held `api.github.com` and
  `objects.githubusercontent.com` — the CDN host a download *ends up* on after
  the redirect. Every asset GitHub publishes starts from
  `github.com/{owner}/{repo}/releases/download/...`, which was not on the list,
  so no update could ever install.

  Found by pressing the button. The 0.3.1 banner correctly offered 0.3.2, and
  "Update now" answered *"the installer is hosted at an unexpected address"*.

  The tests were green because every one of them used a CDN URL. They were
  careful about rejecting `example.com`, plain `http://`, and
  `githubusercontent.com.evil.com`, and never once tried the address GitHub
  actually produces. A test suite can be thorough about the wrong thing.

  So 0.3.1 was half a fix: it made the interface ask, which was the bug it set
  out to solve, and the install behind the button had never worked either.
  Nobody noticed because nobody had pressed it.

- `github.com` is allowed but not as a bare host. Anyone can publish a release
  there, so a host check alone would accept an installer from any repository on
  the site — a `github.com` URL must be under this project's own releases path.

- The version this binary reports and the version its installer carries are now
  pinned equal by a test. If they drift the updater loops forever: it compares
  the published tag against the running version, so an installer stamped 0.3.3
  that installs a binary reporting 0.3.2 offers the same update, installs it,
  and is still out of date — with no error anywhere, because every step worked.

**This cannot fix itself in the field.** 0.3.1 and 0.3.2 both carry the broken
check, so either needs one manual install of 0.3.3 before updates work.


## [0.3.2] — 2026-09-11

The release where writes actually reached a vehicle.

### The headline

- **First configuration write to a real vehicle.** AutoLock on a 2019 F-250,
  `DE0E` byte 4 `01` -> `00`, read back and confirmed by the owner against the
  dash menu. It took four fixes to get there, and none of them would have
  worked alone.

- **The part worth remembering.** Immediately after the write the dash still
  showed the old value. Every machine-checkable signal said it had worked — the
  module accepted it, the read-back returned the new bytes, nothing reverted —
  and the vehicle behaved as though nothing had changed. It took an ignition
  cycle for the module to latch it. One key cycle away from being filed as "the
  mapping is wrong". Now recorded in the profile, pinned by a test, and warned
  about on every successful write.

### Adapters

- **STN firmware is recognised by asking, not by matching a name.** `STI` was
  gated on the banner or vendor containing "STN", "OBDLINK" or "SCANTOOL". An
  OBDLink MX+ answers `ELM327 v1.4b` and `OBD SOLUTIONS LLC` — so the flagship
  STN device failed all four tests and was told, in its own capability caveats,
  to go and buy an OBDLink MX+.
- **Long requests go through `STPX`.** The old probe tested one transmit form of
  two; the ELM request form caps at seven data bytes on *every* device, STN
  included. An adapter that segments perfectly well was recorded as unable to
  write, which is the single capability gating configuration changes.
- **Adapter fitness as data**, including whether an absence of findings means
  anything. A clean scan through a failing link is the link's silence wearing
  the vehicle's clothes, and the agent is now told so.
- An interrupted command is re-sent rather than triggering a full warm start.
  Answering #11 from the session database: all 611 `stopped` responses were in
  one session, all on `ATSH` during an address sweep, all inside 50 ms.

### Knowing what vehicle this is

- **One `VehicleIdentity`** assembled from evidence that was previously
  collected and discarded. Holds candidates rather than answers, so two sources
  disagreeing stay visible instead of one silently winning.
- **One interface for every source of vehicle knowledge**, ordered by authority
  and never merged. Licence travels with the answer.
- **As-built import**, VIN-checked. A file for another vehicle is refused with
  both VINs named.
- **Measured mappings are offered to similar vehicles** as candidates to check
  by prediction, and refused a write until confirmed here.

### Safety and privacy

- **API keys moved to the OS credential store.** Migrated from the plaintext
  file and then removed from it — a migration that left a copy behind would have
  improved nothing while looking like it had.
- **The VIN can be withheld from models outside your control.** The distinction
  is whose machine, not local versus hosted.
- **Imported profiles arrive unverified**, whatever the file claims about
  itself.
- **Guided procedures** that put the vehicle in a state and measure it there,
  with anything needing road speed described and refused rather than walked
  through.

### Interface

- A splash that closes itself, reports the boot step actually running, and
  carries the project links.
- "Where is it?" on a trouble code: a zone on a generic silhouette, captioned as
  what it is.
- The assistant can ask you questions with buttons rather than burying them in a
  paragraph.

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
