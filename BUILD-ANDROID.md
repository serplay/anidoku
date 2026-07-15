# Building AniDoku for Android

Distribution is a direct APK (sideload / F-Droid-style) — no Play Store, per
ARCHITECTURE.md §5.

## Prerequisites

- Android Studio (or plain SDK) with: platform 36, build-tools, NDK 29.x,
  platform-tools, emulator (optional).
- Rust Android targets:
  `rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android`
- JDK 17–21. Android Studio's bundled JBR works:
  `/Applications/Android Studio.app/Contents/jbr/Contents/Home`

Environment (adjust paths to your machine):

```sh
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME="$ANDROID_HOME/ndk/29.0.13599879"
```

## Debug build (emulator / USB device)

```sh
npx tauri android build --debug --target aarch64
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

`--target aarch64` covers modern phones *and* Apple Silicon emulators. Add
`--target armv7` etc. for older devices. `npx tauri android dev` works too
(hot-reloading webview against the Vite dev server).

## Release build (signed APK)

One-time keystore setup:

```sh
keytool -genkey -v -keystore ~/anidoku-release.jks \
  -keyalg RSA -keysize 2048 -validity 10000 -alias anidoku
```

Then create `src-tauri/gen/android/keystore.properties` (gitignored):

```properties
keyAlias=anidoku
password=<store/key password>
storeFile=/Users/you/anidoku-release.jks
```

Build:

```sh
npx tauri android build --apk    # per-ABI + universal APKs, signed
```

Outputs land under
`src-tauri/gen/android/app/build/outputs/apk/**/release/`. Keep the keystore
and its password out of the repo and backed up — APK updates must be signed
with the same key or Android refuses to install over the old version.

## Android-specific pieces (where to look when something breaks)

- **TLS**: the whole workspace uses `reqwest` with **rustls** — do not
  reintroduce native-tls/openssl; it doesn't cross-compile here.
- **System bars / notch**: `MainActivity.kt` pads the content view with
  window insets (Android WebView never fills CSS `env(safe-area-inset-*)`).
- **Foreground service**: `DownloadForegroundService.kt`, toggled from
  `src-tauri/src/android.rs` (JNI via `with_webview`/`jni_handle`) whenever
  queued/downloading rows cross zero. Manifest declares `dataSync` type.
- **Cleartext**: the in-app media server and OAuth capture are plain HTTP on
  `127.0.0.1`; `res/xml/network_security_config.xml` exempts loopback from
  the release cleartext block.
- **Custom scheme**: `stream://` cover images become
  `http://stream.localhost/` on Android (`streamUrl` in `src/lib/api.ts`).
- **OAuth**: the loopback capture on `127.0.0.1:8737` works on-device — the
  system browser and the app share the device's loopback interface.
