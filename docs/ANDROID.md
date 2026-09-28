# The Android app

The phone goes to the car; the laptop with Bluetooth is slow. So the same app
also builds for Android, and the phone becomes the scanner.

It is **one app with two shells**, not two apps. The Rust core, the `/api/v1`
contract and the React interface are shared. Android only owns two things:

- **How it reaches the adapter.** A phone has no serial ports. A Kotlin plugin
  owns the Bluetooth socket and the shell installs it as the core's *platform
  link* provider, so paired adapters are listed and opened like ports.
- **Its layout.** Below 760 px wide the interface stacks, and anything wider
  than the screen scrolls inside its own box.

If a bug ever has to be fixed twice, once per platform, the split is in the
wrong place.

## Install it on a phone

**From a release.** Each release has an `.apk` beside the Windows installers.
Open the release page on the phone, download the APK and open it. Android asks
once to allow installs from the browser or file manager. Later versions install
over it and keep your sessions.

**From any commit on main.** Every CI run builds a debug APK and keeps it for
14 days: open the run under *Actions → check*, and download
`wtfault-scanner-android-debug` from the bottom of the page. A debug build is
signed with a throwaway key, so it cannot update a release install or be
updated by one: uninstall one before installing the other.

## Connect to the adapter

1. Pair the ELM327 in Android's settings first: *Settings → Connected devices →
   Pair new device*. The PIN is usually `1234` or `0000`.
2. Open the app, choose **Real adapter**, and pick the adapter by name.
3. The first time, Android asks for the **Nearby devices** permission. It is
   needed to connect to a paired device, and it is the only permission the app
   asks for. The app never scans, so it never asks for location.

When nothing can be listed, the list says why instead of being empty:
Bluetooth off, permission not granted, nothing paired.

Only **Bluetooth Classic** adapters work, which is almost every cheap ELM327
(they appear as a COM port on Windows). Bluetooth LE-only and Wi-Fi adapters are
not supported yet.

## How it differs from the desktop

| | Windows | Android |
|---|---|---|
| Adapter link | COM port (Bluetooth SPP or USB) | Bluetooth Classic, by device |
| Where data lives | `%APPDATA%\ai-mechanic\data` | The app's private storage |
| Updates | Downloads and runs the installer | Says a new version exists; install the APK from the release page |
| Probe button | Opens each port and sends `ATZ`/`ATI` | Lists without probing: opening a Bluetooth link is slow and visible |

Sessions do not move between the two by themselves. They are separate
databases on separate devices.

## Building it

### Once, on Windows

- **JDK 17 or newer.** `JAVA_HOME` must point at it.
- **Android SDK** in `C:\Android\sdk` (the build script's default; set
  `ANDROID_HOME` to use another place). The command-line tools are enough, no
  Android Studio needed:

  ```bash
  sdkmanager "platform-tools" "platforms;android-36" "build-tools;36.0.0" "ndk;29.0.14206865"
  ```

  The NDK version is pinned in `.github/workflows/check.yml` and
  `release.yml`; keep all three in step.
- **Rust targets:**

  ```bash
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
  ```

### Build

```bash
powershell -NoProfile -File scripts/build-android.ps1
```

That is a debug APK for a phone. `-Target x86_64` builds one for the emulator,
and `-Release` a signed release build. The script prints where the APK landed.

Why a script rather than `npx tauri android build`: Tauri symlinks the compiled
library into the Gradle project, and Windows only lets an ordinary user create
symlinks in **Developer Mode**. Without it, the Tauri build compiles everything
and then fails on that one step. The script notices, copies the library into
place and runs Gradle itself. With Developer Mode on, the Tauri build just
succeeds and the fallback never runs.

### Try it without a phone

```bash
sdkmanager "emulator" "system-images;android-35;google_apis;x86_64"
avdmanager create avd -n wtfault -k "system-images;android-35;google_apis;x86_64" -d pixel_7
emulator -avd wtfault
adb -s emulator-5554 install -r <the x86_64 APK>
```

The emulator has no Bluetooth to pair, so use **Virtual vehicle**. That runs the
whole core, the simulator included, on Android.

Pass `-s emulator-5554` to every `adb` command: other Android devices on the
network, such as a TV, can show up in `adb devices` too.

## Release signing

Android refuses an update signed by a different key than the installed app. So
the release key **must never change** once anyone has installed a release, and
losing it means everyone has to uninstall and reinstall.

- The keystore lives in repository secrets (`ANDROID_KEYSTORE_BASE64`,
  `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`). The release workflow
  writes it to `gen/android/keystore.properties` and Gradle signs with it.
- **Keep a backup outside the repository.** The original is in
  `%USERPROFILE%\.android-keys\` on the machine that created it. Copy that
  folder somewhere safe (a password manager or an offline drive).
- For a signed local build, copy `keystore.properties` from that folder into
  `apps/desktop/src-tauri/gen/android/`. It is gitignored.
- Without the secrets, the release workflow builds an unsigned APK and the
  release notes say that Android will not install it.

## Where things are

| | |
|---|---|
| `apps/desktop/src-tauri/gen/android/` | The Gradle project. Generated once by `tauri android init`, then edited and committed |
| `…/java/com/wtfault/scanner/BluetoothClassicPlugin.kt` | The Bluetooth socket: list, open, write, read, close |
| `apps/desktop/src-tauri/src/android.rs` | The core's `Transport` over that plugin, and the link provider |
| `core/transport/src/platform.rs` | The seam: how a shell offers links the OS does not expose as ports |
| `…/res/xml/network_security_config.xml` | Plain HTTP to `127.0.0.1` only, which the interface needs to reach the core |
| `scripts/build-android.ps1` | The Windows build, with or without Developer Mode |

## Not done yet

- **Never run against a real adapter.** Everything above has run in the emulator
  against the virtual vehicle. The Bluetooth path needs a phone, a paired
  adapter and the truck, and until then it is unverified.
- **iOS.** iPhones cannot use Bluetooth Classic adapters at all. iOS needs a
  Bluetooth LE or Wi-Fi adapter, a Mac to build on and an Apple developer
  account.
- **Google Play.** Releases are APKs on GitHub for now.
