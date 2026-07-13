//! Every allanime-specific constant lives here. When the site changes (it
//! will — ani-cli's history is a stream of such fixes), this file is the
//! blast radius.
//!
//! Ported from ani-cli 4.14.1 (https://github.com/pystardust/ani-cli).

/// Browser user agent sent with every request (ani-cli `$agent`).
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:150.0) Gecko/20100101 Firefox/150.0";

/// Referer allanime's API and embed hosts expect (ani-cli `$allanime_refr`).
pub const REFERER: &str = "https://youtu-chan.com";

/// Host serving the embed/clock endpoints (ani-cli `$allanime_base`).
pub const BASE_HOST: &str = "allanime.day";

/// GraphQL API endpoint (ani-cli `$allanime_api` + "/api").
pub const API_URL: &str = "https://api.allanime.day/api";

/// Passphrase whose SHA-256 digest is the AES-256-CTR key for the
/// "tobeparsed" episode-source payloads (ani-cli `$allanime_key`).
pub const DECRYPT_PASSPHRASE: &str = "Xot36i3lK3:v1";

/// Persisted-query hash for the episode embed query (ani-cli `$query_hash`).
pub const EPISODE_QUERY_HASH: &str =
    "d405d0edd690624b66baba3068e0edc3ac90f1597d898a1ec8db4e5c43c00fec";

/// Search query. ani-cli only requests `_id name availableEpisodes`; we also
/// ask for `englishName` and `thumbnail` for the results grid (both are
/// long-standing fields on allanime's Show type, used by other clients).
pub const SEARCH_GQL: &str = "query( $search: SearchInput $limit: Int $page: Int \
$translationType: VaildTranslationTypeEnumType $countryOrigin: VaildCountryOriginEnumType ) { \
shows( search: $search limit: $limit page: $page translationType: $translationType \
countryOrigin: $countryOrigin ) { edges { _id name englishName thumbnail availableEpisodes __typename } }}";

/// Episode list query (ani-cli `episodes_list_gql`).
pub const EPISODES_LIST_GQL: &str =
    "query ($showId: String!) { show( _id: $showId ) { _id availableEpisodesDetail }}";

/// Episode source-urls query (ani-cli `episode_embed_gql`), POST fallback for
/// when the persisted GET does not return an encrypted payload.
pub const EPISODE_EMBED_GQL: &str = "query ($showId: String!, $translationType: \
VaildTranslationTypeEnumType!, $episodeString: String!) { episode( showId: $showId \
translationType: $translationType episodeString: $episodeString ) { episodeString sourceUrls }}";
