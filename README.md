# AniDoku

A cross-platform anime streaming app: search, pick an episode, quality and
subtitles, and play — with AniList sync and managed offline downloads. It is a
native reimplementation of [ani-cli](https://github.com/pystardust/ani-cli)'s
flow as a GUI app, built on **Tauri 2** (Rust core) and **Svelte 5**.

Runs on macOS, Windows, Linux, Android (APK) and iOS (sideload).

> **Status:** v0.1.0. Desktop, Android (emulator-verified) and iOS
> (simulator-verified, signed IPA export works) all build and stream. See
> [Roadmap](#roadmap-and-known-gaps).

---

## Contents

- [Features](#features)
- [How it works](#how-it-works)
- [Quick start (desktop)](#quick-start-desktop)
- [AniList sign-in](#anilist-sign-in)
- [Mobile builds](#mobile-builds)
- [Repository layout](#repository-layout)
- [Development](#development)
  - [Quality gates](#quality-gates)
  - [Playwright end-to-end tests](#playwright-end-to-end-tests)
  - [Continuous integration](#continuous-integration)
- [The streaming provider and how it stays alive](#the-streaming-provider-and-how-it-stays-alive)
  - [What a rotation looks like](#what-a-rotation-looks-like)
  - [Self-healing pipeline](#self-healing-pipeline)
  - [Fixing a rotation by hand](#fixing-a-rotation-by-hand)
- [Configuration reference](#configuration-reference)
- [Troubleshooting](#troubleshooting)
- [Roadmap and known gaps](#roadmap-and-known-gaps)
- [Legal notes](#legal-notes)
- [Further reading](#further-reading)

---

## Features

**Streaming**
- Search (with AniList-powered catalog filters and tags), episode list, sub/dub.
- In-app player: HLS via hls.js where the webview needs it, native HLS on
  Apple platforms. Quality chips switch between provider renditions.
- Provider subtitle tracks plus external SRT/ASS files (converted to WebVTT in
  Rust).
- Dead-source auto-advance: if a host fails to load, the player silently moves
  to the next candidate and only reports an error when every source is
  exhausted. Embed pages (mp4upload, ok.ru) are resolved server-side into
  direct media links.
- Resume position, next/previous episode, autoplay-next, keyboard shortcuts.

**Downloads**
- Persistent SQLite-backed queue with pause, resume and cancel.
- HLS (per-segment, resumable) and MP4 (byte-range resume) engines with
  bounded exponential backoff on transient CDN errors.
- Offline playback through the same player via a loopback media server, so
  online and offline share one code path. Android runs downloads in a
  foreground service; iOS holds a background task while the queue is active.

**AniList**
- OAuth sign-in (loopback capture on desktop and Android, in-app webview on
  iOS), library view, list-entry editing, progress auto-sync when an episode
  crosses the watched threshold.
- Offline-safe outbound mutation queue with conflict resolution.
- Airing tracker with an in-app inbox and new-episode notifications.

**Operations**
- Remote provider configuration: when the streaming provider rotates its
  anti-scraping scheme, installed apps fetch a published JSON fix on the next
  play attempt. No release required.
- A scheduled health check that detects a rotation, derives the new values
  from the live web client automatically, and lands the fix by pull request.

---

## How it works

```
┌────────────────────── UI (SvelteKit SPA in the Tauri webview) ─────────────────────┐
│  Home · Search · Anime detail · Watch · Library · Downloads · Inbox · Settings      │
└──────────────┬──────────────── Tauri IPC (commands + events) ───────────────────────┘
               │
┌──────────────▼──────────── anidoku-core (one Rust crate, all platforms) ────────────┐
│  Provider engine        Download manager           AniList sync                     │
│  trait Provider →       SQLite job queue,          GraphQL client, OAuth,           │
│  allanime impl          HLS/MP4 resume, backoff    mutation queue, conflicts        │
│                                                                                     │
│  Media server + stream proxy (loopback): downloaded files, rewritten playlists,     │
│  converted subtitles, and live streams with the referer the CDN expects             │
│                                                                                     │
│  SQLite: anime · episodes · downloads · list entries · sync queue · airing cache    │
└─────────────────────────────────────────────────────────────────────────────────────┘
   Thin platform shims: Android foreground service + JNI, iOS background task
   (Swift), iOS in-webview OAuth navigation.
```

The hard, breakage-prone logic (scraping, downloads, database, sync) lives in
`core/` and compiles unchanged for every target. `src-tauri/` is the desktop
and mobile shell exposing it as commands. `src/` is the Svelte frontend, which
consumes the CSS design-token system in `DESIGN.md` directly.

---

## Quick start (desktop)

Prerequisites: [Rust](https://rustup.rs) (stable), Node.js 20 or newer (22
recommended), and the
[Tauri 2 system dependencies](https://v2.tauri.app/start/prerequisites/) for
your OS. On Linux that is WebKitGTK 4.1, GTK 3, libsoup 3, and friends; the
exact `apt-get` list is in `.github/workflows/ci.yml`.

```sh
git clone https://github.com/serplay/anidoku.git
cd anidoku
npm ci
npm run tauri dev          # builds the Rust core, starts Vite on :1420, opens the app
```

Release bundle for the current OS:

```sh
npm run tauri build        # output under src-tauri/target/release/bundle/
```

Running only the frontend (`npm run dev`) opens the UI in a browser, but every
feature that needs the Rust backend shows a "desktop app only" hint. Use it for
styling work; use `tauri dev` for everything else.

---

## AniList sign-in

AniList requires each app to register its own OAuth client, so the client ID
is entered by you rather than shipped in the binary.

1. Go to [anilist.co/settings/developer](https://anilist.co/settings/developer)
   and create a client.
2. Set its **Redirect URL** to exactly `http://127.0.0.1:8737/callback`.
3. In AniDoku, open **Settings → AniList account**, paste the client ID, save,
   and press **Sign in to AniList**.

Desktop and Android open the system browser and capture the redirect on a
loopback listener. iOS shows the AniList page inside the app's own webview
because Safari would suspend the listener. Tokens last about a year and
AniList offers no refresh flow, so you will be asked to sign in again when it
expires.

---

## Mobile builds

Both mobile targets are scaffolded and committed. Follow the dedicated guides:

- **Android:** [`BUILD-ANDROID.md`](BUILD-ANDROID.md). Debug APK for an
  emulator or USB device, signed release APK, icon, and the Android-specific
  pieces (foreground service, cleartext loopback exemption, `stream://`
  rewrite, rustls instead of OpenSSL for cross-compilation).
- **iOS:** [`BUILD-IOS.md`](BUILD-IOS.md). Simulator run, device build via
  Xcode free provisioning or an ad-hoc IPA for AltStore/SideStore, ATS
  loopback exemption, the Swift background-task shim, and the known
  `FORCE_COLOR` build failure.

Store distribution is intentionally not a goal (see [Legal notes](#legal-notes)).

---

## Repository layout

```
.
├── core/                         anidoku-core: the shared Rust crate
│   ├── src/provider/allanime/    scraper: constants, runtime config, crypto, parsing
│   ├── src/downloads/            queue + HLS/MP4 engines with resume and backoff
│   ├── src/anilist/  src/sync/   AniList client, matcher, conflict resolver
│   ├── src/db/                   SQLite schema + migrations
│   ├── src/media_server.rs       loopback server for offline playback
│   ├── src/proxy.rs              stream proxy that replays CDN referers
│   ├── src/airing.rs  subs.rs    airing tracker planning, subtitle conversion
│   └── tests/allanime_live.rs    #[ignore]d live network test (the health check)
├── src-tauri/                    Tauri shell: commands, auth, platform shims
│   ├── src/commands.rs           every IPC command the UI calls
│   ├── src/auth.rs               AniList OAuth loopback capture
│   ├── src/android.rs  ios.rs    foreground service / background task glue
│   └── gen/apple, gen/android    committed mobile projects
├── src/                          SvelteKit frontend (static adapter, SPA)
│   ├── lib/api.ts                typed wrappers over invoke()
│   ├── lib/components/           Button, AnimeCard, ProviderOutage, ...
│   └── routes/                   home, search, anime/[id], watch/[id]/[ep], library,
│                                 downloads, inbox, settings
├── tests/e2e/                    Playwright specs + the Tauri IPC mock
├── scripts/allanime-oracle/      derives provider rotation values from the live site
├── allanime-config.json          the published remote provider config (see below)
├── .github/workflows/            ci.yml, provider-health.yml
├── ARCHITECTURE.md               design rationale, data model, milestones
├── DESIGN.md                     CSS design-token system the UI consumes
├── BUILD-ANDROID.md  BUILD-IOS.md
└── PLAN.md                       pending work and milestone notes
```

---

## Development

### Quality gates

All of these run in CI and should stay green locally before a push:

```sh
cargo fmt --all -- --check
cargo clippy -p anidoku-core --all-targets -- -D warnings
cargo test -p anidoku-core                 # unit + integration tests, no network
cargo check --workspace --all-targets      # includes the Tauri shell
npm run check                              # svelte-check (types + templates)
npm run test:e2e                           # Playwright, see below
```

The live provider test is `#[ignore]`d because it hits the network. Run it
explicitly when touching the provider:

```sh
cargo test -p anidoku-core --test allanime_live -- --ignored --nocapture
```

### Playwright end-to-end tests

The E2E suite drives the real SvelteKit frontend in Chromium with the **Tauri
IPC layer mocked**, so it needs no Rust backend and runs in a plain Ubuntu
container. `tests/e2e/tauri-mock.ts` installs a fake `window.__TAURI_INTERNALS__`
before any page script runs; every `invoke()` and event subscription resolves
against canned data. Tests override only the commands they care about:

```ts
await mockTauri(page, {
  get_sources: sequence(reject('PROVIDER_ROTATED: ...'), [sourceFixture()]),
  refresh_provider_config: { changed: true, build_id: '167', config_source: 'remote' }
});
```

`reject(msg)` makes a command fail; `sequence(a, b, ...)` answers successive
calls differently. Recorded calls are available as `window.__IPC_CALLS__` for
assertions on what the frontend requested.

Specs:

| File | Covers |
|---|---|
| `smoke.spec.ts` | app boots under the mock, navigation |
| `watch.spec.ts` | no-sources empty state, quality chips, playback error surfacing |
| `provider-outage.spec.ts` | provider-rotation state, "Check for fix" success and no-op, retry, non-rotation errors keep the plain path, Settings provider card |

```sh
npx playwright install chromium      # once
npm run test:e2e                     # headless
npm run test:e2e:ui                  # Playwright UI mode
```

The Vite dev server is started automatically on port 1420. If a stray server
is already bound there the run can hang; free the port with
`lsof -ti:1420 | xargs kill`.

### Continuous integration

`ci.yml` runs on every push and pull request:

| Job | What it checks |
|---|---|
| `core` | rustfmt, clippy with warnings denied, the full core test suite |
| `frontend` | svelte-check |
| `e2e` | the Playwright suite, uploading the HTML report as an artifact |
| `oracle-smoke` | the rotation oracle still parses, and `allanime-config.json` is in lockstep with the baked-in constants |
| `tauri-check` | `cargo check` of the whole workspace with the Linux webkit deps |

`provider-health.yml` is the scheduled provider watchdog described next.

---

## The streaming provider and how it stays alive

The only provider today is allanime, reached through its `mkissa.to` web
frontend and `api.mkissa.net` GraphQL API. It actively rotates anti-scraping
defenses every few weeks: a client build id, an AES key mask, a signed
bootstrap header, epoch buckets, persisted-query hashes, and occasionally the
shape of the signature itself. Each rotation is a total streaming outage until
re-ported, so most of the engineering around the provider is about making that
cheap and automatic.

### What a rotation looks like

The watch page shows **"Streaming source changed"** with **Check for fix** and
**Retry** buttons instead of an error string. Under the hood the backend tags
the failure with `PROVIDER_ROTATED:` — typically
`sources: bootstrap rejected epoch N (404 Not Found)` for an unknown build id,
or a `403` when the boot signature or epoch bucket is wrong. Network problems
keep the ordinary error path so the two are never confused.

### Self-healing pipeline

Three layers, in the order they act:

1. **Runtime remote config.** Every volatile value (build id, mask, hosts,
   lane, epoch bucket, boot label, boot signature template, aaReq seed
   template) is a field of `AllAnimeConfig`. The app starts from the baked-in
   defaults, then fetches `allanime-config.json` from this repo's `master`
   branch at startup and again whenever a sources fetch fails. **Check for
   fix** forces that fetch. Merging a config change to `master` therefore *is*
   the deployment.

2. **The oracle** (`scripts/allanime-oracle/`, `npm run oracle`). Instead of
   deobfuscating a bundle whose identifiers change every build, it opens the
   real web client in headless Chromium with `SubtleCrypto` hooked and
   captures what the client actually does: the HMAC key it imports is the
   mask, the strings it signs are the boot label and the signature template,
   and the bootstrap request carries the build id and lane. It writes the
   config only after recomputing, in Node, the exact `x-aa-boot` header the
   client sent. Anything that no longer fits the known shapes exits with
   "scheme drift" for a human. `npm run oracle:apply` mirrors the JSON into
   `constants.rs`; a unit test and the `oracle-smoke` CI job keep the two in
   lockstep.

3. **`provider-health.yml`** (every 6 hours, or on demand):
   - `live` runs the live sources test with the baked-in config **and** with
     the published config, classifies a failure as rotation, transient,
     remote-config or unknown, and retries a transient once.
   - `auto-port` runs on a rotation: oracle, apply, fmt/clippy/tests, live test
     again, then a `bot/allanime-rotation-<id>` pull request that auto-merges
     once green.
   - `report` keeps a single tracking issue whose status block is edited in
     place, comments only when the state changes, labels `needs-human` when
     automation could not fix it, closes the issue with a "Recovered" note
     when the check passes again, and nudges any provider fix PR that has sat
     open for more than a day.

   Dispatching the workflow with `simulate_rotation=true` is a drill: it forces
   the rotation path with a bogus build id so the oracle and gates are
   exercised on a GitHub runner without waiting for allanime. Drills never
   touch the tracking issue.

### Fixing a rotation by hand

When the pipeline labels an issue `needs-human`:

```sh
npm run oracle -- --dry-run     # what does the live client do today?
```

- **Exit 2 (scheme drift):** the captured signature no longer tokenises into
  the known fields plus one key group, or the HMAC chain changed. Read the
  `FAIL:` line, extend `derive()` and the config templates, add a unit test
  with the captured vector (see `aa_boot_matches_live_client_2026_09` in
  `decrypt.rs`).
- **Exit 3 (unreachable):** the site blocked or changed its show page. Try
  `--headed`, a different `--show`, or a different `--site` mirror.
- **Oracle agrees but the live test still fails:** the change is on the
  episode query side (aaReq payload or nonce seed, query text, response
  envelope). Hook `SubtleCrypto.encrypt` on an episode page to capture the
  plaintext payload and IV, then port it.

Always finish with the live test and bump `allanime-config.json` and
`constants.rs` together. The full history of past rotations, with what changed
each time, is the doc-comment at the top of
`core/src/provider/allanime/constants.rs`.

---

## Configuration reference

| Setting | Where | Meaning |
|---|---|---|
| AniList client ID | Settings → AniList account | Your registered OAuth client. Redirect URL must be `http://127.0.0.1:8737/callback`. |
| Airing notifications | Settings → Notifications | Toggle the airing tracker's new-episode notifications. |
| Provider build / config source | Settings → Streaming provider | Shows the current build id and whether the remote (self-heal) config has been applied; button forces a refresh. |
| `ANIDOKU_ALLANIME_CONFIG_URL` | environment variable | Overrides the remote config URL for one run. Handy for testing a candidate config before publishing it. Set to an empty string to disable remote config entirely. |
| `REMOTE_CONFIG_URL` | `core/src/provider/allanime/constants.rs` | Baked-in remote config URL. Points at `allanime-config.json` on `master`. |
| `autoNext` | browser `localStorage` | Autoplay the next episode; toggled in the player. |

Application data (SQLite database, AniList token, downloads) lives in the
platform's app-data directory, for example
`~/Library/Application Support/AniDoku` on macOS. On Android it is under
`files/`, not `cache/`, so the system does not evict it.

---

## Troubleshooting

**"Streaming source changed" on every episode.** The provider rotated. Press
**Check for fix**; if nothing is published yet, check the open
`provider-health` issue on GitHub for the pipeline's status.

**"No sources available" on one episode only.** The provider genuinely has no
hosts for it. Try another episode or switch sub/dub. This is not a rotation.

**A source plays black or errors, then another one starts.** Expected. Some
hosts (streamsb, streamlare, region-locked ok.ru) cannot be extracted; the
player skips them.

**AniList sign-in never completes.** Confirm the redirect URL on the AniList
client matches exactly, and that nothing else is bound to port 8737. On iOS,
re-tapping Sign in aborts the previous capture and retries the bind.

**Playwright hangs on startup.** A stale Vite server holds port 1420. Kill it
(see above).

**Android build fails linking OpenSSL.** The workspace pins reqwest to rustls
for this reason; do not re-enable `native-tls` features.

**iOS: "Arch specified by Xcode was invalid".** npm's `FORCE_COLOR` leaked into
xcodebuild. `BUILD-IOS.md` documents the guard in `project.yml`.

---

## Roadmap and known gaps

Detailed notes live in [`PLAN.md`](PLAN.md). Headlines:

- **iOS on a physical device** is unverified: sideload, background-download
  behaviour (the simulator does not model suspension), and an end-to-end
  native-HLS stream on hardware.
- **Android on a physical device** and a signed release APK are pending a
  keystore.
- **Second provider.** Everything sits behind the `Provider` trait; adding one
  is additive but not started.
- **Deeper E2E coverage:** search-results grid to detail navigation, and the
  player's auto-advance across several failing sources.

---

## Legal notes

- **AniList** allows client apps like this under its terms. The app respects
  the 90 requests/minute limit and uses the documented OAuth flow.
- **The streaming provider aggregates unlicensed streams.** This app therefore
  facilitates access to content its rights holders did not authorise. That is
  why it is distributed as a sideloaded build rather than through app stores,
  carries no monetisation, and keeps provider endpoints in a single updatable
  module. Use it in accordance with the law where you live.
- `DESIGN.md` is a token system derived from a third party's design language.
  The structure (spacing, radii, type scale) is used as-is; treat the signature
  colour pairing as replaceable.

---

## Further reading

- [`ARCHITECTURE.md`](ARCHITECTURE.md): framework choice, data model, sync
  conflict resolution, milestone history.
- [`DESIGN.md`](DESIGN.md): the CSS design tokens the UI is built from.
- [`core/src/provider/allanime/constants.rs`](core/src/provider/allanime/constants.rs):
  the provider's rotation history and every constant that has ever moved.
- [`scripts/allanime-oracle/README.md`](scripts/allanime-oracle/README.md):
  the oracle in one page.
- [ani-cli](https://github.com/pystardust/ani-cli), whose provider flow this
  app ports. Thanks to its maintainers for years of keeping it working.
