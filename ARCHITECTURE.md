# AniDoku — Architecture & Build Roadmap

## Context

AniDoku is a greenfield cross-platform (desktop + Android + iOS) GUI app that reimplements ani-cli's streaming flow — search → episode list → provider/quality/subtitle selection → playback — with two things ani-cli can't do: AniList account sync and managed offline downloads. The repo currently contains only `DESIGN.md`, a Binance-derived design-token system (dark-first canvas `#0b0e11`, single yellow accent `#FCD535`, Inter/IBM Plex substitutes, flat color-block elevation). This document is the planning deliverable; no code yet.

Decisions already made with the user:
- **Platforms:** macOS/Windows/Linux + Android + iOS (iOS via sideload/AltStore — App Store is off the table for a scraper app anyway).
- **Playback:** embedded in-app player, not external mpv.
- **Providers:** native port of ani-cli's scraping logic (allanime flow), not shelling out to the binary.

---

## 1. Recommended tech stack

### Framework: **Tauri 2** (Rust core + webview UI) — with eyes open

| | Tauri 2 | Flutter | React Native | .NET MAUI |
|---|---|---|---|---|
| Desktop quality | Excellent (native webview, ~10 MB) | OK but non-native feel; Linux weakest | Poor/immature desktop | Windows-centric; Linux unsupported |
| Mobile maturity | Stable since v2 (Oct 2024) but youngest of the four; some plugins desktop-only | Excellent | Excellent (mobile), weak desktop | OK |
| Shared "business core" | **Rust crate shared 1:1 across all 5 targets** — scraping, downloads, DB, sync | Dart shared | JS shared; native modules per-platform | C# shared |
| Video playback | Webview player (hls.js / native HLS) or native plugin | media_kit (libmpv) — very strong | react-native-video | MediaElement |
| Fit with DESIGN.md | **Direct** — it's a CSS token system; webview UI consumes it natively | Requires re-expressing tokens in Flutter theming | Direct-ish (styles ≠ CSS) | Poor |

**Recommendation: Tauri 2.** The deciding factors:

1. The hard, breakage-prone logic (provider scraping, HLS download engine, SQLite state, AniList sync queue) lives in **one Rust crate compiled unchanged for all five platforms**. That's the part that needs to be rock-solid; the UI is comparatively easy.
2. `DESIGN.md` is literally a CSS-variable design system. A webview frontend consumes it as-is; Flutter would mean hand-porting every token and component spec.
3. Desktop is a first-class target here (it's ani-cli's home turf), and Tauri's desktop story beats Flutter's.
4. Video playback in a webview is genuinely fine for this app: **iOS WKWebView plays HLS natively**, and desktop/Android webviews support MSE so **hls.js/Vidstack** works.

**Honest tradeoffs (the case for Flutter):** Tauri mobile is production-usable but young — expect to write small Swift/Kotlin plugin shims for background downloads (iOS `URLSession` background sessions, Android foreground service) and OAuth deep-link handling; some community plugins are desktop-only. Flutter would give a more polished mobile baseline (and media_kit is a superb player) at the cost of a worse desktop app, a second implementation of the design system, and losing the shared Rust core. If mobile were the *primary* surface, Flutter would win; with desktop + mobile weighted equally and a Rust-shaped core, Tauri wins.

### Frontend: **Svelte 5 (+ SvelteKit in static/SPA mode) + TypeScript**

- Pairs with Tauri's lightweight ethos: no virtual DOM, tiny bundle, fast cold start in a webview — noticeable on mid-range Android.
- Svelte's scoped styles + CSS custom properties map 1:1 onto `DESIGN.md` tokens (`--color-primary: #fcd535`, `--rounded-md: 6px`, …); component specs like `markets-table-card`/`markets-row` translate directly into Svelte components (episode table, download rows).
- Ecosystem needs are covered: **Vidstack** (player UI) is framework-agnostic with Svelte support; virtual scrolling for 1000+ episode lists exists (`@tanstack/svelte-virtual`).
- React is the fallback if contributor familiarity matters more; Vue offers no specific advantage here.

**Supporting pieces:** `rusqlite` + migrations (SQLite is the single local store), `reqwest` (scraping + downloads), `tokio` (download concurrency), Tauri plugins: `deep-link` (OAuth callback), `stronghold` or OS keychain (token storage), custom `asset`/`localhost` protocol (serving downloaded media to the player).

---

## 2. High-level architecture

```
┌────────────────────────── UI (Svelte, webview) ──────────────────────────┐
│  Search / Detail / Player (Vidstack) / Library / Download Manager / Auth │
└────────────┬──────────────── Tauri IPC (commands + events) ─────────────┘
             │
┌────────────▼──────────────── Rust core (shared crate) ───────────────────┐
│                                                                           │
│  Provider Engine          Download Manager         AniList Sync Engine    │
│  - trait Provider         - job queue (SQLite-     - GraphQL client       │
│  - allanime impl            backed, resumable)     - OAuth token mgmt     │
│  - search/episodes/       - HLS segment fetcher    - outbound mutation    │
│    sources/decrypt          + local m3u8 rewrite     queue (offline-safe) │
│  - quality/sub tracks     - progress events → UI   - conflict resolver    │
│                                                                           │
│                    SQLite (single source of truth)                        │
│         anime · episodes · downloads · list_entries · sync_queue          │
│                                                                           │
│  Media Server: custom localhost/asset protocol serving downloaded         │
│  segments + rewritten playlists + converted VTT subs to the player        │
└───────────────────────────────────────────────────────────────────────────┘
   Platform shims (thin): Android foreground-service downloads,
   iOS background URLSession handoff, deep-link OAuth redirect
```

**Key flows:**

- **Streaming:** UI → `search(query)` → Provider Engine (allanime GraphQL, same flow ani-cli uses: search → episode list → source URLs → decrypt links) → returns sources with quality + subtitle tracks → UI feeds the chosen URL to Vidstack (hls.js on desktop/Android; native HLS on iOS). External subtitle files: user picks a file → Rust converts SRT/ASS → WebVTT → served via local protocol → added as a text track. Watch-progress events (e.g. crossed 85% of episode) → Sync Engine.
- **Downloads:** UI enqueues episode/range/season → rows in `downloads` → manager resolves source, fetches HLS segments concurrently with resume support (segment index = natural checkpoint), writes a rewritten local `.m3u8`, emits progress events. Playback of downloads goes through the same player via the local media protocol — online and offline playback share one code path.
- **Provider volatility containment:** everything behind a `Provider` trait (`search`, `episodes`, `sources`). When allanime changes (it will), only one module changes; adding a second provider later is additive.

## 3. Data model & sync conflict resolution

SQLite schema (essentials):

```sql
anime        (anilist_id PK, provider_id, title_romaji, title_english,
              cover_url, episode_count, format, cached_at)
episodes     (anime_id, number, title, PRIMARY KEY(anime_id, number))
downloads    (id PK, anime_id, episode_number, state, -- queued|downloading|paused|done|failed
              quality, bytes_total, bytes_done, segments_done, dir_path,
              error, created_at, updated_at)
list_entries (anilist_id PK, status, -- CURRENT|PLANNING|COMPLETED|DROPPED|PAUSED|REPEATING
              progress, score,
              local_updated_at, remote_updated_at, dirty INTEGER)
sync_queue   (id PK, anilist_id, mutation_json, queued_at, attempts)
watch_state  (anime_id, episode_number, position_secs, updated_at)  -- resume points
```

**Sync model: local-first with an outbound queue.**
- Every in-app action (episode crosses completion threshold, status change) writes `list_entries` locally, marks `dirty`, and appends to `sync_queue`. A worker drains the queue whenever online (AniList `SaveMediaListEntry` mutation), respecting the 90 req/min limit with batched/debounced flushes.
- On login and periodically, pull the remote list (`MediaListCollection`, which includes AniList's `updatedAt`).

**Conflict resolution (offline progress vs. remote state):**
1. **Progress:** `max(local.progress, remote.progress)` — watching is monotonic; watching E5 here and E7 on the website means you're at E7. Never regress progress automatically.
2. **Status:** last-writer-wins by timestamp (`local_updated_at` vs remote `updatedAt`), with one guard: if merged progress == episode_count, status promotes to COMPLETED regardless.
3. **Local dirty + remote unchanged since last pull:** push local, no conflict.
4. Silent auto-merge in all cases; a small "synced from AniList" toast when remote overwrote a local value, so the user is never confused. No conflict dialogs — this data isn't precious enough to interrupt for.

**AniList auth detail:** tokens live 1 year, **no refresh tokens** — store token + expiry in the OS keychain, surface a re-login prompt as expiry nears, and ensure the app degrades gracefully to "local-only tracking" when logged out or expired.

## 4. Milestones

**M0 — Foundations (scaffold):** Tauri 2 + SvelteKit workspace; Rust core crate skeleton; SQLite migrations; `DESIGN.md` tokens → CSS custom properties + base components (buttons, cards, table rows, inputs); dark-first theme with light transactional surfaces.

**M1 — Streaming MVP (desktop only):** allanime provider port (search, episodes, source resolution, link decrypt); detail page + episode list; Vidstack player with quality switching and provider subtitle tracks; external subtitle file loading (SRT/ASS→VTT); resume-position tracking. *Exit: parity with `ani-cli <query>` in a GUI.*

**M2 — AniList sync:** OAuth (deep-link/loopback flow), keychain token storage; library views per status; auto-progress on watch; outbound queue + conflict resolver; offline-tolerant.

**M3 — Downloads + manager (desktop):** HLS segment download engine (pause/resume/cancel, concurrency limits); enqueue single/range/season; manager view per DESIGN.md table conventions (progress, speed, storage per anime, bulk delete); offline playback via local media protocol; downloaded subtitle tracks.

**M3.5 — Home page + airing tracker & notification inbox:**

*Home page* (new `/` landing; search moves to `/search`; nav: Home · Search · Library · Downloads · Settings):
- **Continue Watching** row first (most useful): local data only — CURRENT list entries joined with watch_state; each card deep-links to the next unwatched episode. Works offline.
- **Trending Now**, **Popular This Season**, **Upcoming Next Season** rows: AniList public GraphQL (no auth) — `media(sort: TRENDING_DESC, status: RELEASING)`, `media(season, seasonYear, sort: POPULARITY_DESC)`, and next season's equivalent. One batched query per section, 12–20 items each.
- Layout per DESIGN.md: horizontally scrollable card rows (reuse AnimeCard + Skeleton), section heads in `title-lg`, dark canvas. Airing shows carry a small "Ep N in Xd" caption from `nextAiringEpisode`.
- Caching: `home_cache` table (section, json, fetched_at), 6h TTL — instant render on launch, offline-tolerant, kind to the 90 req/min budget.
- Card click → AniList id → provider id: reuse an existing mapping, else provider title-search matched by the aniListId the provider carries (sync matching, reversed). New command `resolve_provider_for_anilist`.

*Airing tracker + notification inbox:*
- **Data**: for list entries with status CURRENT/REPEATING (PLANNING via Settings toggle) whose media is RELEASING: `nextAiringEpisode { episode airingAt }`, fetched in ONE batched `media(id_in: [...])` query (50/page).
- **DB**: `airing (anilist_id PK, next_episode, airing_at, media_status, refreshed_at)`; `notifications (id PK, anilist_id, episode, airing_at, kind, created_at, read)`.
- **Worker** (same pattern as sync.rs): refresh airing on startup, after each list pull, and every 6h. A 60s ticker moves rows with `airing_at <= now` into `notifications`, emits `notify:new` (toast + badge) and fires an OS notification via `tauri-plugin-notification`. Airings missed while the app was closed notify once on startup. De-dupe on (anilist_id, episode).
- **Inbox UI**: bell in the top nav with unread badge → panel with **Upcoming** (per tracked show: cover, "Ep N airs <weekday, local time>" + countdown) and **New episodes** (fired, newest first; click → detail page or straight to the episode when a provider mapping exists); mark-all-read + clear.
- Lifecycle: `nextAiringEpisode == null` / media FINISHED ⇒ untrack (final episode still notifies); shows leaving Watching stop being tracked at next refresh.

**M4 — Android:** mobile-responsive UI pass (DESIGN.md already specifies breakpoints); foreground-service download shim; deep-link OAuth verified; playback via hls.js in Android WebView; APK distribution (direct/F-Droid-style — not Play Store).

**M5 — iOS:** native-HLS playback path in WKWebView; background `URLSession` download shim; sideload/AltStore distribution and signing story.

**v1 polish:** provider failover UX, download auto-cleanup rules ("keep last N watched"), library search/filters, error reporting for provider breakage, auto-update (desktop).

Sequencing rationale: streaming before sync (core value first), sync before downloads (downloads reuse source resolution but the manager is the biggest UI lift), Android before iOS (cheaper distribution loop; iOS inherits a working mobile UI).

## 5. Legal / ToS caveats

- **AniList API:** free for apps under $150/mo revenue; a *client* app like this is fine (the prohibition targets competing list-tracker *services*). Respect 90 req/min; OAuth tokens last 1 year with no refresh flow. ([Terms](https://docs.anilist.co/guide/terms-of-use), [rate limits](https://docs.anilist.co/guide/rate-limiting), [auth](https://docs.anilist.co/guide/auth/))
- **Provider scraping:** allanime & co. aggregate unlicensed streams — the app facilitates access to pirated content. Consequences to accept upfront: **no App Store / Play Store distribution** (hence sideload/APK plans above), possible DMCA pressure on distribution channels (GitHub releases have been hit for similar apps — ani-cli itself has survived, but mirrors matter), and zero monetization to avoid aggravating exposure. Keep provider endpoints in an updatable module rather than marketing them.
- **Provider stability is an ops burden, not a one-off:** allanime changes its API/obfuscation periodically; ani-cli's history is a stream of such fixes. The `Provider` trait isolation + a "provider broken, update available" UX path is load-bearing, not optional.
- **DESIGN.md provenance:** it's an extraction of Binance's design language. Using the token *structure* (spacing, radii, type scale) is fine; consider swapping the signature yellow `#FCD535` + near-black combination or at minimum not mimicking Binance's wordmark styling, so the app doesn't read as Binance trade-dress.

## Verification (for the planning deliverable)

This pass produces no code. The plan is validated by: (a) user sign-off on the stack recommendation and conflict-resolution policy above; (b) M1's first task doubling as a technical spike — if the allanime port and a Vidstack HLS stream work inside a Tauri webview on desktop within the first week of implementation, the stack bet is confirmed; if webview playback proves inadequate, the documented fallback is a native player plugin (libmpv on desktop, ExoPlayer/AVPlayer on mobile) behind the same UI.
