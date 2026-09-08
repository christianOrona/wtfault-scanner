# AI Mechanic — desktop shell

Tauri 2 + React/TypeScript. A client of the `/api/v1` contract in
[`docs/API.md`](../../docs/API.md) and nothing more: no vehicle logic lives
here, and anything the window can do, `curl` can do.

## Two ways to run it

### In a browser, against a server you started

Fastest loop while working on the UI. Start the core yourself:

```bash
cargo run -p aim-api -- --simulator --scenario dpf-regen --db ./sessions.sqlite
```

then, in `apps/desktop`:

```bash
npm install
npm run dev        # http://localhost:5173
```

Vite proxies `/api` to `127.0.0.1:8787`, so the browser has no CORS story to
get wrong. Point it somewhere else with `AIM_API_URL`.

### As the desktop app

```bash
npm run tauri:dev      # dev build, console attached
npm run tauri:build    # msi + nsis installers in src-tauri/target/release/bundle
```

## How the shell hosts the core

The shell links `aim-api` as a library and runs it **in-process**, rather than
launching the binary as a sidecar. A sidecar holding an open serial port is a
sidecar that can be orphaned, and an orphaned one keeps the COM port locked
until it is killed by hand — which on a garage laptop is indistinguishable from
a broken adapter. One process cannot leak one.

It binds `127.0.0.1:8787` when that is free and asks the OS for a port when it
is not, then injects the real origin as `window.__AIM_API_ORIGIN__` before the
first line of UI script runs. A second instance, or an unrelated program on
8787, degrades to "a different port" instead of "the app is broken".

Unlike `cargo run -p aim-api`, the desktop build **persists by default**, to
`%APPDATA%/ai-mechanic/sessions.sqlite`. Someone who scanned a truck in a
driveway expects the history to still be there tomorrow.

## What the UI is careful about

The contract has some sharp edges, and most of the code here exists to respect
them:

- **Two answer shapes.** `HTTP 200` + a `ToolResult` means the operation *ran*,
  not that it worked — `client.ts` throws only for structural request errors, so
  a truck answering `NO DATA` arrives as a value with its evidence attached.
- **Warnings always render.** Several are the difference between an honest
  reading and a confident wrong one.
- **Unverified is never shown as a measurement.** A value whose
  `provenance.verification` is not `verified` is labelled and presented as raw
  evidence. The particulate-filter and exhaust-temperature PIDs are unverified
  today.
- **Nothing is invented.** No module name, DTC description or vehicle model is
  synthesised client-side. A `null` from the API is the answer, and the UI says
  so in words.
- **Codes group by `(code, status)`.** `P2463` confirmed and `P2463` permanent
  are two distinct observations and get two rows.
- **`degraded` is not a failure.** The adapter working and the truck answering
  are different facts; a degraded connect succeeds, with a serious warning.
- **Evidence is reachable.** Every value and every failure links to the event
  in the Flight Recorder that produced it — the literal adapter exchange.

## The agent tabs

**Inspect** runs a pre-purchase inspection: the agent chooses which reads to make,
makes them, and returns a verdict-first report. **Ask** is conversational over the
same tools. **Settings** is where a model provider is configured.

Presentation rules in the report view, all load-bearing:

- The verdict leads. Someone standing next to a car wants the answer first.
- Every claim is tagged `measured` / `code catalog` / `AI general knowledge`, so a
  fault read from the vehicle and a guess about what it costs never look alike.
- Costs are bands with their basis attached, never point figures.
- `not_checked` gets its own section: a plug-in scan cannot see brakes, tyres,
  suspension or rust, and a buyer who reads a clean scan as a clean car has been misled
  by this screen.
- An empty findings list is stated, not silently omitted.

## Layout

```
src/
  api/types.ts     the contract, in TypeScript
  api/client.ts    the only thing that talks to the core
  hooks/           the two websockets: live data, flight recorder
  components/      one file per pane, plus shared primitives
src-tauri/         the shell that hosts the core in-process
```

Icons are generated, not committed as art:
`powershell -File ../../scripts/make-icon.ps1` then `npm run tauri icon
src-tauri/icons/source.png`.
