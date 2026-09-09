# Safety

This is about not damaging vehicles and not hurting people, which is a different
question from the one in `SECURITY.md`.

## Two ceilings, not one

Two independent limits, because "how privileged is this operation" and "what
happens if it is wrong" are different questions and collapsing them loses the
distinction that matters.

### Permission level — how privileged

| | | |
|---|---|---|
| **L0** | reads | runs freely |
| **L1** | non-invasive tests, clearing codes | needs a typed confirmation naming a person |
| **L2** | configuration changes | needs a typed confirmation and every precondition |
| **L3** | programming, firmware | **compiled off** |

`MAX_ENABLED_LEVEL` is a constant in `aim-safety`. Changing it is a reviewable
code change — not a setting, not a config file, and never something a model can
ask for.

### Risk class — what happens if it is wrong

| | |
|---|---|
| `Read` | nothing changes |
| `Cosmetic` | chimes, animations, gauge styling |
| `Convenience` | mirror fold, lighting, lock behaviour |
| `Service` | maintenance resets and relearns on non-safety systems |
| `DiagnosticControl` | commanding an actuator to move |
| `SafetyCritical` | **refused** — braking, steering, throttle, restraints |
| `Security` | **refused** — immobiliser, keys |
| `Programming` | **refused** — firmware |

`MAX_ENABLED_RISK` is `Service`. Everything above it is refused **as policy**,
and the refusal says so in those words.

That wording is deliberate. "Nobody has measured this yet" and "this tool will
never do that" are different sentences, they look identical from the outside,
and a person is owed the right one. The first is a gap you can close. The second
is a decision about what this tool is for, and no amount of evidence changes it.

## Why safety-critical is refused permanently

A convenience setting and a brake calibration are the same permission level.
Only one of them can hurt somebody. A diagnostic tool that a person runs in
their own driveway, on a vehicle they will then drive on a public road, has no
business changing how that vehicle stops.

Immobiliser and key operations are refused for a different reason: over a wire,
the legitimate uses are indistinguishable from theft.

Firmware is refused because a failed write leaves a module that does not boot,
and recovering it needs equipment this app cannot assume you have.

## What has to be true before a write

Every one of these, evaluated fresh at the moment of the write rather than
trusted from an earlier preview — the engine may have been started since.

- The feature exists in the catalogue. A caller can name a feature id and a
  value, and nothing else; there is no field for an address.
- Its risk class is within the ceiling.
- A mapping exists **and has been verified against a real vehicle**.
- The module that owns it was actually observed on this vehicle.
- The adapter can transmit and has not produced a truncated reply this session.
  A truncated read is the signature of an adapter mishandling multi-frame flow
  control, and configuration writes are multi-frame.
- Ignition on, engine off, vehicle stationary.
- Battery at least **12.4 V**. Module writes that lose power partway through are
  the classic way to produce a control unit that no longer answers.
- A typed confirmation naming a person. The model cannot supply one, and cannot
  reach the operation at all.

A failed check is reported as a specific question with a specific answer, not as
"unavailable".

## A write is not believed until it is seen

```
read the record  →  change the masked bits  →  write it whole
                 →  read it back  →  compare
```

A positive response from a module means the module accepted the request. It does
not mean the setting changed. If the read-back does not match what was written,
the result is reported as **unverified** and the final state as unknown — not as
success, and with no automatic retry, because a second write on top of an
unknown state is how a bad situation gets worse.

If the record comes back shorter than the mapping expects, that is an error and
nothing is written: the mapping does not match this vehicle.

## What the model can and cannot do

It can propose. It can explain. It can run reads and non-invasive tests.

It cannot change a vehicle setting or clear codes — those are marked
`agent_forbidden` and refused with the initiator recorded in the flight
recorder, before any confirmation is even considered, so that supplying one
gets a model no further.

This is enforced in the capability gate rather than in the prompt, because
prompt instructions are not a security boundary.

## Standards, and where they stop

The transport and services are ISO 14229 and SAE J1979, which is why this works
on vehicles nobody wrote special code for. Module addressing, data identifiers,
scaling, available sessions and security access are **per-manufacturer**. The app
treats those as data it either has for your vehicle or honestly does not, and it
does not extrapolate one manufacturer's numbers onto another's vehicle.

## If you are working on this

The invariants worth not breaking:

1. A request type must not have a field capable of expressing an arbitrary write.
2. Nothing may be reachable by an `agent:` initiator that is marked forbidden.
3. A write reports success only after read-back verification.
4. A refusal must say whether it is a gap or a policy.
5. Preconditions are evaluated at execution, never inherited from a preview.

Each has a test. If you change one, change its test deliberately and say why in
the commit message rather than adjusting the assertion until it passes.
