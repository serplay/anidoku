# UI Polish Plan (next work batch)

User-reported issues + one feature, planned 2026-07-15. Work through in order; each item
is independently committable. Read DESIGN.md before styling. All four gates must stay
green: `cargo test --workspace`, `cargo check --workspace`, `npm run build`, `npm run check`.

## 1. Search page: stray horizontal scrollbar

**Symptom:** after searching/using filters, a horizontal scrollbar sometimes appears at
the bottom of the page. Per DESIGN.md, the page body must never scroll horizontally.

**Plan:** reproduce with filters open + many genre/tag chips selected + results loaded,
then audit `src/routes/search/+page.svelte` containers for unwrapped flex rows
(chips rows need `flex-wrap: wrap`), fixed widths wider than the viewport, and grid
`minmax` that can't shrink (`minmax(150px, 1fr)` is fine; check the filter panel's
selects/inputs and the tag picker dropdown for `width` overflows — give the panel
`max-width: 100%` and children `min-width: 0`). As a final backstop put
`overflow-x: clip` on the app's main content container in `src/routes/+layout.svelte`
(clip, not hidden — don't create another scroll container), but only after fixing the
actual offenders, not instead of.

## 2. Home page: full-title hover tooltip clipped

**Symptom:** the AnimeCard hover tooltip (full title) is cut off on home rows — it
renders below the card and gets clipped by the row's scroll container
(`HomeRow .scroller` is `overflow-x: auto; overflow-y: hidden` — the overflow-y:hidden
is REQUIRED, it stops rows from capturing vertical page scroll; do not revert it).

**Plan:** move the tooltip inside the card's cover instead of below the card:
in `src/lib/components/AnimeCard.svelte`, relocate the `.tooltip` div inside the
`.cover` element (which is `position: relative; overflow: hidden`), positioned
`left/right/bottom: 6px`, keeping the fade-in-on-hover (0.3s delay) and the
english + romaji two-line content. It then can never escape the card bounds, so no
scroll container can clip it — fixes home rows and search grid alike. Delete the
old outside-the-card positioning CSS (`top: 100%`, `min-width`, `width: max-content`).

## 3. Unreleased anime/episodes should not try to fetch

**Symptom:** clicking a not-yet-aired show (Upcoming Next Season row) or an upcoming
episode tries to resolve/stream and errors out.

**Plan:**
- Home "Upcoming Next Season" row + search results with `status == NOT_YET_RELEASED`:
  do NOT run provider resolution on click. Show a toast "Not released yet — airs
  <season year>" and (nice-to-have, cheap) offer/perform "Add to Planning" when the
  user is logged in (reuse `setListEntry(anilist_id, 'PLANNING', 0, null)` from
  `src/lib/api.ts`). Status is already available: `HomeMedia`/`CatalogMedia` carry (or
  can carry — verify, else add to the GraphQL query + parser + fixture) `status`.
- Inbox Upcoming rows (`src/routes/inbox/+page.svelte`): clicking must go to the show's
  DETAIL page (never a watch/episode URL for an unaired episode). Verify current
  behavior; the detail page naturally shows only episodes that exist on the provider.
- Detail page is already provider-backed so it can't list unaired episodes — no change.

## 4. Search results that have no streamable source ("Hell's Paradise" case)

**Symptom:** AniList-backed search shows catalog entries the provider doesn't have;
clicking one fails with only a toast. User wants clear info or hiding.

**Plan (badge + de-emphasize, not hide — entries are still useful for Planning):**
- Add a Tauri command `check_availability(anilist_id, title, episodes) -> bool` that
  runs the existing reverse-resolution (`resolve_provider_for_anilist` internals,
  src-tauri/src/commands.rs) WITHOUT navigating, and CACHE the outcome in a new
  `availability` table (anilist_id PK, available INTEGER, checked_at; TTL ~7 days for
  negative results, permanent for positive since a stored mapping already implies
  available). Negative caching matters — repeat searches must not re-hammer allanime.
- After search results render, check availability lazily: batch the visible results
  through the command with small concurrency (2-3 at a time) to be gentle on allanime;
  results stream back per-card (Svelte state map, like the detail page's download map).
- UI: unavailable cards get `opacity ~0.55` + a small "Not available" corner badge
  (muted, not red); clicking one shows the informative toast and offers Add to Planning
  (same affordance as item 3). Available cards unchanged. While unchecked, cards look
  normal (no flicker — only downgrade once a negative answer arrives).
- Do NOT hide by default. If trivially cheap, an "Only streamable" filter toggle in the
  filter panel that hides negatives client-side is a welcome extra.

## 5. Animated boot splash screen

**Feature:** an animated splash with the app logo on startup.

**Plan (in-webview overlay — no extra Tauri window needed):**
- Logo asset: use the existing app logo. Check `src/lib/assets/` (favicon.svg) and
  `src-tauri/icons/` for what the user considers "the logo they're using". If it's
  only the placeholder Tauri icon, ASK the user for their logo file before building
  this item — do not invent a new logo. (An `AniDoku` wordmark + the yellow play-glyph
  from `src/lib/assets/cover-placeholder.svg` is an acceptable fallback if they say so.)
- Implement as a full-viewport fixed overlay rendered by `src/routes/+layout.svelte`
  on first mount only (module-level flag so client-side navigation never re-shows it):
  canvas-dark background, centered logo, CSS animation (scale/fade or SVG
  stroke-draw, ~0.9s), then fade the overlay out (0.3s) and remove it from the DOM.
  Dismiss at `max(minimum 1.2s, first page ready)` — home already renders instantly
  from cache so a fixed ~1.2-1.5s total is fine and simpler than readiness plumbing.
- Match DESIGN.md: dark canvas #0b0e11, the single yellow accent is appropriate here
  (brand moment). Respect `prefers-reduced-motion` (skip animation, short fade).
- Optional polish if trivial: set the Tauri main window `"visible": false` in
  `tauri.conf.json` + `getCurrentWindow().show()` on frontend mount, so the OS window
  doesn't flash white before the webview paints. Test that dev-mode reload still works.

## Context for a fresh session

Repo: Tauri 2 + Svelte 5 anime app; architecture + milestone history in ARCHITECTURE.md
(M0–M3.5 built: streaming, AniList sync, downloads, home/airing/inbox, AniList search
with filters + card meta chips). Recent relevant commits: `2f7a799` (AniList catalog
search + meta chips), and the home-row horizontal-scroll fix. Search page:
`src/routes/search/+page.svelte`. Cards: `src/lib/components/AnimeCard.svelte`.
Rows: `src/lib/components/HomeRow.svelte`. Toasts: `pushToast` in
`src/lib/state.svelte.ts`, rendered by `+layout.svelte`.
Disk space is tight (~2.5 GB free) — avoid full release builds; `cargo check` + tests.
