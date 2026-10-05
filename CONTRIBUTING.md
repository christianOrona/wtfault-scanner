# Contributing

WTFault Scanner is a one-person project built on one truck. That is its biggest
weakness, and it is the thing other people can fix: almost everything it has
measured on a real vehicle came from Ford trucks. Help is welcome, and the most
useful help does not need any Rust.

## The most useful things you can bring

### A recording from a vehicle that is not a Ford

Every assumption that only holds on the vehicle it was built on is invisible
until another make is plugged in. A recorded session shows them, and CI then
replays it on every change and fails if a later version discovers less.

1. Connect, identify, *Rescan*, *Full scan*. Notice anything that came back
   empty without saying why: **a silent empty result is a bug, even when
   nothing crashed**, and worth an issue by itself.
2. *Sessions* → open the session → *Export transcript*. The VIN is replaced
   before the file is written, and the export refuses to write if any trace of
   it is left.
3. Read the file before you share it. It is every byte the vehicle sent.
4. Open an issue with the make, model and year, and attach it. Or add it under
   `core/diagnostics/tests/replays/` yourself with
   `scripts\export-replay.ps1`; [docs/AT-THE-CAR.md](docs/AT-THE-CAR.md) has
   the steps.

Issues labelled
[`needs-vehicle`](https://github.com/christianOrona/wtfault-scanner/labels/needs-vehicle)
are waiting on exactly this.

### A vehicle profile

Everything the app knows about vehicles is data: PID scalings, code
descriptions, module names, configurable settings. A profile is a YAML file,
and the README's *Vehicle profiles* section shows the shape.

- Say where each thing came from: the standard, a document, or a measurement
  on a vehicle you can name by make, model and year.
- Mark anything you have not checked as `unverified`. Nobody is promoted to
  `verified` by saying so; see
  [vehicle-profiles/generic-obd/README.md](vehicle-profiles/generic-obd/README.md).
- A setting that can be **written** needs evidence that it was written on a
  real vehicle, read back, and seen to change what the vehicle does. Until
  then the app will read it and refuse to write it, on purpose.

### A problem report

The problem report in *Settings* gathers the version, the operating system and
the end of the log, with VINs and your user name taken out. Paste it into an
issue. If the app said something about your vehicle that was wrong, say what
the vehicle actually does: that is the kind of report that changes the code.

## Working on the code

### Setting up

Windows is where the desktop app is built and run. `scripts\setup-windows.ps1`
installs the toolchain; the Rust version is pinned by `rust-toolchain.toml`.
The core also builds and tests on Linux, which CI proves on every push.

You do not need a car or an adapter. `scripts\dev-core.ps1` runs the core
against a virtual vehicle, and the README's *Try it without a car* lists the
three that are built in. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) is the
map of the crates, [docs/API.md](docs/API.md) the contract between the core and
the interface, and [docs/ANDROID.md](docs/ANDROID.md) covers the phone build.

### Before you open a pull request

```powershell
powershell -NoProfile -File scripts/check.ps1
```

All of it has to pass: formatting, tests, clippy with warnings as errors, the
interface's typecheck and build, the build without serial support, and the
desktop shell. Run this script, not `cargo test --workspace` alone: the desktop
shell is a separate Cargo workspace and a plain cargo run never compiles it.

### What a change needs

- **A test that fails without it.** For a fix, the test is the bug. Tests here
  are named for the behaviour they hold, such as
  `a_snapshot_refuses_to_overwrite_a_file`.
- **No claim without its source.** Every value the app shows links to the
  adapter exchange it came from, and everything else is labelled as catalogue
  or as a model's general knowledge. A decoder with no checked source ships as
  `unverified` or not at all. When two sources disagree, say so in the file
  and decode neither.
- **An honest empty result.** "Nothing was found", "nothing was asked" and
  "the vehicle did not answer" are three different results, and a change
  should not merge them.
- **Its documentation.** A changed route goes in `docs/API.md`. Anything a
  person using the app would notice goes under `## [Unreleased]` in
  `CHANGELOG.md`, written for that person. `STATUS.md` changes only for
  something significant, and its *Verified on real hardware* section only for
  something that ran on a vehicle. Simulator results are not evidence there.
- **A commit message that says why.** `feat:`, `fix:` or `docs:`, one line of
  what, then the reasoning and anything deliberately left out.

If you change anything that can write to a vehicle, read
[docs/SAFETY.md](docs/SAFETY.md) first. Its last section lists five invariants,
each held by a test. Change one of those tests deliberately and say why, never
by adjusting an assertion until it passes.

### Other people's work

- **Code from a GPL or other copyleft project cannot be used here.** What a
  vehicle or an adapter does is a fact and can be learned from anywhere; the
  code that handles it has to be written independently. The README's
  *Standing on other people's work* shows how that has been done so far.
- **Data needs a licence that allows redistribution**, and its attribution
  shipped with it, as OBDb's is. A dataset with no licence is all rights
  reserved, however public it is.

## What will not be merged

These are decisions, not gaps, and no evidence changes them:

- Anything in the braking, steering or throttle path.
- Immobilisers and keys. Writing firmware in any form: programming, flashing,
  patching or converting a calibration. Reading which software a module runs
  is in scope and stays read-only; see
  [docs/CALIBRATION.md](docs/CALIBRATION.md).
- Fetching manufacturers' calibration files from anywhere the project is not
  entitled to fetch them from.
- Anything that defeats an emissions control.
- Anything that gets past a vehicle's own security: key algorithms, or a way
  around a security gateway.
- Anything that sends a VIN, or fetches from the internet, without a person
  asking for it each time. What the app sends, and when, is listed in
  [docs/CODE_SIGNING.md](docs/CODE_SIGNING.md) and has to stay true.
- Invented numbers: a repair cost as a figure, a guessed model, a description
  for a code nobody has a source for.

## How a change gets in

Open a pull request against `main`. For anything large, open an issue first so
the approach can be agreed before the work is done.

The maintainer reviews and merges every change; nobody else can. This is one
person with a day job, so a reply can take a while, and a pull request that
arrives with its tests and its reasoning is reviewed soonest.

Releases are built by CI from a tag and published by hand;
[docs/RELEASING.md](docs/RELEASING.md) has the procedure.

## Licence

The project is dual-licensed under MIT and Apache-2.0. Unless you state
otherwise, anything you submit for inclusion is licensed the same way, with no
additional terms.

## Security

For anything sensitive, do not open a public issue. See *Reporting something*
in [docs/SECURITY.md](docs/SECURITY.md).
