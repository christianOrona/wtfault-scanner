# `automotive_diag`: evaluated, not adopted — table gap closed instead

**Decided 2026-09-11.** Issue #40.

## The question

`automotive_diag` is a `no_std`, definitions-only crate covering UDS, KWP2000,
OBD-II and DoIP, under MIT OR Apache-2.0 — this project's own licence — and
maintained (0.1.29, published 2026-08-19). Our UDS tables are hand-written and
cover the subset we needed. Should we adopt it?

The issue set the bar explicitly: **not on download count.** On whether the
tables are more complete than ours and whether the mapping layer stays thin.
*If adopting it means writing more conversion code than the tables save, it is
not worth it.*

## What was actually compared

| | ours, before | `automotive_diag` |
|---|---|---|
| Named NRC variants | 17 | 40 |
| Unknown values | `Other(u8)` | `ByteWrapper<T>::Extended(u8)` |
| Licence | — | MIT OR Apache-2.0 |
| Dependencies | none | `strum` |

The gap was not random. Ours was missing the whole **`0x81`–`0x93` block**,
which is the specific vocabulary for *why* a vehicle's state is wrong:
`engineIsRunning`, `vehicleSpeedTooHigh`, `shifterLeverNotInPark`,
`voltageTooLow`, and fifteen more.

That block is the most valuable thing in either table for this project. A
module answering `conditionsNotCorrect` has told somebody the vehicle is in the
wrong state and left them to guess which. A module answering `engineIsRunning`
has told them to turn the key. Before this, every one of those arrived as
`Other(0x8n)` and was reported as an *unexplained* refusal — the module had
already given the answer and we threw it away.

## The decision: take the table, not the dependency

Three reasons, in order of weight.

**1. The conversion layer is the same size as the data.** Adopting means a
match arm from 40 foreign variants onto our 7 `RefusalKind`s, plus enabling
their `serde` feature, plus reshaping a type that is part of our HTTP wire
contract. Writing the 23 missing variants ourselves is pure data of about the
same length. The issue's own test — more conversion code than the tables save —
comes out roughly even, and a tie goes to fewer dependencies.

**2. `ByteWrapper<T>` is not better than `Other(u8)`, just different.** It is a
tidier general solution to the same problem and produces the same outcome:
a standardised value or the original byte. There is no behaviour we could have
with it that we cannot have now, so it is not a reason to migrate.

**3. The KWP2000 argument in the issue does not hold.** It reasoned that a
maintained definition set is a better starting point than our guesses for
non-CAN multi-line reassembly. But our caveat there is about *reassembly* —
how bytes are put back together on a transport — and a definitions crate does
not address transport behaviour at all. Adopting it would not have retired that
caveat, and believing it would was the weakest step in the original case.

## What changed as a result

All 23 missing codes added, and the `0x81`–`0x93` block wired to
`NegativeResponseCode::what_to_change()` — a concrete instruction, `None` for
every refusal that is not about the vehicle's state, because there is nothing
to do about `securityAccessDenied` except not ask. A refused write now reports
"put the selector in park" rather than "conditions not correct".

One case deliberately refuses to be encouraging: `vehicleSpeedTooLow` says it
needs a second person driving or a rolling road, and says not to attempt it
while reading a screen.

`RefusalKind`, the plain-language explanations and `worth_retrying` are
untouched, as the issue required. This replaced tables, not meaning.

## What would change this decision

- If we implement KWP2000 or DoIP properly, their coverage there is real and
  ours is nothing.
- If the ISO-14229 table starts moving. It has not; it is a published table
  from a stable standard, which is why owning 40 constants is cheap.
- If we need the service, session, routine or scaling enumerations they carry
  and we do not. That is a much larger surface than NRCs and the argument
  would be different.

## Also recorded, so nobody re-treads it

`ecu_diagnostics`, `isotp-rs`, `ecu-uds` and `j1939` are GPL. In Rust a GPL
dependency is linked into the binary and relicenses the whole application,
which is a harder constraint than the same library would be in Python.
