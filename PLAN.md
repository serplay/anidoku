# Plan

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
