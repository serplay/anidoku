# Building AniDoku for iOS

Distribution is sideload only — sideload via AltStore/SideStore or a
free-provisioning Xcode install. No App Store, per ARCHITECTURE.md §5 (a
scraper app is ineligible anyway).

## Prerequisites

- **Xcode** (full app, not just Command Line Tools) — provides the iOS SDK,
  `xcodebuild`, and the Simulator. Verified against Xcode 26.6.
- **CocoaPods** (`brew install cocoapods`) and **XcodeGen**
  (`brew install xcodegen`). Note: `tauri ios build` does *not* regenerate
  `anidoku.xcodeproj`; after editing `gen/apple/project.yml`, run
  `xcodegen generate` inside `gen/apple/` manually.
- Rust iOS targets:
  `rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios`
- The project is scaffolded already (`npx tauri ios init` was run; the result
  is committed under `src-tauri/gen/apple`). Re-run `tauri ios init` only to
  regenerate from scratch — it overwrites `project.yml`/`Info.plist`, so the
  iOS-specific edits below would need re-applying.

## Simulator (no signing required)

The fastest verification loop; needs no Apple Developer account.

```sh
xcrun simctl boot "iPhone 16 Pro"      # or any installed iPhone runtime
npx tauri ios dev "iPhone 16 Pro"      # builds, installs, launches, hot-reloads
```

`tauri ios dev` runs the Vite dev server, cross-compiles the Rust core for
`aarch64-apple-ios-sim`, regenerates the Xcode project, and launches the app in
the Simulator. Screenshot with `xcrun simctl io booted screenshot out.png`.

Simulator builds skip code signing entirely, so this is the CI-friendly path.
Note the Simulator cannot exercise real background suspension, so the download
background-task shim (below) can only be fully validated on a device.

## Device build + sideload

A device build must be signed. With **no paid Apple Developer account** you use
*free provisioning* (a personal team): apps install for 7 days, then must be
re-signed, and are capped at 3 sideloaded apps per device.

### Option A — Xcode free provisioning (simplest)

```sh
npx tauri ios build --open      # builds the frontend + Rust, opens Xcode
```

In Xcode: select the `anidoku_iOS` target → Signing & Capabilities → check
*Automatically manage signing* → pick your personal Apple ID team. Xcode
generates a development cert + provisioning profile. Plug in the device, select
it as the run destination, and Run. This installs directly, no IPA needed.

### Option B — unsigned/ad-hoc IPA for AltStore/SideStore

AltStore re-signs the app with your Apple ID on install, so it accepts an IPA
whose own signature is throwaway. To produce an IPA:

```sh
npx tauri ios build --export-method debugging
```

Output lands in
`src-tauri/gen/apple/build/arm64/AniDoku.ipa` (path echoed at the end of the
build). `--export-method` accepts `debugging` (development),
`release-testing` (ad-hoc), or `app-store-connect`. All of them still require
*some* signing identity at archive time — see the signing note below. Once you
have the IPA:

- **AltStore/SideStore**: AltServer (desktop) or SideStore (on-device) installs
  the IPA and refreshes the 7-day signature over Wi-Fi. Point it at the IPA.
- Re-open/refresh weekly (free-provisioning limit), or use a paid account
  (`release-testing` export, 1-year certs) to avoid the weekly refresh.

### Signing note

Signing needs an Apple ID added in Xcode → Settings → Accounts (free
provisioning is enough). The development team ID is set in
`tauri.conf.json` under `bundle > iOS > developmentTeam` — without it the
build fails with a "You must set the code signing certificate development
team ID" warning followed by a script-phase error. Find your team ID with
`security find-identity -v -p codesigning` (it's the OU of the cert, shown
by Tauri's warning as "Available certificates: <name> (ID: XXXXXXXXXX)").
IPA export via `--export-method debugging` is verified working on this
machine with a free personal team.

### Known build failure: "Arch specified by Xcode was invalid"

If the Rust build phase dies with `Arch specified by Xcode was invalid.
{arch} isn't a known arch`, an npm-style `FORCE_COLOR=3` env var has leaked
into xcodebuild and is being parsed as an architecture. cargo-mobile2 uses
`FORCE_COLOR` to smuggle its `--force-color` flag into the script phase, and
npm/terminals set the same variable to `1`/`2`/`3`. The script template in
`gen/apple/project.yml` guards against this with
`${FORCE_COLOR:+--force-color}` — keep that guard if the project is ever
re-scaffolded with `tauri ios init`. Note that despite what xcodegen docs
suggest, `tauri ios build` does **not** regenerate `anidoku.xcodeproj` from
`project.yml`; after editing `project.yml`, run `xcodegen generate` inside
`gen/apple/` yourself.

## iOS-specific pieces (where to look when something breaks)

- **Native HLS**: iOS WKWebView plays HLS natively via the `<video>` element,
  so hls.js is bypassed. The player feature-detects this
  (`video.canPlayType('application/vnd.apple.mpegurl')`) rather than sniffing
  the user agent — see `attachMedia()` in
  `src/routes/watch/[id]/[ep]/+page.svelte`. This is the *same* code path macOS
  desktop already uses (WKWebView there too), so it is exercised on every macOS
  run. Quality/source switching does not depend on hls.js — each quality is a
  separate source URL that re-attaches — so it works unchanged. Provider and
  external subtitle `<track>`s attach to the native player the same way.
- **TLS**: the workspace uses `reqwest` with **rustls** (no openssl/native-tls,
  which does not cross-compile). Do not reintroduce native-tls. Same constraint
  as Android.
- **Cleartext / ATS**: the in-app media server, HLS proxy, and OAuth loopback
  capture are plain HTTP on `127.0.0.1`. App Transport Security blocks cleartext
  by default, so `Info.plist` (and `project.yml`, the regeneration source) set
  `NSAppTransportSecurity → NSAllowsLocalNetworking = true` to exempt loopback.
  This is the iOS analogue of Android's `network_security_config.xml`.
- **Custom scheme**: `stream://` cover images work natively in WKWebView (unlike
  Android, which needs `http://stream.localhost`). `streamUrl` in
  `src/lib/api.ts` only rewrites for Android, so iOS falls through to
  `stream://localhost/` correctly.
- **Background downloads**: `Sources/anidoku/DownloadBackgroundTask.swift`
  exposes `anidoku_set_download_active(_:)` via `@_cdecl`; Rust
  (`src-tauri/src/ios.rs`) calls it whenever the queued/downloading row count
  crosses zero (same trigger as Android's foreground service). It holds a
  `UIApplication` background task to keep the download engine alive off-screen.
  **Honest limitation**: a UIKit background task only grants a short grace
  window (currently ~30s) before iOS reclaims it — it is *not* the unbounded
  equivalent of an Android foreground service. Foregrounded downloads and the
  brief background tail work; truly unbounded background downloading would
  require migrating the reqwest/tokio engine to a native background
  `URLSession` (out-of-process, system-resumed) — a substantial rewrite not
  attempted in M5. The download engine already checkpoints per HLS segment, so
  a suspended download resumes cleanly when the app is next foregrounded.

## App icon

`npx tauri icon <png>` writes the iOS icon set to
`src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset` (and the Android set).
`tauri ios init` seeds the default Tauri logo; re-run `tauri icon` after
changing the source art.
