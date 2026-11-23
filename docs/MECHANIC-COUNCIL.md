# Local agents and the Mechanic council

Design for letting the desktop app's chat run on the AI agents a person
already has installed (Claude Code, Codex, Gemini CLI), and for letting
several of them look at the same fault together.

Status: built on `claude/project-thread-gthg2c`. Nothing here has been run
against a real vehicle; see [What needs a car](#what-needs-a-car).

---

## Why

Today Ask and the AI inspection call a hosted model on the owner's own API
key, or an Ollama box. Plenty of people who would use this already pay for a
Claude or ChatGPT subscription and have the CLI signed in. Paying again per
token for the same model is the thing to remove.

The second half is the "council": one model reading a fault is one opinion.
Two or three models reading the same evidence, saying where they agree and
where they do not, is closer to asking more than one mechanic.

The approach is borrowed from Commonroom (`christianorona/commonroom`,
`src/providers/claude-cli.mjs`, `codex.mjs`, `engine/process.mjs`), which
already drives the installed CLIs on Windows with the person's own sign-in.

## Goals

- The chat can use an installed **Claude Code**, **Codex** or **Gemini CLI**
  as its model, signed in with the person's own subscription.
- Installed agents are **found automatically**; a path can be set by hand.
- A **council** of two to five configured models (local agents, API models,
  Ollama, in any mix) answers one question together.
- The API-key providers keep working unchanged and stay the fallback.

## Non-goals

- Mobile. There are no CLIs on a phone. Android hides all of it.
- Letting a local agent use its *own* tools. It edits no files, runs no
  commands and reaches no MCP servers. See below.
- Council for the structured inspection report. Ask only, in this round.

## Anti-goals (a failure even if it works)

- **A second path to the vehicle.** A local agent must reach the truck only
  through the same tool registry, schema validation and safety gate as every
  other model.
- **Silent API billing.** If `ANTHROPIC_API_KEY` happens to be set in the
  environment, Claude Code would quietly bill it. The child process never
  sees API-key variables (Commonroom does the same).
- **A busier Ask screen.** One line saying who is on the council, and the
  individual opinions folded away under the answer.

---

## Approaches considered

1. **Text tool protocol over one-shot CLI calls (chosen).** Each model turn
   is one non-interactive CLI run. The prompt carries the system prompt, the
   tool catalogue, the conversation and an instruction to ask for tools in a
   `<tool_calls>` JSON block. The reply is parsed back into the same
   `Content::ToolUse` blocks every other provider produces, so the existing
   agent loop, claim checks and safety gate apply untouched.
   *Costs:* a few seconds of CLI start-up per step; a model can malform the
   block (handled: parsed leniently, asked once to fix it).
2. **Expose the vehicle tools to the CLI as an MCP server.** Native tool
   calls, but the CLI would then hold a live route to the truck outside the
   agent loop: no step limit, no "you already read that", no claim check.
   Rejected on the first anti-goal.
3. **Long-lived app-server sessions** (what Commonroom does for Codex).
   Streams nicely, but each CLI speaks its own protocol, and a scanner needs
   one answer per question, not a session. Can be added later behind the same
   `CliRunner` seam if streaming is wanted.

For the council:

1. **Shared evidence, then opinions, then a merged answer (chosen).** The
   lead model gathers evidence through the normal tool loop. Every other
   member is shown exactly that evidence and the lead's draft, and writes its
   own diagnosis with no tools. The lead merges. Every member reasons over
   the same bytes, the adapter is read once, and a member that cannot do tool
   calls well still contributes.
2. **Every member drives its own tool loop.** N times the adapter traffic,
   N different evidence sets to reconcile, and receipts that point at
   different reads. Rejected.
3. **Open debate for several rounds.** More tokens and minutes for answers
   that mostly converge after one look. Left as a possible setting.

---

## How it works

### Finding agents (`aim_agent::local_agents`)

Per agent, candidate executables are collected and the newest version wins
(Commonroom's rule; several installs often coexist):

| Agent | Where it looks, besides `PATH` |
|---|---|
| Claude Code | `~/.local/bin`, npm global, Claude desktop's own copies under `%APPDATA%`/`%LOCALAPPDATA%` |
| Codex | npm's vendored binary, `%LOCALAPPDATA%\OpenAI\Codex\bin\*` |
| Gemini CLI | `PATH` and npm global |

On Windows `PATHEXT` is honoured, so `claude.cmd` from npm is found. A GUI
app does not inherit a shell's `PATH` on macOS, so the usual Homebrew and
npm folders are added there too. Sign-in is checked where the CLI says so
cheaply (`claude auth status`, `codex login status`).

`GET /api/v1/settings/local-agents` returns what was found. Settings shows it
with one **Add** button per agent.

### Running one (`aim_agent::provider::local_agent`)

A new provider kind, `local_agent`, carrying which agent and an optional
executable path. Each turn:

- spawns the CLI in an **empty temporary folder**, prompt on stdin (Windows
  caps a command line at 32 KB; a system prompt with vehicle context does not
  fit), no console window, killed at the timeout (5 min default);
- removes API-key variables from its environment;
- turns its own tools off:

| Agent | Arguments |
|---|---|
| Claude Code | `-p --output-format json --tools "" --safe-mode --strict-mcp-config --no-session-persistence --permission-mode dontAsk [--model M]` |
| Codex | `exec --sandbox read-only --skip-git-repo-check --ephemeral --color never -C <empty dir> -o <file> - [-m M]` |
| Gemini CLI | `--approval-mode plan [-m M] -p " "` (prompt on stdin) |

The reply text goes through `text_tools::parse_reply`. A malformed block is
handed back once with the parse error; a second failure is shown as prose.

`locality_of` treats a local agent as **somebody else's computer**: the CLI
runs here, but it sends the prompt to Anthropic, OpenAI or Google. The VIN
privacy setting therefore applies to it exactly as to a hosted API.

### The council (`aim_agent::council`)

```
question ─► lead: normal tool loop ─► evidence + draft
                                          │
            ┌─────────────────────────────┼──────────────┐
            ▼                             ▼              ▼
        member A                      member B        member C     (in parallel, no tools)
            └──────────────┬──────────────┴──────────────┘
                           ▼
                 lead merges: agreed, disagreed, what to check next
                           ▼
            claim check (no "no faults" without a read) ─► answer
```

- **Lead** is the selected provider. **Members** are chosen in Settings, up
  to four. A member that fails (not signed in, timed out) is skipped and
  named; the answer still comes back. If every member fails, the lead's
  draft is the answer.
- Members are told they cannot read the vehicle, must cite only the
  evidence shown, and must say plainly when they disagree.
- The merged answer is held to the same "no faults without a read" check as
  a single-model answer, using the reads the lead actually made.
- Members' opinions are returned with the answer (`council.opinions`) so the
  interface can show them folded under it.
- Privacy: when the council is on, the VIN is withheld unless **every**
  member is allowed to receive it.

### Settings and API

```json
// providers.json
"council": { "enabled": true, "members": ["prov_…", "prov_…"] }
```

| Route | |
|---|---|
| `GET /api/v1/settings/local-agents` | what is installed, version, signed in |
| `POST /api/v1/settings/council` | `{ enabled, members }` |
| `GET /api/v1/settings/providers` | now also returns `council` and `local_agents_supported` |
| `POST /api/v1/agent/messages` | unchanged request; adds `council` to the response when it ran |

### Interface

- **Settings → Model providers:** a "Local agents on this computer" row with
  an **Add** button per agent found, and *Local agent* in the kind list.
- **Settings → Mechanic council:** one switch and a tick box per other
  configured provider.
- **Ask:** one line under the input, "Mechanic council: Claude Code, Codex".
  Under each answer, a closed "What each mechanic said".

---

## Calls made by the builder

- Members run in parallel. Two Ollama members on one GPU box will queue on
  that box; Commonroom runs local models one at a time for that reason.
- 5-minute timeout per CLI turn; step limits are the lead provider's own.
- Model left blank means the CLI's own default.

## What needs a car

Nothing here changes how the vehicle is read, but the council has only been
run against the simulator and scripted models. At the truck: one Ask with a
real fault and the council on, to see whether the members' opinions add
anything and how long the extra round takes on a laptop.

## Not yet verified

- The CLIs' actual replies to this protocol on the owner's own sign-in. The
  flags were checked against Claude Code 2.1, Codex 0.171 and Gemini CLI 0.70
  `--help`. A real signed-in Claude Code accepted the exact arguments and
  answered in the JSON shape the parser reads. Codex accepted them and stopped
  at the network; Gemini accepted them but had no sign-in to answer with.
- Gemini CLI turns `--approval-mode plan` back to `default` in a folder it does
  not trust, which the empty temporary folder always is. With its tools
  unused that is harmless, but it is worth a look on a real run.
- Windows: discovery paths and `.cmd` launching are written from Commonroom's
  working code and unit-tested on path logic only.
