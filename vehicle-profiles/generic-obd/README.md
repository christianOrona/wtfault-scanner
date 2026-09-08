# generic-obd profile

Generic SAE OBD-II (J1979 service definitions, J2012 DTC format) only.
**Nothing manufacturer-specific belongs in this directory.**

Every entry carries a `verification` field:

| value        | meaning                                                                                       |
|--------------|-----------------------------------------------------------------------------------------------|
| `verified`   | Checked against the public standard and safe to present as a factual reading.                  |
| `unverified` | Present so the plumbing can be exercised, but **not validated**. The core attaches an `unverified_decoder` warning to any value produced from these and `DecodedValue::is_trustworthy()` returns false. Never use one to make a claim. |
| `rejected`   | Known wrong; kept only so old transcripts still parse.                                          |

Formulas use the conventional SAE byte names: `A` is the first payload byte
after the PID echo, `B` the second, and so on.
