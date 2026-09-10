# Community signal definitions

The `*.json` files in this directory are **signalsets from [OBDb](https://obdb.community)**,
a community effort to document the diagnostic parameters and trouble codes that
vehicles answer to. One file per make and model, in OBDb's `signalsets/v3`
format.

## Licence

OBDb content is licensed **Creative Commons Attribution-ShareAlike 4.0
International (CC BY-SA 4.0)** — <https://creativecommons.org/licenses/by-sa/4.0/>

That is **not** the licence of the rest of this repository, which is
MIT OR Apache-2.0. These files are *data*, kept in their own directory so the
boundary is obvious:

- Attribution to OBDb must be preserved.
- Modifications to these files, and works derived from them, stay CC BY-SA 4.0.
- Do not paste this content into source files elsewhere in the tree, which would
  blur a licence boundary that is currently clean.

Source repositories are at <https://github.com/OBDb>, one per vehicle.

## What these are, and are not

These are **claims recorded by a community**, not measurements taken from the
vehicle in front of you. Nobody in this project has verified that a given
identifier exists on your vehicle, that it means what the definition says, or
that the scaling is right for your model year.

The code that reads them treats them accordingly: every reading derived from a
definition here is `SourceKind::ProfileData` and `VerificationStatus::Unverified`
until the vehicle itself has been shown to answer it sensibly. See
`core/decoders/src/signalset.rs`.

## Files

| File | Source repository | Retrieved |
|---|---|---|
| `Ford-F-150.json` | <https://github.com/OBDb/Ford-F-150> | 2026-09-10 |

Many OBDb repositories exist but are still empty stubs — `Ford-F-250` and
`Honda-Odyssey` were both empty when checked on 2026-09-10. A missing or empty
catalogue is a normal state, not an error.
