//! Every allanime-specific constant lives here. When the site changes (it
//! will — ani-cli's history is a stream of such fixes), this file is the
//! blast radius.
//!
//! Originally ported from ani-cli 4.14.1 (https://github.com/pystardust/ani-cli).
//!
//! In mid-2026 allanime rotated its scraping defenses (issue #1793/#1806 in
//! ani-cli): the GraphQL API moved to `api.mkissa.net` behind the `mkissa.to`
//! frontend, and the `episode(...)` source query now requires an `aaReq`
//! AES-256-GCM token in `extensions`, keyed by a per-epoch secret fetched from
//! a bootstrap endpoint. `AA_CRYPTO_MISSING`/`AA_CRYPTO_STALE` errors and the
//! downstream "sources: missing episode.sourceUrls" come from getting this
//! wrong.
//!
//! **2026-07-30 rotation (buildId 63 -> 75, "lane" scheme).** The bootstrap
//! endpoint stopped accepting a bare `?buildId=63` (400 `missing_or_invalid_lane`).
//! It now requires a per-content-type *lane* and a signed `x-aa-boot` header:
//!   - `GET /client-crypto/v1/bootstrap?buildId=75&k=<lane>` with headers
//!     `x-build-id: 75` and `x-aa-boot: <token>`. `<lane>` is `k7` for the
//!     `episode(...)` query (the web client's `kf()` maps `episode(`->`k7`,
//!     `chapterPages(`->`k9`, `music(`->`k2`).
//!   - `x-aa-boot = hex(HMAC(HMAC(mask, "aa-boot:75"), sig))` where
//!     `sig = "75:mkissa:mkissa.to:<epoch>:k7"` (`buildId:keyGroup:refererHost:
//!     epoch:lane`), `mask` = [`QD_MASK_HEX`]. `epoch = floor(now_ms / 3days)`;
//!     the bootstrap response echoes the authoritative epoch used for the aaReq.
//!   - The aaReq payload gained a `"k":"<lane>"` field, its nonce seed gained a
//!     trailing `:<lane>`, and the query `extensions` gained a `"k":"<lane>"`.
//!
//! `BUILD_ID`, `QD_MASK_HEX`, `EPISODE_LANE`, `KEY_GROUP`, `REFERER`, and the
//! hosts are all baked into the web client (the crypto `chunks/*.js`; the mask
//! is `Fh(buildId)`/`ev(buildId)` over the embedded `ad` array — decode by
//! resolving the rotated string arrays) and will rotate again; this file is
//! where to re-point them.
//!
//! **2026-08-02 rotation (buildId 75 -> 81 + APQ hash rotation).** Bootstrap
//! started 404ing `{"error":"unknown_build_id"}` for buildId 75; the new build
//! is `81` with a fresh `QD_MASK_HEX`. This rotation *also* rotated the
//! persisted-query hash (the old `d405d0…` began returning
//! `PersistedQueryNotFound`). Rather than chase that hash, we now **define the
//! `episode` query ourselves ([`EPISODE_SOURCES_GQL`]) and POST it with a
//! self-computed `sha256(query)` hash** (standard Apollo APQ client
//! registration) — the aaReq binds `qh` to that same hash, so query/hash/aaReq
//! always agree and a future persisted-hash rotation no longer breaks us. Only
//! buildId/mask/hosts still need tracking. (`show{ _id }` in the query is
//! required — `sourceUrls` alone trips a server resolver bug.)

/// URL of the optional remote config JSON that overrides the rotatable
/// constants below (see [`AllAnimeConfig`](super::config::AllAnimeConfig)). This
/// is the release-free kill-switch for provider rotations: publish updated
/// `build_id`/`qd_mask_hex`/hosts here and installed apps self-heal on the next
/// play attempt.
///
/// **Empty by default = disabled** (no network call, pure baked-in behaviour).
/// Set it to a raw-hosted JSON you control — e.g. a GitHub raw URL like
/// `https://raw.githubusercontent.com/<you>/AniDoku/main/allanime-config.json`,
/// or a gist's raw URL. `allanime-config.json` in the repo root is a ready-made
/// starting point mirroring the current defaults. A per-run override is also
/// read from the `ANIDOKU_ALLANIME_CONFIG_URL` env var (handy on desktop).
pub const REMOTE_CONFIG_URL: &str = "";

/// Browser user agent sent with every request (ani-cli `$agent`).
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:150.0) Gecko/20100101 Firefox/150.0";

/// Referer/Origin the API and embed hosts expect (web client origin).
pub const REFERER: &str = "https://mkissa.to";

/// Host serving the embed/clock endpoints. Still `allanime.day` — only the
/// GraphQL API moved to mkissa; `/apivtwo/clock.json` stays here.
pub const BASE_HOST: &str = "allanime.day";

/// Referer the mp4upload CDN gates its direct `/d/…/video.mp4` links on. The
/// embed page's own host (`mp4upload.com`) 403s the file; only the `www.`
/// origin is accepted. Used both to fetch the embed page and as the `referer`
/// carried on the extracted [`VideoSource`] so the media proxy replays it.
pub const MP4UPLOAD_REFERER: &str = "https://www.mp4upload.com/";

/// GraphQL API endpoint (moved off the Cloudflare-walled api.allanime.day).
pub const API_URL: &str = "https://api.mkissa.net/api";

/// Client build id, sent as the `x-build-id` header and used inside the aaReq /
/// x-aa-boot tokens and the bootstrap request. Baked into the web client (the
/// crypto bundle: `Cf = (Ur(..)+Ur(..)) !== "string" ? "81" : ""`). Rotates
/// often — 63 (mid-Jul), 75 (2026-07-30), 81 (2026-08-02).
pub const BUILD_ID: &str = "81";

/// Per-epoch key bootstrap endpoint (base; the `?buildId=&k=<lane>` query is
/// built at call time). Returns `{"epoch":<int>,"partB":<b64>,"switchAt":..}`;
/// the AES key is `base64(partB) XOR QD_MASK`. Requires the signed `x-aa-boot`
/// header (see [`AA_BOOT_PREFIX`], [`KEY_GROUP`], [`REFERER_HOST`]).
pub const BOOTSTRAP_URL: &str = "https://api.mkissa.net/client-crypto/v1/bootstrap";

/// Content lane for the `episode(...)` source query (web client `If`). Sent as
/// the bootstrap `k=` query param, the aaReq/x-aa-boot signatures, and the
/// query `extensions.k`. (`chapterPages`->`k9`, `music`->`k2`; we only scrape
/// anime episodes, so `k7`.)
pub const EPISODE_LANE: &str = "k7";

/// Key group for the `x-aa-boot` signature (web client `zS(hostname)` maps the
/// `mkissa.to` origin to `"mkissa"`; mirrors like `youtu-chan.com` map to
/// `"mirror"`).
pub const KEY_GROUP: &str = "mkissa";

/// Referer *host* (not origin) baked into the `x-aa-boot` signature — the web
/// client uses `window.location.hostname` lowercased with a leading `www.`
/// stripped. Distinct from [`REFERER`], which is the full origin used as the
/// HTTP `Referer`/`Origin` header value.
pub const REFERER_HOST: &str = "mkissa.to";

/// HMAC label prefixing the buildId to derive the `x-aa-boot` inner key
/// (`bg(mask, "aa-boot:" + buildId)` in the web client's `PS`).
pub const AA_BOOT_PREFIX: &str = "aa-boot:";

/// Bucket (ms) the `x-aa-boot` epoch is floored to (web client `qh = 2592e5`,
/// i.e. 3 days). `epoch = floor(now_ms / EPOCH_BUCKET_MS)`.
pub const EPOCH_BUCKET_MS: u128 = 259_200_000;

/// Static mask XORed with the bootstrap `partB` to derive the AES-256 key, and
/// HMAC-keyed for `x-aa-boot`. The web client computes it as `ev(buildId)` (was
/// `Fh`) over an embedded `ad` byte-array, so it rotates with `BUILD_ID`.
pub const QD_MASK_HEX: &str = "1c51425b45d71a76c58adb6b52fe3e766d615bb48a252327b7c74323ea37658b";

/// The `episode(...)` sources GraphQL query we send.
///
/// **We define this ourselves and compute its persisted-query hash at runtime**
/// (`sha256(query)`) rather than baking in a server-side hash — a deliberate
/// resilience choice. allanime rotates its persisted-query hashes along with the
/// build (buildId 81 started rejecting the old `d405d0…` hash with
/// `PersistedQueryNotFound`). Standard Apollo APQ lets a client register its own
/// query by POSTing the full text with a matching `sha256Hash`; the aaReq token
/// binds `qh` to that same self-computed hash, so query and hash can never drift
/// and a future hash rotation no longer breaks us.
///
/// The `show{ _id }` selection is load-bearing: requesting `sourceUrls` alone
/// trips a server resolver bug (`Cannot set properties of undefined (setting
/// 'countryOfOrigin')`). Keep a `show` sub-selection.
pub const EPISODE_SOURCES_GQL: &str = "query($showId:String!,$translationType:VaildTranslationTypeEnumType!,$episodeString:String!){episode(showId:$showId translationType:$translationType episodeString:$episodeString){sourceUrls show{_id}}}";

/// Search query. ani-cli only requests `_id name availableEpisodes`; we also
/// ask for `englishName` and `thumbnail` for the results grid (both are
/// long-standing fields on allanime's Show type, used by other clients).
pub const SEARCH_GQL: &str = "query( $search: SearchInput $limit: Int $page: Int \
$translationType: VaildTranslationTypeEnumType $countryOrigin: VaildCountryOriginEnumType ) { \
shows( search: $search limit: $limit page: $page translationType: $translationType \
countryOrigin: $countryOrigin ) { edges { _id name englishName aniListId thumbnail availableEpisodes __typename } }}";

/// Episode list query (ani-cli `episodes_list_gql`).
pub const EPISODES_LIST_GQL: &str =
    "query ($showId: String!) { show( _id: $showId ) { _id availableEpisodesDetail }}";
