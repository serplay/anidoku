# Plan

## M6 — Multi-source (2026-09-18, in progress)

Motivation: on 2026-09-14 allanime rotated its `x-aa-boot` scheme and the app
was a brick for 4.5 days (issue #6). One source is a single point of failure,
and ARCHITECTURE.md always listed "provider failover UX" as v1 polish. This
milestone cashes that in.

Decisions taken with the user: fix the outage first; ship HiAnime + AnimePahe
alongside allanime; both automatic failover and a manual picker; migrate
existing user data in place (keep downloads, watch progress, library).

### Done

- **Outage fixed (PR #7).** The rotation oracle tokenised the outer signature
  with a hard-coded `split(':')`; allanime moved to a `+`-joined signature with
  a reordered field list, so every known field landed in `missing[]` and the
  auto-port aborted before opening a PR. `scripts/allanime-oracle/boot-template.ts`
  now infers the separator (any character in the signature but in no known field
  value, then a fallback list) and derives the template from wherever the fields
  land. buildId 174 ported; live test green (6 sources). Unit-tested over the
  real 166/174 captures and wired into the `oracle-smoke` CI job, which
  previously only syntax-checked `derive.ts`.

  Also fixed three things that let the outage stay invisible: the health
  workflow's `reason()` regex (`[^\n]` in a POSIX bracket means "not backslash,
  not the letter n"), a failed `gh pr merge` only emitting a `::warning::`, and
  the live test panicking with a file:line instead of a readable message.

- **Source framework.** `Provider` gained `id()`/`display_name()`/
  `capabilities()` plus defaulted `status()`/`refresh_config()`, so the app
  holds `Arc<dyn Provider>` instead of a concrete scraper. New:
  `provider::registry` (ordered lookup, order + disabled set in `app_settings`),
  `provider::id::SourceId` (`"<source>:<show_id>"`, colon-free ids read as
  legacy allanime), `provider::rank` (playability ranking, never was
  allanime-specific).

- **Schema (migration 007).** Ids namespaced across `anime`/`episodes`/
  `downloads`/`watch_state`; `anime.anilist_id UNIQUE` replaced by an
  `anime_sources` table (one row per source, one preferred); `availability`
  keyed `(anilist_id, source)` and read as "any source has it".
  `downloads.dir_path` deliberately untouched — it names real directories, so
  rewriting it would orphan every completed download. 8 tests over a database
  built at the pre-007 schema prove data survives, including schema parity
  between a fresh and a migrated database.

- **Dispatch + failover** (`provider::aggregate`). Parallel search across
  enabled sources, interleaved by source rank rather than concatenated; ids
  namespaced on the way out and stripped on the way in; `VideoSource.source`
  stamped by the dispatch layer, not trusted from a scraper. `sources_for`
  tries the source the user is on, then falls across to every sibling mapped to
  the same AniList show in parallel, preserving the original error when nothing
  resolves anywhere (the outage UI keys off it). The download engine dispatches
  the same way, so a job whose source rotates mid-queue can finish elsewhere.
  15 tests.

- **UI.** Settings lists every source with enable/reorder and per-source config
  status (refusing to disable the last one). The watch page groups the picker
  by source, collapsing to the old flat "Quality" row for a single source; a
  manual pick pins that source for the show. The outage banner names the source
  that broke. 18 e2e tests.

- **CI** now runs clippy and tests over the workspace — `src-tauri` was only
  ever `cargo check`ed, so its lints went unenforced and its 5 tests never ran.

### Blocked: the two concrete scrapers

The framework is complete and allanime runs through it unchanged, but neither
target source can be scraped by a plain HTTP client as of 2026-09-18:

- **AnimePahe** — `animepahe.ru` now redirects to `animepahe.su` and serves a
  JS challenge page instead of `GET /api?m=search`. The challenge sets a cookie
  that a normal `reqwest` request cannot obtain.
- **HiAnime** — `hianime.to` does not complete a TLS handshake for a plain
  client at all (Cloudflare bot management rejects the fingerprint); the
  mirrors redirect to it.

This is an architectural decision, not a porting task, and it needs a call
before either scraper is worth writing:

1. **Browser-assisted bootstrap** — solve the challenge in a hidden webview at
   startup and hand the cookie to the Rust client. Works on desktop and mobile
   (both have a webview), mirrors how the allanime oracle already drives a real
   client, but adds a startup cost and a platform-specific shim.
2. **TLS-fingerprint impersonation** — swap `reqwest` for a client that mimics
   a browser's ClientHello. Least UI cost, but an arms race, and it changes the
   HTTP stack every source shares.
3. **Different sources** — pick ones without active bot management. Cheaper to
   port, likely shorter-lived.

Whatever is chosen, each new source inherits the allanime ops shape: its own
`<source>-config.json` remote override, an oracle, and a matrix entry in the
health workflow.

### Remaining

1. Decide the anti-bot approach above, then port the two scrapers (each with
   fixture-based parse tests and an `#[ignore]`d live test).
2. Matrix `provider-health.yml` over sources: one issue per source, "all
   sources down" as the P0 versus "one source down" as a warning.
3. Escalate `needs-human` beyond a GitHub label — the pipeline sat red 4.5 days
   unnoticed, which is the failure mode it was built to prevent.
4. De-duplicate the rotation-signature list, currently in three places
   (`allanime/mod.rs`, `src/lib/api.ts`, `provider-health.yml`).
5. `Error::ProviderRotated` as a real variant instead of the `ROTATED_PREFIX`
   string hack.
6. Startup `.expect()`s in `src-tauri/src/lib.rs` (open DB, create downloads
   dir, bind media server) crash with no UI; port-in-use and read-only-FS are
   realistic.

### Known gaps (unchanged, recorded)

- ASS/SSA subtitles are not parsed (`core/src/subs.rs:73`); the dominant fansub
  format degrades to "unsupported".
- `src-tauri/gen/apple/anidoku.xcodeproj/project.pbxproj` has an uncommitted
  change removing `libapp.a` from the Resources build phase — decide whether it
  is a real fix or regeneration drift.

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

**Signing/IPA export: DONE (2026-07-16, commit 3e51042).** After the user
added their Apple ID, two fixes were needed: `bundle > iOS > developmentTeam`
in tauri.conf.json, and a `${FORCE_COLOR:+--force-color}` guard in
`gen/apple/project.yml` (npm's `FORCE_COLOR=3` leaked into xcodebuild and was
parsed as an arch — "Arch specified by Xcode was invalid"). A signed debug
IPA now exports to `gen/apple/build/arm64/AniDoku.ipa`. Details in
BUILD-IOS.md.

**AniList login on iOS: DONE (2026-07-16).** The desktop flow (external
browser + loopback capture) dies on iOS because Safari backgrounds the app and
iOS suspends the listener. The login command now shows the auth page in the
app's own webview on iOS and navigates back after capture — verified
end-to-end in the Simulator with a real AniList sign-in. See BUILD-IOS.md.

Remaining for M5 sign-off (needs a physical device):

1. **Device install**: sideload the IPA via AltStore/SideStore or run from
   Xcode onto a plugged-in iPhone (see BUILD-IOS.md).
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
