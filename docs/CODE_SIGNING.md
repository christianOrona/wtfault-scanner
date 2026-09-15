# Code signing policy

Free code signing provided by [SignPath.io](https://signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

> **Status: applied for, not yet approved.** Until it is, releases are built by
> the same workflow described below but are **not signed**, and each release
> says so. Windows SmartScreen warns before an unsigned installer runs. This
> notice is removed once signed releases ship.

---

## What is signed

Only files built from this repository's source by its own continuous
integration, from a tagged commit:

| File | What it is |
|---|---|
| `WTFault Scanner.exe` | The desktop application. Signed before it is packaged, so the copy inside each installer is signed too. |
| `WTFault.Scanner_<version>_x64-setup.exe` | The NSIS installer. |
| `WTFault.Scanner_<version>_x64_en-US.msi` | The MSI installer. |

Nothing else is signed. In particular: nothing built on a developer's machine,
no third-party component, and no build of anybody else's code. The uninstaller
the NSIS installer writes at install time is generated on the user's machine and
is not signed.

## How a release is built and signed

Everything happens in [`.github/workflows/release.yml`](../.github/workflows/release.yml),
on a GitHub-hosted Windows runner, and the whole run is public in the
repository's Actions tab.

1. A maintainer pushes a tag `vX.Y.Z` to a commit on `main`.
2. The workflow checks the tag matches every version number in the repository,
   then builds from that commit with the pinned Rust toolchain
   (`rust-toolchain.toml`) and the committed lockfiles (`Cargo.lock`,
   `package-lock.json`). No build cache is used.
3. The application executable is uploaded to SignPath, which verifies it was
   produced by this repository's workflow run before signing it.
4. The signed executable is packaged into both installers, and the installers
   are signed the same way.
5. The workflow checks every signature — including the executable unpacked from
   inside each installer — and fails if any file is not signed by SignPath
   Foundation.
6. The installers and a `SHA256SUMS.txt` are attached to a **draft** release.
7. A maintainer reviews the draft and publishes it. Nothing is ever published
   automatically, because the application's updater offers the latest published
   release to people who already have it installed.

Every signing request under the release policy is approved by hand in SignPath
by an approver listed below, after checking that it came from the expected tag
and workflow run.

The artifact configurations SignPath applies are kept for review in
[`.signpath/artifact-configurations/`](../.signpath/artifact-configurations/).

## Team roles

This is a one-person project, so one person holds every role. Changes from
anyone else are merged only after the maintainer has reviewed them. The default
branch is protected by a repository ruleset: it cannot be deleted or
force-pushed, and changes reach it through pull requests. Only the maintainer
can bypass it.

| Role | Who | Responsibility |
|---|---|---|
| Committer | [Christian Orona](https://github.com/christianOrona) | May push to the repository. |
| Reviewer | [Christian Orona](https://github.com/christianOrona) | Reviews and merges every change from anybody else. |
| Approver | [Christian Orona](https://github.com/christianOrona) | Approves each signing request in SignPath. |

Every account holding one of these roles uses multi-factor authentication on
both GitHub and SignPath.

## Privacy policy

This program will not transfer any information to other networked systems unless
specifically requested by the user or the person installing or operating it,
**with the exceptions listed below**. They are listed individually so none of
them has to be inferred.

### What it does without being asked

- **Checks for a newer version** a few seconds after it starts, by asking
  GitHub's public API for this repository's latest release
  (`api.github.com/repos/christianOrona/wtfault-scanner/releases/latest`). No
  vehicle data, identifier or setting is sent; GitHub sees an ordinary web
  request from the computer's address.
- **During installation only, if the Microsoft Edge WebView2 runtime is missing**
  — it ships with Windows 10 and 11, so this is rare — the installer downloads
  and runs Microsoft's WebView2 bootstrapper, which the application needs to
  draw its window. That download is Microsoft's, from Microsoft.

### What it sends when a person asks it to

- **Downloads a newer installer**, when the check above has found one and the
  person presses **Download**, from this repository's GitHub release. It is
  installed only when they then press **Install and restart**.

- **To the AI model provider the person configured** — and only once they have
  configured one and asked a question or started an inspection: the question,
  readings and fault codes from the connected vehicle, what the application has
  recorded about that vehicle, and a description of the adapter. Whether the
  vehicle's VIN is included is a setting, **Keep it on my own network** by
  default: sent to a model running on this computer or the person's own network,
  withheld from any other. A hosted provider (for example Anthropic or xAI)
  receives this data under its own privacy policy; a local one (for example
  Ollama) keeps it on hardware the person controls. No provider is configured
  after installation.
- **To an address the person enters** when importing a vehicle profile from a
  URL: a request for that file, over HTTPS, with nothing attached.

### What it never sends

- No analytics, telemetry, crash reports or usage statistics, to anybody.
- Problem reports are assembled on the computer and shown to the person, who
  decides whether to copy or save them. The application does not send them.
- Vehicle as-built files, session history and recorded findings stay on the
  computer.

### What it keeps on the computer

In the user's application data folder (`%APPDATA%\ai-mechanic` on Windows):

| | |
|---|---|
| `data\sessions.sqlite` | Every session: readings, fault codes, the adapter exchange behind them, VINs, recorded findings, imported as-built files. |
| `data\providers.json` | Model provider settings. API keys are stored in the operating system's credential store where one exists; the settings screen states where each key is actually kept. |
| `data\logs\` | Application logs, used when reporting a problem. |
| `data\profiles\` | Vehicle profile files the person added. |

## What it does to the computer and to a vehicle

**The computer.** The NSIS installer installs for the current user, under
`%LOCALAPPDATA%`; the MSI is the alternative for managed installs. Either adds
shortcuts to start the application, and installs Microsoft's WebView2 runtime
only if it is missing. The application changes no system settings and installs
no service, driver or background task; it runs only while its window is open.
It is removed like any other program, from Windows Settings → Apps, using the
installer it came with; the data folder above can be deleted afterwards to
remove everything it recorded.

**A vehicle.** This is a diagnostic tool, in the same category as the scan tools
workshops use. Through a standard OBD-II adapter plugged in by the person, it:

- **reads** — identification, fault codes, live sensor data, self-test results,
  configuration records — which needs no confirmation;
- **clears fault codes** and **changes configuration settings** such as a horn
  chirp or door-lock behaviour, only when a person types a confirmation and only
  with the engine off and the vehicle stopped. A setting can be changed only
  where its location has been recorded, and it is read back afterwards. Every
  request and answer is recorded in the session log.

It cannot program or flash modules — that capability is compiled out — and it
changes nothing in the braking, steering, throttle or airbag systems,
immobilisers or keys, though it can read their fault codes. It does not bypass
or defeat vehicle security, and has no means to: it contains no security key
algorithm and no code that sends a key. The AI assistant can read and explain,
but cannot reach any path that writes to a vehicle.

The full rules are in [`docs/SAFETY.md`](SAFETY.md).

## Verifying a download

Right-click the file → **Properties** → **Digital Signatures**: the signer is
**SignPath Foundation**. Or, in PowerShell:

```powershell
Get-AuthenticodeSignature '.\WTFault.Scanner_0.4.3_x64-setup.exe' |
  Format-List Status, SignerCertificate
```

`Status` should be `Valid`. Checksums for every file are in the release's
`SHA256SUMS.txt`:

```powershell
(Get-FileHash '.\WTFault.Scanner_0.4.3_x64-setup.exe' -Algorithm SHA256).Hash
```

## Releasing (maintainers)

1. Bump the version in `Cargo.toml`, `apps/desktop/package.json`,
   `apps/desktop/src-tauri/Cargo.toml` and `apps/desktop/src-tauri/tauri.conf.json`.
   The workflow refuses a tag that does not match all four.
2. Move the `[Unreleased]` changelog entries under `## [X.Y.Z] — date`. The
   release notes are taken from that section, and the updater shows them to
   people before they install.
3. Run `scripts/check.ps1`, commit, and push to `main`.
4. `git tag vX.Y.Z && git push origin vX.Y.Z`.
5. Approve the two signing requests in SignPath when they appear — the
   executable first, then the installers.
6. Check the draft release, give it a title, and publish it.

A dry run without releasing anything: **Actions → release → Run workflow**, with
signing `none` or `test-signing`.

## Reporting a problem with a signed file

If a file signed for this project looks wrong — signed but not built from this
repository, or behaving in a way this page does not describe — open an issue, or
contact the maintainer directly if it is sensitive. It can also be reported to
[SignPath Foundation](https://signpath.org).
