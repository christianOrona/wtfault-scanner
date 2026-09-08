# AI Mechanic

An OBD-II scanner you can actually understand. It reads your vehicle, explains
what it found in plain English, and shows the evidence behind every claim.

**Status:** Phase 0 + Phase 1 complete — the diagnostic core, the ELM327
adapter, a virtual vehicle to develop against, a localhost API, and the desktop
app. No AI is wired up yet; the typed tool registry and safety gate it will call
through are built and tested.

- `docs/HANDOFF.md` — the spec this is built against
- `docs/API.md` — the `/api/v1` contract (the UI is built against this)
- `docs/ARCHITECTURE.md` — crate map and design decisions
- `apps/desktop/README.md` — the desktop shell
- `WORKPROGRESS.md` — what is done, what is next

---

## The desktop app

```bash
cd apps/desktop
npm install
npm run tauri:dev      # the app, with hot reload
npm run tauri:build    # installers in src-tauri/target/release/bundle
```

The window hosts the diagnostic core itself — there is no separate server to
start and none to leave running. It persists sessions to
`%APPDATA%/ai-mechanic` by default.

Prefer a browser while working on the UI? Start the API yourself and run
`npm run dev` (see `apps/desktop/README.md`).

---

## Try it in one command

No adapter, no vehicle, no hardware of any kind:

```bash
cargo run -p aim-api -- --simulator --scenario dpf-regen
```

That boots a virtual 2019 F-250 with a 6.7 L diesel mid-particulate-filter
regeneration, behind an emulated cheap ELM327 clone. The API is then at
`http://127.0.0.1:8787`.

```bash
BASE=http://127.0.0.1:8787/api/v1

curl -sX POST $BASE/adapter/connect      # connect and start a session
curl -sX POST $BASE/vehicles/identify    # read the VIN
curl -sX POST $BASE/modules              # scan for modules
curl -s     $BASE/modules/ECU_7E8/dtcs   # read the trouble codes
curl -sX POST $BASE/modules/ECU_7E8/read \
     -H 'content-type: application/json' \
     -d '{"signals":["engine_rpm","coolant_temp"]}'
```

Scenarios: `healthy`, `dpf-regen`, `bus-silent` (an adapter that works
connected to a vehicle that is not answering — worth seeing, because the UI has
to handle it).

```bash
cargo run -p aim-api -- --list-scenarios
```

## Against a real adapter

```bash
cargo run -p aim-api -- --serial COM5        # Windows (paired Bluetooth ELM327)
cargo run -p aim-api -- --serial /dev/rfcomm0 # Linux
```

Not sure which port? Ask, and let it probe:

```bash
curl -s "http://127.0.0.1:8787/api/v1/adapters/ports?probe=true"
```

That sends `ATZ`/`ATI` to each port and reports which one answers like an
ELM327 — and reports the reason for the ones that will not open, which is
usually the more useful answer.

## Useful flags

```
--simulator                 use the virtual vehicle (the default)
--serial <PORT>             use a real adapter on this port
--scenario <NAME>           healthy | dpf-regen | bus-silent
--personality <NAME>        clone | genuine  (which ELM327 to emulate)
--db <PATH>                 persist sessions; omit for an in-memory run
--port <N>                  listen on a different port (default 8787)
--list-scenarios
```

Session history is only kept with `--db`:

```bash
cargo run -p aim-api -- --simulator --db ./sessions.sqlite
```

---

## Setting up

### Windows (the garage laptop)

A fresh laptop has none of the prerequisites. The script checks first and
changes nothing until you let it:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1 -CheckOnly
powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1
```

It handles, in dependency order: **Visual Studio C++ Build Tools** (Rust needs
the MSVC linker — miss this and `cargo build` fails at link time with an error
that looks like a Rust problem and is not), **Rust**, the **WebView2 runtime**,
and **Node.js**. It also tells you whether your ELM327 has produced a COM port
yet.

Pairing the adapter, for reference: Settings → Bluetooth & devices → Add
device → the ELM327 (PIN is usually `1234` or `0000`), then Bluetooth settings
→ More Bluetooth options → COM Ports, and use the **Outgoing** port.

### Linux / macOS

Rust 1.82 or newer. On Linux, real serial support needs libudev:

```bash
sudo apt-get install -y libudev-dev pkg-config
```

Or skip it — everything except real hardware works without:

```bash
cargo test --workspace --no-default-features
```

---

## Building and testing

```bash
cargo build --workspace
cargo test  --workspace     # 345 tests, no hardware required
cargo clippy --workspace
```

Every layer is verified against the simulator, which speaks the real ELM327
wire protocol rather than stubbing the adapter out. A parsing bug in the
adapter is a bug the test suite catches.

---

## What it does today

- Connects over Bluetooth/serial to an ELM327-class adapter, or to the
  simulator
- Reports what the adapter **demonstrated** it can do, with explicit caveats
  for everything a cheap clone only claims
- Reads the VIN and calibration identifiers
- Discovers responding modules and names them from what they report
- Enumerates supported live-data parameters, and says which ones this build can
  decode
- Reads stored, pending and permanent trouble codes with generic SAE
  descriptions
- Reads freeze frames
- Streams live data over a websocket
- Records every adapter request, reply, decision and reading into an
  append-only SQLite log — the Flight Recorder

## What it deliberately does not do

- **No writes, no configuration, no programming.** `clear_dtcs` is implemented
  end to end and permanently refused; the refusal is recorded with whoever
  asked for it.
- **No invented Ford specifics.** No OEM PIDs, module addresses or magic bytes.
  A module at address `7EA` is called "OBD module at 7EA" unless it says
  otherwise, because which module lives where is vehicle-specific and this
  project has not validated it.
- **No guessed vehicle details.** Manufacturer, region and model year come from
  the VIN standard's own fields. Model, trim and engine stay empty.
- **No AI yet.** The tool registry and safety gate are built; no model is
  connected.

Values decoded by definitions this project has not validated against a real
truck — the particulate-filter and exhaust-temperature parameters — are marked
`unverified` and carry a warning all the way to the UI. They are shown as raw
evidence, never as a measured fact.

---

## Layout

```
core/          the diagnostic core
  types/       shared domain vocabulary
  transport/   bytes in and out: serial, loopback
  protocols/   CAN, ISO-TP, OBD-II, UDS
  decoders/    bytes to named values, driven by data files
  adapter/     the ELM327 driver
  safety/      the L0-L3 permission gate
  session/     SQLite and the Flight Recorder
  diagnostics/ the one path to the vehicle
  tools/       typed tool registry (the agent's seam)
simulator/     virtual 2019 F-250 that speaks ELM327
apps/api/      the localhost HTTP + WebSocket API
apps/desktop/  the Tauri 2 + React shell
vehicle-profiles/  PID and DTC definitions as versioned YAML
docs/          spec, API contract, architecture
```

## Licence

MIT OR Apache-2.0
