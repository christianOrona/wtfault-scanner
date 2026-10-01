# At the car

Everything still open that only a person with a vehicle (or a key) can finish,
as steps to follow. Each one says what it closes and what to bring back.

Before any of it: install the latest release (or run from source), and close
the desktop app before running a script; it holds the adapter and the
database.

After every session, whatever it was for, export it as a replay so CI can
hold the core to what that vehicle showed (#59). The VIN is anonymised and the
export refuses to write if any trace of it is left:

```powershell
scripts\export-replay.ps1                                    # list sessions
scripts\export-replay.ps1 -Session <id> -Name <year-make-what>
```

Commit the `.transcript` and `.baseline.json` it writes under
`core\diagnostics\tests\replays\`.

## 1. Baseline the F-250: cold start against everything learned (#54)

Nothing is deleted. A cold start is the core on an empty database of its own.

```powershell
# Cold: a brand-new install, on the truck.
scripts\dev-core.ps1 -Serial COM4 -Db $env:TEMP\cold.sqlite
#   in the app or over the API: connect, identify, scan, full scan
scripts\scorecard.ps1 -Vin <VIN> -Api http://127.0.0.1:8788/api/v1    # "cold"
#   stop the core (Ctrl+C)

# Learned: the app's own database. No vehicle needed for this half.
scripts\dev-core.ps1
scripts\scorecard.ps1 -Vin <VIN> -Api http://127.0.0.1:8788/api/v1    # "learned"

scripts\scorecard.ps1 -Diff scorecards\<cold>.json, scorecards\<learned>.json
```

Bring back the diff, for `STATUS.md` under findings from real sessions.

## 2. Check what was fixed at a desk, on the F-250

These were tested against the simulator and recorded sessions only.

- **Freeze frame with nothing stored.** On the engine module with no
  emissions fault: *Codes* → *Read freeze frame* should say no snapshot is
  stored, not fail.
- **A long fault list that stops part-way.** The 2012 F-250 cut one off at
  407 bytes. A full scan there should show the codes that arrived and say the
  list is incomplete.
- **Clearing a body module's codes** goes over UDS on its own bus. Only if
  there is a code you actually want gone: clearing cannot be undone. The
  result should name the module as cleared, or say why not.

## 3. A second manufacturer, end to end (#39)

The Mazda 3, or the 2023 Odyssey. What this tests is whether every empty
result comes with an honest reason. A screen that shows nothing and says
nothing is the bug to report, even if nothing crashed.

1. Connect, identify. The make and year should settle from the VIN, shown in
   the sidebar under *Vehicle*.
2. *Rescan*, then *Full scan*. Note how many modules each finds and on which
   bus.
3. *Inspect* the engine module; *Live data* for a minute; *Guided tests* →
   *Warm idle* (petrol only).
4. In the sidebar under *Vehicle*, look up the VIN (vPIC) and the signal set
   (OBDb). Then *Settings on the car*: expect no settings listed, with a note
   saying the catalogue has none for this make.
5. *Inspect* → *What this module answers* → *Probe*, on each module. It reads
   only. A range it did not ask should say why.
6. Export the session (above), named e.g. `2023-honda-odyssey`.

Bring back the transcript, and a note of anything that was empty without
saying why.

## 4. The phone as the scanner (#64)

1. Install the `.apk` from the latest release.
2. Pair the ELM327 in Android's Bluetooth settings.
3. Open the app, pick the adapter, connect, full scan.

Bring back whether the full scan finished, and anything that differed from
the laptop.

## No car needed, but needs you

- **Sessions already recorded on another make (#59).** If the app's database
  holds a session from the Odyssey (or any non-Ford), it can become a replay
  without another visit: `scripts\export-replay.ps1` lists every session, and
  exporting one is the same as above. Each one makes CI fail when a change
  makes that vehicle discover less.
- **OpenRouter (#65).** *Settings* → add OpenRouter with a free key, leave
  the model on *Automatic*, press *Test*. Then run an inspection, against the
  simulator if you like (`scripts\dev-core.ps1`). Done when it reaches a
  report, on desktop and on Android.
- **SignPath (#62)** is waiting on their approval.
- **The report endpoint (#50)** is built (`apps/report-endpoint`, see
  `docs/REPORTS.md`) and needs somewhere to run. A release then needs
  `AIM_REPORT_ENDPOINT` set to its address.
