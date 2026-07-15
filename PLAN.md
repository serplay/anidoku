# Plan

No pending work. The UI polish batch planned 2026-07-15 is fully implemented and
committed (one commit per item, all four gates green on each):

1. `2189662` — search page stray horizontal scrollbar fixed
2. `58b13df` — AnimeCard tooltip moved inside the cover, no more clipping
3. `ab85df8` — unreleased shows no longer resolve a stream; toast + Add to Planning
4. `a275304` — availability check + cache, "Not available" badge, "Only streamable" filter
5. `e502b27` — animated boot splash with the AniDoku logo

Deliberately skipped: item 5's optional Tauri `"visible": false` + `show()` polish
(avoids OS white flash on launch) — risky to ship unverified since the app wasn't
launched. Pick it up later if the white flash bothers in practice. Note: no live-app
run has verified this batch yet; types/builds/tests are clean but a manual smoke pass
of search, home rows, an unreleased show click, and the splash is worthwhile.
