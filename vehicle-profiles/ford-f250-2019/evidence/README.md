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
