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
//! is `Fh(buildId)` over the embedded `ad` array — decode by resolving the
//! rotated string arrays) and will rotate again; this file is where to re-point
//! them.

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
/// x-aa-boot tokens and the bootstrap request. Baked into the web client (`wf`
/// in the crypto bundle: `(Rn(..)+mr(..)) !== "string" ? "75" : ""`).
pub const BUILD_ID: &str = "75";

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
/// HMAC-keyed for `x-aa-boot`. The web client computes it as `Fh(buildId)` over
/// an embedded `ad` byte-array, so it rotates with `BUILD_ID`.
pub const QD_MASK_HEX: &str = "ff65f1ba05d2556424dfec9f38f816e0a7d284a951845c865a609cb83bee7690";

/// Persisted-query hash for the episode embed query (ani-cli `$query_hash`).
pub const EPISODE_QUERY_HASH: &str =
    "d405d0edd690624b66baba3068e0edc3ac90f1597d898a1ec8db4e5c43c00fec";

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
