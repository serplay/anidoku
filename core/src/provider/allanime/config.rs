//! Runtime-overridable allanime constants — the remote "kill-switch" for
//! provider rotations.
//!
//! allanime rotates its anti-scraping crypto every few weeks (see
//! [`constants`](super::constants)'s doc-comment for the history). Baked-in
//! constants mean every rotation is a full streaming outage until a new binary
//! ships — brutal for a **sideloaded** app whose users can't easily update.
//!
//! [`AllAnimeConfig`] decouples the volatile values from the binary: the
//! provider starts from the baked-in defaults (so it always works offline and
//! on a fresh install) and, at startup and on the self-heal path, pulls an
//! optional JSON override from [`REMOTE_CONFIG_URL`](super::REMOTE_CONFIG_URL).
//! When allanime rotates you publish a new JSON (only the changed fields —
//! everything is `#[serde(default)]`, so a one-line `{"build_id":"76",
//! "qd_mask_hex":"…"}` is a valid config) and every installed app self-heals on
//! the next play attempt. No release required.
//!
//! Anything genuinely structural (the user agent, the HMAC label, the GraphQL
//! query bodies) stays a `const` — those change so rarely that a rebuild is the
//! right tool, and keeping them out of the config keeps the override surface
//! small.

use super::constants::*;
use serde::Deserialize;

/// The set of allanime values that have rotated (or plausibly will) and are
/// therefore overridable at runtime. Every field defaults to its baked-in
/// [`constants`](super::constants) value, so a remote override need only carry
/// the fields that actually changed.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "snake_case")]
pub struct AllAnimeConfig {
    /// Client build id (`x-build-id`, aaReq/x-aa-boot payloads). Rotates most.
    pub build_id: String,
    /// GraphQL API endpoint.
    pub api_url: String,
    /// Per-epoch key bootstrap endpoint (base; query built at call time).
    pub bootstrap_url: String,
    /// HTTP `Referer`/`Origin` origin the API and embed hosts expect.
    pub referer: String,
    /// Referer *host* baked into the `x-aa-boot` signature (not the origin).
    pub referer_host: String,
    /// Host serving the `/apivtwo/clock.json` embeds.
    pub base_host: String,
    /// Key group for the `x-aa-boot` signature.
    pub key_group: String,
    /// Content lane for the `episode(...)` source query (`?k=` / signatures).
    pub episode_lane: String,
    /// AES-256 key mask (XORed with bootstrap `partB`; HMAC-keyed for x-aa-boot).
    pub qd_mask_hex: String,
    /// Persisted-query hash for the episode-sources query.
    pub episode_query_hash: String,
    /// Bucket (ms) the `x-aa-boot` epoch is floored to.
    pub epoch_bucket_ms: u128,
}

impl Default for AllAnimeConfig {
    fn default() -> Self {
        Self {
            build_id: BUILD_ID.to_string(),
            api_url: API_URL.to_string(),
            bootstrap_url: BOOTSTRAP_URL.to_string(),
            referer: REFERER.to_string(),
            referer_host: REFERER_HOST.to_string(),
            base_host: BASE_HOST.to_string(),
            key_group: KEY_GROUP.to_string(),
            episode_lane: EPISODE_LANE.to_string(),
            qd_mask_hex: QD_MASK_HEX.to_string(),
            episode_query_hash: EPISODE_QUERY_HASH.to_string(),
            epoch_bucket_ms: EPOCH_BUCKET_MS,
        }
    }
}

impl AllAnimeConfig {
    /// Parse a remote override, falling back to baked-in defaults for any field
    /// the JSON omits. Rejects a config whose critical fields are structurally
    /// implausible (empty, or a mask that isn't 32 bytes of hex) so a corrupt or
    /// truncated fetch can never *disable* streaming — the caller keeps whatever
    /// config it already had.
    pub fn parse_validated(json: &str) -> Result<Self, String> {
        let cfg: Self =
            serde_json::from_str(json).map_err(|e| format!("bad allanime config json: {e}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        if self.build_id.is_empty()
            || self.api_url.is_empty()
            || self.bootstrap_url.is_empty()
            || self.episode_lane.is_empty()
            || self.episode_query_hash.is_empty()
        {
            return Err("allanime config has an empty required field".into());
        }
        // Mask must decode to exactly 32 bytes or key derivation can't work.
        match hex::decode(&self.qd_mask_hex) {
            Ok(b) if b.len() == 32 => {}
            _ => return Err("allanime config qd_mask_hex is not 32 bytes of hex".into()),
        }
        if self.epoch_bucket_ms == 0 {
            return Err("allanime config epoch_bucket_ms is zero".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_constants() {
        let d = AllAnimeConfig::default();
        assert_eq!(d.build_id, BUILD_ID);
        assert_eq!(d.qd_mask_hex, QD_MASK_HEX);
        assert_eq!(d.epoch_bucket_ms, EPOCH_BUCKET_MS);
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        // A realistic "buildId rotated" config carries only the two changed
        // fields; everything else must fall back to the baked-in defaults.
        let json = r#"{
            "build_id": "76",
            "qd_mask_hex": "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
        }"#;
        let cfg = AllAnimeConfig::parse_validated(json).unwrap();
        assert_eq!(cfg.build_id, "76");
        assert_eq!(
            cfg.qd_mask_hex,
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
        );
        // Untouched fields keep their defaults.
        assert_eq!(cfg.api_url, API_URL);
        assert_eq!(cfg.episode_lane, EPISODE_LANE);
        assert_eq!(cfg.epoch_bucket_ms, EPOCH_BUCKET_MS);
    }

    #[test]
    fn rejects_bad_mask_and_empty_fields() {
        // Too-short mask.
        assert!(AllAnimeConfig::parse_validated(r#"{"qd_mask_hex":"abcd"}"#).is_err());
        // Explicitly-empty required field.
        assert!(AllAnimeConfig::parse_validated(r#"{"build_id":""}"#).is_err());
        // Non-hex mask.
        assert!(AllAnimeConfig::parse_validated(
            r#"{"qd_mask_hex":"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"}"#
        )
        .is_err());
    }

    #[test]
    fn empty_json_is_all_defaults() {
        let cfg = AllAnimeConfig::parse_validated("{}").unwrap();
        assert_eq!(cfg, AllAnimeConfig::default());
    }

    /// The `allanime-config.json` shipped in the repo root is the ready-to-host
    /// starting point; it must parse and equal the baked-in defaults so hosting
    /// it as-is is a no-op. This also fails loudly if the constants rotate but
    /// the sample file isn't updated to match.
    #[test]
    fn shipped_sample_matches_defaults() {
        let json = include_str!("../../../../allanime-config.json");
        let cfg =
            AllAnimeConfig::parse_validated(json).expect("shipped allanime-config.json must parse");
        assert_eq!(
            cfg,
            AllAnimeConfig::default(),
            "allanime-config.json drifted from the baked-in constants"
        );
    }
}
