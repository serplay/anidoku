# Plan

## M5 — iOS (2026-07-16)

Scaffolded and **simulator-verified** (iPhone 16 Pro, iOS 26.5); build recipe
in BUILD-IOS.md. Toolchain present: Xcode 26.6, iOS rust targets, CocoaPods,
XcodeGen, iOS simulators installed.

Done:

- `tauri ios init` → XcodeGen project committed under `src-tauri/gen/apple`.
- **Native-HLS playback**: already implemented — the player feature-detects
  `video.canPlayType('application/vnd.apple.mpegurl')` and bypasses hls.js
  (no UA sniffing). This is the *same* path macOS WKWebView already uses, so
  it is continuously exercised on desktop. Quality/source switching is
  independent of hls.js (each quality is a separate source URL); provider +
  external `<track>` subtitles attach to the native player. No change needed.
- **Background download shim**: `DownloadBackgroundTask.swift` (`@_cdecl`
  `anidoku_set_download_active`) holds a `UIApplication` background task while
  downloads are active; toggled from `src-tauri/src/ios.rs` on the same
  queued/downloading-crosses-zero trigger as Android's foreground service.
  Linked via the standard Tauri Rust↔Swift pattern (cdylib link gets
  `-undefined dynamic_lookup` in build.rs; the app links the staticlib where
  the Swift symbol resolves).
- **ATS / cleartext**: `NSAllowsLocalNetworking` in Info.plist + project.yml so
  WKWebView can load the loopback media server / HLS proxy / OAuth capture
  (iOS analogue of Android's network_security_config).
- `stream://` custom scheme works natively in WKWebView (no localhost rewrite,
  unlike Android) — `streamUrl` already only rewrites for Android.
- BUILD-IOS.md written (simulator loop + AltStore/free-provisioning sideload +
  signing story).

Verified on the iOS Simulator: full debug build compiles + links (Rust core for
`aarch64-apple-ios-sim` on rustls, Swift shim, app archive), installs, and
launches; the webview UI renders (proving the ATS loopback exemption works for
the app shell).

Remaining for M5 sign-off (needs hardware/credentials this machine lacks):

1. **Device sideload + signing**: this Mac has **no code-signing identity** and
   no Apple Developer team, so device install / IPA export could not be run.
   Needs the user to sign into Xcode with an Apple ID (see BUILD-IOS.md).
2. **On-device background-download behaviour**: the background-task shim links
   and is wired, but the Simulator does not model real background suspension,
   so its actual effect (and iOS's short grace window vs. Android's unbounded
   foreground service) is unverified on a device. Honest limitation documented:
   truly unbounded background downloads would need a native background
   `URLSession` rewrite of the engine — not attempted in M5.
3. **On-device native-HLS streaming smoke test**: playback path is the macOS
   code path and builds for iOS, but an end-to-end stream on a real device is
   untested (simulator streaming not driven here).

## M4 — Android (in progress, near complete; 2026-07-15)

Done and verified on the Pixel_8 emulator (Android 16, arm64) — see
BUILD-ANDROID.md for the build recipe:

- Debug APK builds (`tauri android build --debug --target aarch64`);
  reqwest moved to **rustls** (openssl-sys does not cross-compile).
- Mobile-responsive pass < 768px: bottom tab bar (top nav collapses),
  per-page narrow-screen fixes; system-bar insets padded natively in
  MainActivity (Android WebView never fills `env(safe-area-inset-*)`).
- Streaming playback verified on-device (hls.js/MSE + loopback media
  server); downloads verified (307 MB episode completed); offline playback
  of the downloaded episode verified ("Playing offline copy" path).
- Download **foreground service** (dataSync) verified end to end: starts
  with the queue via JNI (`with_webview → jni_handle().exec`), posts the
  notification, stops when the queue drains. ndk-context does NOT work
  under Tauri v2 mobile — don't regress to it.
- Covers fixed on Android (`stream://` → `http://stream.localhost`).
- App data moved cache/ → files/ (Android may clear cache; DB + downloads
  were landing there).
- Release signing config (gitignored keystore.properties) + loopback
  cleartext exemption for release builds; BUILD-ANDROID.md written.

- **OAuth loopback verified on Android** with an injected redirect: Chrome
  → 127.0.0.1:8737 delivery, fragment bridge, token capture, and clean
  error surfacing all work on-device. No deep-link plugin needed. (A real
  login with actual AniList credentials remains a 5-minute human task:
  Settings → client ID → Sign in.)

Remaining for M4 sign-off:

1. **Real-device smoke test** (only emulator tested so far) + a signed
   release APK build once a keystore exists (needs the user's keystore —
   see BUILD-ANDROID.md).
2. Cosmetic: watch page shows the raw provider id as the title when
   deep-linked from Downloads after an app restart (summary cache empty);
   could warm from media_cache like the list pages do.

Notes: desktop gates all green (cargo test 123 passed, check/build/svelte-check
clean). One flaky provider download observed ("error decoding response body"
mid-stream from the CDN) — Retry exists; not an M4 regression.
