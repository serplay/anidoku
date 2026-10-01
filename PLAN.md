# Plan

## M6 — Multi-source (2026-09-18 → 2026-10-01)

Motivation: on 2026-09-14 allanime rotated its `x-aa-boot` scheme and the app
was a brick for 4.5 days (issue #6). One source is a single point of failure,
and ARCHITECTURE.md always listed "provider failover UX" as v1 polish. This
milestone cashes that in.

Decisions taken with the user: fix the outage first; ship more sources
alongside allanime (originally HiAnime + AnimePahe — see "Sources shipped" for
why it became AniZone + AnimeGG); both automatic failover and a manual picker;
migrate existing user data in place (keep downloads, watch progress, library).

### Done

- **Outage fixed (PR #7).** The rotation oracle tokenised the outer signature
  with a hard-coded `split(':')`; allanime moved to a `+`-joined signature with
  a reordered field list, so every known field landed in `missing[]` and the
  auto-port aborted before opening a PR. `scripts/allanime-oracle/boot-template.ts`
  now infers the separator (any character in the signature but in no known field
  value, then a fallback list) and derives the template from wherever the fields
  land. buildId 174 ported; live test green (6 sources). Unit-tested over the
  real captures and wired into the `oracle-smoke` CI job, which previously only
  syntax-checked `derive.ts`. allanime rotated again before this merged
  (buildId 177, `/`-joined, yet another field order); the same oracle derived
  it unmodified on 2026-10-01, and that capture is now a test case too.

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

### Sources shipped (2026-10-01)

The framework above was blocked on its two target scrapers: AnimePahe serves a
JS challenge and HiAnime refuses the TLS handshake of a non-browser client, and
both were still unreachable on 2026-10-01 (as were AnimeKai, 9anime-family
mirrors, AnimeFLV, and anime-sama). Of the three ways out recorded here before
— browser-assisted bootstrap, TLS-fingerprint impersonation, or different
sources — the third was taken: it needs no new machinery and, unlike the other
two, does not add a second thing that rotates.

Selection criteria: reachable by plain `reqwest`, no client-side crypto, and
media that plays in a `<video>` without an embed-page extractor.

| Source | What it serves | Sub/Dub | Subtitles | Notes |
|---|---|---|---|---|
| **AllAnime** (`allanime`) | mixed hosts via signed GraphQL | both | hard | carries AniList ids; rotates (oracle + remote config) |
| **AniZone** (`anizone`) | one adaptive HLS master per episode | sub | soft, ASS, ~20 languages | data inlined as JSON in the page; long shows page via Livewire |
| **AnimeGG** (`animegg`) | self-hosted MP4, 360–1080p | both | hard | catalogue has gaps (missing episodes) — failover covers them |

Surveyed and not taken: KickAssAnime (JSON API for search/episodes is open,
but sources sit behind an encrypted player — a second rotating source),
AnimeHeaven and gogoanime clones (reachable; kept in reserve), AniWorld and
AnimeUnity (reachable, but German/Italian catalogues).

What the two new sources needed beyond a scraper each:

- **ASS subtitles.** AniZone's tracks are ASS, which `subs.rs` did not parse.
  `ass_to_vtt` converts the dialogue (override tags dropped, `\N` breaks, vector
  drawings skipped, top alignment kept); the media server converts `.ass`/
  `.ssa`/`.srt` on the way through, so a `<track>` can point at whatever a
  source serves. `SubtitleTrack.default` lets a soft-subbed source say which
  track must start enabled.
- **Demuxed HLS downloads.** AniZone keeps audio in separate renditions; the
  download engine took only the video variant and produced a silent file. It
  now downloads the matching audio playlist too (default language for sub,
  English for dub) under one resume checkpoint and writes a small local master.
- **Failover that works for shows nobody linked.** `sources_for` only fell
  across to sources already mapped to the same AniList show — which is never
  the case for a show opened while only allanime existed. Siblings are now
  *discovered* by a strict title match (`sync::matching::confident_provider_match`:
  near-exact title under any known name, and the same season — plain similarity
  cannot tell seasons apart), persisted once found, and remembered as a miss
  for 30 minutes when not. `episodes` fails over the same way, so the show page
  itself opens during an outage. `resolve_provider_for_anilist` links every
  source's match up front (`aggregate::link_matches`).
- **Health.** `provider-health.yml` walks each source live (search → episodes →
  sources → first bytes of media through the real proxy client) with one issue
  per source. One source down is a warning; all of them down is labelled
  `all-sources-down`.

Tests: fixture-based parser tests over captured pages for both sources, each
scraper driven end to end against a scripted local server (site down, page
changed, dead mirror, missing episode), the demuxed download through the real
engine, and e2e coverage of the failover note and default subtitle track.
`cargo test -p anidoku-core --test sources_live -- --ignored` is the live walk.

### Adding a source

1. `core/src/provider/<slug>/{mod.rs,parse.rs}`; keep parsing pure and test it
   against captured pages in `fixtures/` (see `anizone`, `animegg`).
2. Shape every error `<stage>: <detail>` and distinguish "no results" from
   "page layout changed" — the health workflow and its issue text key off both.
3. Return `Ok(vec![])`, not an error, for an episode the source lacks: empty
   means "fall across", an error is reserved for "this source is broken".
4. Register it in `src-tauri/src/lib.rs`, add its name to `SOURCE_NAMES` in the
   watch page, a `live_<slug>` test in `core/tests/sources_live.rs`, and a
   matrix entry + name in `provider-health.yml`.

### Remaining

1. Episode lists are not merged across sources: a show opened from AnimeGG
   lists only AnimeGG's episodes, so one it lacks is not offered at all (it
   does play if reached). Merge the lists of linked siblings.
2. AniZone's dub audio is in the stream but the player has no audio-track
   picker, so the source is registered sub-only.
3. Listing a very long show on AniZone costs one request per 24 episodes
   (~50 for One Piece, cached for 10 minutes afterwards).
4. Escalate `needs-human` / `all-sources-down` beyond a GitHub label — the
   pipeline sat red for weeks unnoticed, which is the failure mode it was built
   to prevent.
5. De-duplicate the rotation-signature list, currently in three places
   (`allanime/mod.rs`, `src/lib/api.ts`, `provider-health.yml`).
6. `Error::ProviderRotated` as a real variant instead of the `ROTATED_PREFIX`
   string hack.
7. Startup `.expect()`s in `src-tauri/src/lib.rs` (open DB, create downloads
   dir, bind media server) crash with no UI; port-in-use and read-only-FS are
   realistic.

### Known gaps (unchanged, recorded)

- ASS/SSA subtitles are converted as dialogue only: fonts, colours, karaoke
  and positioning other than top/bottom are dropped.
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
