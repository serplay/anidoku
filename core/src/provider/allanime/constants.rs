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
//! wrong. `BUILD_ID`, `QD_MASK_HEX`, `REFERER`, and the hosts are all baked
//! into the web client (`window.__aaCrypto` + the `chunks/*.js` bundle) and
//! will rotate again; this file is where to re-point them.

/// Browser user agent sent with every request (ani-cli `$agent`).
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:150.0) Gecko/20100101 Firefox/150.0";

/// Referer/Origin the API and embed hosts expect (web client origin).
pub const REFERER: &str = "https://mkissa.to";

/// Host serving the embed/clock endpoints. Still `allanime.day` — only the
/// GraphQL API moved to mkissa; `/apivtwo/clock.json` stays here.
pub const BASE_HOST: &str = "allanime.day";

/// GraphQL API endpoint (moved off the Cloudflare-walled api.allanime.day).
pub const API_URL: &str = "https://api.mkissa.net/api";

/// Client build id, sent as the `x-build-id` header and used inside the aaReq
/// token / bootstrap request. Baked into the web client (`kr` in the bundle).
pub const BUILD_ID: &str = "63";

/// Per-epoch key bootstrap endpoint. Returns `{"epoch":<int>,"partB":<b64>,..}`;
/// the AES key is `base64(partB) XOR QD_MASK`.
pub const BOOTSTRAP_URL: &str = "https://api.mkissa.net/client-crypto/v1/bootstrap?buildId=63";

/// Static mask XORed with the bootstrap `partB` to derive the AES-256 key
/// (`qd` in the web client bundle).
pub const QD_MASK_HEX: &str =
    "a39b86dbbcf57f884f3e9074969e7fe26656c74012e4545605896621ffa441c1";

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
