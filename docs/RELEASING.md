# Cutting a release

What a person (or an agent) does to ship a version. The CI side — building,
signing, verifying and drafting — is documented in the header of
[`.github/workflows/release.yml`](../.github/workflows/release.yml) and is not
repeated here.

The short version: write the changelog entry, review `STATUS.md`, bump the
version everywhere it appears, run the full check, tag, push. CI does the rest
and leaves a **draft** that a person publishes.

## 1. Pick the version

Below 1.0 everything is allowed to move, but the convention so far has been:

- **Minor** (`0.4.0`) when something changed incompatibly — that one renamed
  `VehicleBus` and changed what discovery asks each bus.
- **Patch** (`0.3.9`) for additions, even substantial ones.
- **Minor** also when a release is simply too big to read as a patch. `0.5.0`
  was 33 commits adding two outside data sources, dual-bus scans, the scorecard
  and session replay.

## 2. Write the changelog entry

A new `## [x.y.z] — YYYY-MM-DD` section at the top of
[`CHANGELOG.md`](../CHANGELOG.md), directly under the four-line preamble.

**This is the release notes.** The workflow extracts exactly this section and
puts it in the GitHub release, which is what the in-app updater shows somebody
before they install. Write it for the person using the app, not for developers:
bold lead sentence, then why it matters, in plain language. `### Added`,
`### Changed` and `### Fixed` are the usual sections; freeform ones are fine
where they read better.

Use the release date, not the date the work was done.

## 3. Review STATUS.md

[`STATUS.md`](../STATUS.md) is where the project actually is, and it drifts.
The 0.4.0 release existed partly to fix "a STATUS.md that had drifted from what
the truck actually showed", so treat it as part of cutting a release rather than
as a chore for later.

- **What is built** gains whatever the release added.
- **What is not built** gains the honest limits of that same work, and loses
  anything the release closed.
- **Verified on real hardware** takes nothing unless it actually ran against a
  vehicle. Simulator and fixture work is not evidence, and this section is the
  one place where that distinction is load-bearing.
- Bump **Last reviewed** — which asserts the whole document was read, so read it.

`docs/HANDOFF.md` is the original specification, not a worklog. It has not
changed since the initial commit and a release does not touch it.

## 4. Bump the version — in more places than you think

**Four files CI cross-checks.** The workflow refuses to build if these disagree
with each other or with the tag:

| | |
|---|---|
| `apps/desktop/src-tauri/tauri.conf.json` | `"version"` |
| `apps/desktop/package.json` | `"version"` |
| `Cargo.toml` | `[workspace.package]` → `version` |
| `apps/desktop/src-tauri/Cargo.toml` | `[package]` → `version` |

**Eleven more files CI does not check, and which will fail the build anyway.**
Every workspace crate pins its siblings with an explicit version —
`aim-types = { version = "0.5.0", path = "../types" }` — and those are not
covered by `[workspace.dependencies]`. Bump all of them:

```bash
git grep -l 'version = "<old>"' -- '*/Cargo.toml' \
  | xargs sed -i 's/version = "<old>"/version = "<new>"/g'
```

They live in `core/adapter`, `core/agent`, `core/decoders`, `core/diagnostics`,
`core/protocols`, `core/safety`, `core/session`, `core/tools`, `core/transport`,
`apps/api` and `simulator` — 41 occurrences at 0.5.0. The command above also
rewrites `apps/desktop/src-tauri/Cargo.toml`'s own `version`, which is already
in the table above and is harmless to substitute twice: 12 files, 42
occurrences in total.

**Both lockfiles.** There are two Cargo workspaces: the core one, and the
desktop shell, which is its own. CI runs `cargo build --locked`, so a lockfile
still naming the old version fails the release:

```bash
cargo update --workspace
cd apps/desktop/src-tauri && cargo update --workspace
```

Run these **after** the `Cargo.toml` edits. `cargo update` resolves the pinned
sibling requirements, so it errors out if the step above is half done — which is
a useful check that it is not.

**Leave `apps/desktop/package-lock.json` alone.** Its `version` field has said
`0.1.0` since before 0.4.x. `npm ci` only validates the dependency tree, so it
does not care, and every release since has shipped that way.

### Check the footprint against the last release

The fastest way to confirm nothing was missed:

```bash
git show <previous release commit> --stat
```

18 files at 0.5.0: the changelog, the two lockfiles, and 15 manifests.

## 5. Run the full check

```bash
powershell -NoProfile -File scripts/check.ps1
```

**Not `-Quick`.** It skips `core: no serial` and `shell: clippy`, and the script
says so itself in its closing note. The full run is about 90 seconds.

## 6. Commit, tag, push

The commit is `release: x.y.z` with a one-line summary of the release. The tag
is annotated and must be exactly `v` + the version, or the workflow stops:

```bash
git commit -m "release: x.y.z"
git tag -a vx.y.z -m "x.y.z"
git push origin HEAD:main
git push origin vx.y.z
```

## 7. A person publishes it

The tag starts the workflow (about 11 minutes). It compiles, signs the
executable **before** it goes inside an installer, packages, signs the
installers, verifies every signature including the executable *inside* each
installer, writes `SHA256SUMS.txt`, and creates a **draft** release.

Publishing is deliberately manual: the in-app updater installs whatever is
latest-published, so a release that published itself would install itself on
people's machines. Check the draft, then press the button.

`gh release list` shows what is actually published versus still drafted — worth
a look, because a draft left unpublished means people are still being offered
the version before it.

## Traps

- **Testing the release-notes extraction locally gives a false negative.** The
  workflow pulls the changelog section with `awk` using `"^## \\[" v "\\]"`.
  Under gawk that builds a character class and matches nothing; CI runs on
  `ubuntu-latest`, where `awk` is mawk, and it works. Confirm with
  `gh release view <previous tag> --json body -q .body` rather than by running
  the awk locally. The workflow is not broken — do not "fix" it.
- **A manual `workflow_dispatch` run is a dry run.** It never touches Releases,
  whatever signing mode is chosen.
- **Signing is automatic once SignPath is configured.** Until
  `SIGNPATH_ORGANIZATION_ID` exists the build is unsigned, says so in the
  release notes, and the verification step asserts the files really are
  unsigned — so an unsigned build can never be mistaken for a signed one.
