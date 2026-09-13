# Evidence

Raw readings behind the mappings in `../features.yaml`, kept so a mapping can be
checked rather than believed.

## `bdycm-before-double-honk.abt`, `bdycm-after-double-honk.abt`

Whole-module as-built backups of the body control module either side of one
named change — "Double Honk On Leaving Cabine", Disabled to Enabled — with
nothing else on the vehicle altered between them.

`double-honk.diff` is the whole difference: one line of one block.

These carry no VIN in their contents, only in the filenames they were saved
under, which is why they are renamed here. The vehicle they came from is
identified in `features.yaml` by the VIN the mapping is scoped to.

## `asbuilt-bus2-2026-09-11.txt`

The secondary bus read module by module. VIN bytes redacted; see its own header.

## The mapping written back, 2026-09-13

The other direction, by this application rather than by FORScan: `DE28` byte 6
set from `01` to `00` on the module at `72E`, written, read back, and the
ignition cycled.

```
before  04 01 00 01 03 00 01 01 01 01
after   04 01 00 01 03 00 00 01 01 01
```

**The owner then confirmed the horn no longer chirps on walking away.** That
confirmation is the part worth recording. A module accepting a value and
reading it back says the write landed; it says nothing about whether the
vehicle behaves differently, and this project has already measured one case —
the mirror fold — where all four bytes read back correctly and nothing moved.

This is the first mapping in this repository confirmed in both directions and
by observed behaviour rather than by bytes alone.
