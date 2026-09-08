# Vehicle profiles

Drop `.yaml` files in this folder and the app loads them every time it starts.
Nothing needs rebuilding and nothing needs reinstalling — add a file, restart,
and Settings will show you exactly what it read.

This is how the app learns things it did not ship knowing.

## What you can put here

**Feature mappings** — the useful one. The app ships knowing *that* a truck can
have auto-folding mirrors, and not *where* the setting lives, because that is
manufacturer-specific and unpublished. If you work it out on your own vehicle,
put it here:

```yaml
features:
  - id: mirror_auto_fold
    name: "Automatic folding mirrors"
    easy: "Mirrors fold in when you lock the truck."
    technical: "Power-fold auto-actuation bit in the door module."
    risk: convenience
    modules: [DDM, PDM]
    verification: verified      # only this permits a write
    mapping:
      kind: as_built_bits
      block: "740-01"
      byte: 2
      mask: 0x30
      on: 0x10
      off: 0x00
```

**Live-data parameters** the app cannot decode yet — anything listed under
"Offered, but we can't read these":

```yaml
service: 1
pids:
  - pid: 0x9D
    signal_id: fuel_injection_timing
    name: "Fuel injection timing"
    kind: numeric
    bytes: 2
    unit: "degrees"
    formula: "(A*256+B)/128 - 210"
    verification: unverified
```

**Explanations**, in plain language and in technical language:

```yaml
signals:
  - id: fuel_injection_timing
    easy: "How early the diesel is squirted into the cylinder."
    technical: "Commanded start of injection relative to top dead centre."
```

## How to work out a mapping

Never guess. The method that actually works is a diff:

1. Read the vehicle's configuration and save it.
2. Change the one setting with a tool already known to do it correctly.
3. Read the configuration again.
4. Compare. The bits that moved are the mapping.

Change one thing at a time, or you will not know which bits belong to what.

## Rules the app enforces regardless of what you put here

- `verification: verified` is the only thing that allows a write. Anything else
  can be read and shown, never changed.
- Features classed `safety_critical`, `security` or `programming` are never
  written, whatever the file says. Giving one a verified mapping does not
  change that.
- Everything loaded from this folder is labelled on screen with the file it came
  from, so a wrong mapping is traceable to whoever supplied it.
- A file that will not parse is reported in Settings and skipped. It will not
  stop the app from starting, and it will not silently do nothing.

## Sharing

These files are plain text and safe to pass around — they contain no VIN and
nothing about your vehicle unless you put it there. A mapping you verify is
worth more to the next person than to you.
