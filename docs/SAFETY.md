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
| **L1** | non-invasive tests, clearing codes | needs a person's approval |
| **L2** | configuration changes | needs a person's approval and every precondition |
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
- A mapping exists **and has been verified against a real vehicle**, with one
  exception that is deliberate and narrow. See "The one unverified write" below.
- The module that owns it was actually observed on this vehicle.
- The adapter can transmit and has not produced a truncated reply this session.
  A truncated read is the signature of an adapter mishandling multi-frame flow
  control, and configuration writes are multi-frame.
- On a vehicle listed as having a security gateway, the owning module's write
  gate measured open. See below.
- Ignition on, engine off, vehicle stationary.
- Battery at least **12.4 V**. Module writes that lose power partway through are
  the classic way to produce a control unit that no longer answers.
- A person's approval of exactly this change. See "What an approval is" below.
  The model cannot get one, and cannot reach the operation at all.

A failed check is reported as a specific question with a specific answer, not as
"unavailable".

### The one unverified write

A mapping is verified by changing the setting some other way and watching which
bits move. For anything a vehicle's own menus do not offer there is no other
way, so that rule alone would mean this tool could never change most of what
somebody comes to it for.

So a mapping nobody has verified can be written when all of these hold, and
never otherwise:

- its risk class is `Cosmetic` or `Convenience`;
- the module that owns it was measured accepting writes, by this application
  asking it. A statement that it does, typed in or imported, does not count;
- everything else in the list above, the approval included.

It is called an experiment before the person approves it and in the result
afterwards, the bytes as they stood are read first so they can be put back, and
the read-back decides what is reported. It is never called verified.

## What an approval is

A person's approval used to be a string in the request: anything that was not
empty and not spelled `agent:` passed. It is now a value only the application's
own approval desk can create.

1. The interface asks for an approval of one operation on one thing: this
   setting to this value, this module's codes.
2. The core answers with a word to type.
3. The operation is sent with what the person typed. The core checks the word,
   and that the approval is for exactly this, has not been used, and is recent.

The safety gate accepts an approval only from the desk it was built to trust.
The tool registry the model speaks through holds no desk and has no field an
approval could travel in.

What this does not establish is that a human pressed the keys. A program running
as you on this machine can ask and answer as the interface does. That program
could also open the serial port itself, so it is inside the boundary this
application has.

## Security gateways are a boundary, not an obstacle

Some manufacturers put a module between the diagnostic port and the networks
that lets an unauthenticated tool read but not change anything. Those listed in
`vehicle-profiles/gateways/gateways.yaml` are reported as what they are:

```
Gateway access required. Status: unavailable.
Reason: ... blocks this operation for a tool that has not authenticated
with the manufacturer.
```

When the vehicle is identified, in the write-gate probe's verdict, and as a
check in every write plan. Access comes from the manufacturer's own
authentication, and this build has none and will not imitate one: **there is
no bypass, and one will not be added.** The listing is from public
documentation, so a module measured accepting writes overrules it, and the
probe that measures that writes nothing.

## A write is not believed until it is seen

```
read the record  →  change the masked bits  →  write it whole
                 →  read it back  →  compare
```

A positive response from a module means the module accepted the request. It does
not mean the setting changed. What the module said and what the record holds
afterwards are kept apart, and only the record decides what is reported:

| The record reads back as | Reported as |
|---|---|
| what was written | **verified** |
| exactly what it was | **not applied**: nothing changed |
| something else | **changed unexpectedly**: not verified, state not known |
| nothing, it could not be read | **unknown**, as a failure that says so |

The same goes the other way. A module that answered "still working on it", or
whose answer was lost, is not reported as having refused: the record is read, and
if it holds what was written the write is verified.

There is no automatic retry of a write, because a second write on top of an
unknown state is how a bad situation gets worse.

If the record comes back shorter than the mapping expects, that is an error and
nothing is written: the mapping does not match this vehicle.

## What the model can and cannot do

It can propose. It can explain. It can run reads and non-invasive tests.

It cannot change a vehicle setting or clear codes — those are marked
`agent_forbidden` and refused for any caller that is not recognisably a person,
with the initiator recorded in the flight recorder, before any approval is even
considered, so that presenting one gets a model no further.

This is enforced in the capability gate rather than in the prompt, because
prompt instructions are not a security boundary.

## Nothing leaves that is not a read, unless it was authorised

Every request to the vehicle goes through one type, and it looks at the service
byte. A read goes out. A service that changes something goes out only with the
authorization the gate granted for an operation that lists that service, and
only the gate can make one. Anything else is refused before the adapter sees it.

That covers data as well as code. Community signal files state their own service
and come from a folder and from a download, so a file that describes a write as
a reading is not offered, is not built into a request, and would not be sent if
it were.

## Standards, and where they stop

The transport and services are ISO 14229 and SAE J1979, which is why this works
on vehicles nobody wrote special code for. Module addressing, data identifiers,
scaling, available sessions and security access are **per-manufacturer**. The app
treats those as data it either has for your vehicle or honestly does not, and it
does not extrapolate one manufacturer's numbers onto another's vehicle.

## If you are working on this

The invariants worth not breaking:

1. A request type must not have a field capable of expressing an arbitrary write.
2. Nothing marked forbidden may be reachable by anything but a person, however
   the caller spells its own name.
3. A write reports verified only after a read-back that matches, and reports
   unknown, not failed and not done, when it could not check.
4. A refusal must say whether it is a gap or a policy.
5. Preconditions are evaluated at execution, never inherited from a preview.
6. A person's approval is a value from the approval desk, never a string, and
   the tool registry has nowhere to carry one.
7. A service that is not a read is sent only under the gate's authorization for
   an operation that lists it.
8. The level in an audit record comes from the capability registry.

Each has a test. If you change one, change its test deliberately and say why in
the commit message rather than adjusting the assertion until it passes.
