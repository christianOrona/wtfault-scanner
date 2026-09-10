# Security

This app talks to a vehicle and to a language model. Both of those are
untrusted inputs in the sense that matters here: neither one's output may be
allowed to become an instruction.

## Threat model

What this design actually defends against, in rough order of how likely it is.

### A model that says the wrong thing

The realistic failure, not a hypothetical one. Models reason wrongly, get
confident, and produce plausible sentences about vehicles they have not read.

The defence is structural rather than instructional. **Prompt text is not a
security boundary.** A model reaches the vehicle only through the typed tool
registry (`aim-tools`), every tool call passes the capability gate
(`aim-safety`), and the operations that change a vehicle are marked
`agent_forbidden` and refused with the initiator recorded — regardless of what
the model says, what confirmation it supplies, or how the request is phrased.

There is a test that sweeps the whole capability registry and fails if anything
marked agent-forbidden is reachable by an initiator beginning `agent:`, so a
capability added later is covered without anyone remembering this document.

### A model that emits a malformed or hostile tool call

Arguments are validated against each tool's JSON Schema before execution, and
the schemas are closed (`additionalProperties: false`). A call naming a tool
that does not exist is rejected as unknown rather than attempted.

The important property is in the *shape* of the request types. A configuration
change carries a feature id and a desired value and has nowhere to put a module
address, a data identifier, a byte offset, or a raw frame. An arbitrary write is
not something the gate refuses — it is something the type system cannot express.

### A profile file that is wrong or hostile

Vehicle knowledge is data loaded at runtime from a folder. That is the feature,
and it is also an input from outside the program.

Profiles are **data, not code**: YAML describing identifiers, scalings and bit
positions. There is no expression evaluation, no code loading, no shell-out. A
profile cannot make the app do something it could not otherwise do; it can only
change which numbers a supported operation uses.

That still matters, because a wrong mapping written to a module is how a module
stops working. So:

- Everything loaded from a profile is labelled on screen with the file it came
  from, and a wrong mapping is traceable to whoever supplied it.
- A mapping that has not been verified against a real vehicle can be used to
  read and never to write.
- A write whose value would land outside its own declared bit mask is refused,
  because it would change settings the feature does not describe.
- A record shorter than the mapping expects is an error, not a padded guess.

Treat a profile from a stranger the way you would treat a script from a
stranger, notwithstanding all of the above.

### A tool result that does not mean what it looks like

An adapter reply is parsed and classified, never trusted as prose. A positive
response to a write is **not** treated as a successful change: the record is
read back and compared, and a write that cannot be verified is reported as
unverified rather than as success.

### Someone else on the network

The diagnostic API binds to loopback only, and the UI is served same-origin from
the core's own port. That is deliberate and load-bearing: a vehicle-facing
server with permissive CORS would let any web page you happen to have open read
your vehicle, and in the worst case ask it for things.

There is no authentication on the local API, because there is no network
surface. If that ever changes — a phone app, a remote session — it needs
explicit pairing, short-lived session credentials, and no raw CAN or arbitrary
UDS endpoint. Do not expose the current API to a LAN as-is.

## Credentials

Provider API keys are stored in a JSON file in the user's own profile,
**in plain text**. That is worse than the OS credential store and is stated
plainly in the app's About screen rather than glossed.

What is already true:

- Keys are held in a `Secret` wrapper whose `Debug` implementation redacts them,
  with tests asserting a key cannot appear in a formatted error.
- Keys are never written to the flight recorder.
- Keys are never sent to the frontend.

What is not yet true, and should be: storage in the Windows Credential Manager
or an equivalent. Until then, a program running as your user can read the file.

## Where your data goes

Worth being exact, because "local" is doing a lot of work in most privacy
copy:

| Setup | Where the data goes |
|---|---|
| Ollama on this machine | Nowhere. It does not leave the computer. |
| Ollama on another box on your network | It leaves this machine for that one. |
| Anthropic, xAI, any hosted endpoint | It goes to that provider. |

Scans are stored on this machine only, in a SQLite database whose path the About
screen shows. A VIN identifies a specific vehicle and, by extension, often a
specific person; minimising what is sent to a hosted model is worth doing and is
not yet a setting.

## Reporting something

This is a personal project without a security team. Open an issue for anything
that is not itself sensitive; for something that is, contact the repository
owner directly rather than filing it publicly.
