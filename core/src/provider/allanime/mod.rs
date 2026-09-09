//! allanime provider: a native port of ani-cli 4.14.1's scraping flow.
//!
//! Flow (matching ani-cli):
//!   search  -> POST GraphQL, parse shows.edges
//!   episodes-> POST GraphQL, parse availableEpisodesDetail
//!   sources -> GET persisted GraphQL (POST fallback) for sourceUrls, which
//!              may be wrapped in an encrypted `tobeparsed` blob; deobfuscate
//!              each sourceUrl into a /clock.json path, fetch it with the
//!              allanime referer, parse the links.

mod config;
mod constants;
mod decrypt;
mod parse;

use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::provider::Provider;
use crate::{Error, Result};
use async_trait::async_trait;
use config::AllAnimeConfig;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use constants::*;

/// Minimum spacing between forced remote-config refreshes, so a stream that
/// keeps failing (dead episode, offline) can't hammer the config host.
const REFRESH_THROTTLE: Duration = Duration::from_secs(30);

pub struct AllAnime {
    client: Client,
    /// Live, runtime-overridable copy of the volatile allanime constants. Starts
    /// from the baked-in defaults and is swapped in place by [`Self::refresh_config`].
    config: RwLock<AllAnimeConfig>,
    /// Where to pull remote config overrides from; empty = feature disabled.
    config_url: String,
    /// Last time a remote refresh actually hit the network (for throttling).
    last_refresh: RwLock<Option<Instant>>,
    /// Whether the live config came from the remote override (vs. baked-in).
    remote_applied: RwLock<bool>,
}

/// Snapshot of where the provider's volatile config currently comes from —
/// surfaced in the app's Settings so a user (or a bug report) can tell whether
/// the self-heal has kicked in.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderStatus {
    pub build_id: String,
    /// `"remote"` once a remote override has been applied, else `"baked"`.
    pub config_source: &'static str,
    /// The remote config URL in effect (empty = self-heal disabled).
    pub config_url: String,
}

impl Default for AllAnime {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-epoch crypto material for the `episode(...)` query, fetched from the
/// bootstrap endpoint. `epoch` rotates every few days; `partB` XORed with the
/// static [`QD_MASK_HEX`] yields the AES-256 key.
#[derive(serde::Deserialize)]
struct Bootstrap {
    epoch: u64,
    #[serde(rename = "partB")]
    part_b: String,
    /// Bucket the server used for `epoch` (self-describing since 2026-09). We
    /// can't use it *before* signing, but a mismatch with the config is the
    /// tell-tale of a bucket rotation, so it's logged loudly.
    #[serde(rename = "epochMs", default)]
    epoch_ms: Option<u64>,
}

/// Prefix on provider errors whose shape says "allanime rotated its scheme"
/// (as opposed to a network blip or a genuinely source-less episode). The
/// frontend keys its "streaming source changed — check for fix" state off it
/// and the health workflow uses it to classify a failure as auto-portable.
pub const ROTATED_PREFIX: &str = "PROVIDER_ROTATED: ";

/// Wrap a provider error as a rotation if its message matches a known
/// rotation signature. Idempotent.
fn classify_rotation(err: Error) -> Error {
    const SIGNATURES: [&str; 6] = [
        "bootstrap rejected",
        "bad bootstrap response",
        "missing episode.sourceUrls",
        "AA_CRYPTO",
        "tobeparsed",
        "PersistedQueryNotFound",
    ];
    match err {
        Error::Provider(msg) | Error::Decrypt(msg)
            if !msg.starts_with(ROTATED_PREFIX) && SIGNATURES.iter().any(|s| msg.contains(s)) =>
        {
            Error::Provider(format!("{ROTATED_PREFIX}{msg}"))
        }
        other => other,
    }
}

/// Client-computed epoch candidates for the `x-aa-boot` signature, mirroring the
/// web client's `[qS(), zh()]`: the current 3-day bucket (`zh`), plus the
/// previous one while still inside the new bucket's first day (`qS`, a grace
/// window for freshly rotated epochs). Deduped, tried in candidate order.
fn epoch_candidates(bucket_ms: u128, now_ms: u128) -> Vec<u64> {
    // web client `US = 864e5` (1 day)
    const GRACE_MS: u128 = 86_400_000;
    let zh = (now_ms / bucket_ms) as u64;
    let qs = if zh > 0 && now_ms - (zh as u128) * bucket_ms < GRACE_MS {
        zh - 1
    } else {
        zh
    };
    if qs == zh {
        vec![zh]
    } else {
        vec![qs, zh]
    }
}

impl AllAnime {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("reqwest client");
        // Env override wins over the baked-in const so desktop dev can point at
        // a test config without a rebuild.
        let config_url = std::env::var("ANIDOKU_ALLANIME_CONFIG_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| REMOTE_CONFIG_URL.to_string());
        Self {
            client,
            config: RwLock::new(AllAnimeConfig::default()),
            config_url,
            last_refresh: RwLock::new(None),
            remote_applied: RwLock::new(false),
        }
    }

    /// Test-only constructor pointing the remote-config fetch at an explicit URL
    /// (avoids mutating the process-global env var across parallel tests).
    #[cfg(test)]
    fn new_with_config_url(url: impl Into<String>) -> Self {
        let mut s = Self::new();
        s.config_url = url.into();
        s
    }

    /// A consistent snapshot of the current config for the duration of one
    /// request. Cloning up front means a concurrent [`Self::refresh_config`]
    /// can't change host/key material midway through a single sources fetch.
    fn config(&self) -> AllAnimeConfig {
        self.config.read().map(|c| c.clone()).unwrap_or_default()
    }

    /// Pull the remote config override and swap it in if it parses, validates,
    /// and differs from what we have. Returns `true` when the live config
    /// changed. Never errors out of band: any network/parse failure just leaves
    /// the current (baked-in or previously-fetched) config untouched, so a bad
    /// fetch can never take streaming down.
    ///
    /// `force` bypasses the [`REFRESH_THROTTLE`] spacing used by the self-heal
    /// retry path; startup passes `force` too (there's no prior refresh yet).
    pub async fn refresh_config(&self, force: bool) -> bool {
        if self.config_url.trim().is_empty() {
            return false;
        }
        if !force {
            if let Ok(last) = self.last_refresh.read() {
                if last.is_some_and(|t| t.elapsed() < REFRESH_THROTTLE) {
                    return false;
                }
            }
        }
        if let Ok(mut last) = self.last_refresh.write() {
            *last = Some(Instant::now());
        }

        let text = match self.client.get(&self.config_url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(resp) => match resp.text().await {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("[allanime] remote config read failed: {e}");
                        return false;
                    }
                },
                Err(e) => {
                    eprintln!("[allanime] remote config http error: {e}");
                    return false;
                }
            },
            Err(e) => {
                eprintln!("[allanime] remote config fetch failed: {e}");
                return false;
            }
        };
        let parsed = match AllAnimeConfig::parse_validated(&text) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[allanime] remote config rejected: {e}");
                return false;
            }
        };
        if self.config() == parsed {
            return false;
        }
        if let Ok(mut c) = self.config.write() {
            eprintln!(
                "[allanime] remote config applied (build_id {} -> {})",
                c.build_id, parsed.build_id
            );
            *c = parsed;
        }
        if let Ok(mut r) = self.remote_applied.write() {
            *r = true;
        }
        true
    }

    /// See [`ProviderStatus`].
    pub fn status(&self) -> ProviderStatus {
        let remote = self.remote_applied.read().map(|r| *r).unwrap_or(false);
        ProviderStatus {
            build_id: self.config().build_id,
            config_source: if remote { "remote" } else { "baked" },
            config_url: self.config_url.clone(),
        }
    }

    /// Fetch the current `{ epoch, partB }` used to sign the aaReq token and
    /// decrypt the episode-sources payload.
    ///
    /// The endpoint now gates on a per-content `lane` (`?k=`) and a signed
    /// `x-aa-boot` header keyed by a client-computed epoch (3-day buckets). We
    /// try the current bucket and — inside its first day — the previous one
    /// (matching the web client's `[qS(), zh()]` candidate list), and take the
    /// first that authenticates. The response echoes the authoritative epoch.
    async fn fetch_bootstrap(
        &self,
        cfg: &AllAnimeConfig,
        lane: &str,
        now_ms: u128,
    ) -> Result<Bootstrap> {
        let mut last_err: Option<Error> = None;
        for epoch in epoch_candidates(cfg.epoch_bucket_ms, now_ms) {
            let aa_boot = decrypt::sign_aa_boot(
                &cfg.qd_mask_hex,
                &cfg.boot_label,
                &cfg.boot_sig_template,
                &decrypt::BootSig {
                    build_id: &cfg.build_id,
                    key_group: &cfg.key_group,
                    referer_host: &cfg.referer_host,
                    epoch,
                    lane,
                },
            )?;
            let resp = self
                .client
                .get(&cfg.bootstrap_url)
                .header("Referer", &cfg.referer)
                .header("Origin", &cfg.referer)
                .header("x-build-id", &cfg.build_id)
                .header("x-aa-boot", aa_boot)
                .query(&[("buildId", cfg.build_id.as_str()), ("k", lane)])
                .send()
                .await?;
            if !resp.status().is_success() {
                last_err = Some(Error::Provider(format!(
                    "sources: bootstrap rejected epoch {epoch} ({})",
                    resp.status()
                )));
                continue;
            }
            let boot = resp
                .json::<Bootstrap>()
                .await
                .map_err(|e| Error::Provider(format!("sources: bad bootstrap response: {e}")))?;
            if let Some(ms) = boot.epoch_ms {
                if u128::from(ms) != cfg.epoch_bucket_ms {
                    eprintln!(
                        "[allanime] WARNING: bootstrap epochMs={ms} but config epoch_bucket_ms={} — bucket rotated, update the config",
                        cfg.epoch_bucket_ms
                    );
                }
            }
            return Ok(boot);
        }
        Err(last_err
            .unwrap_or_else(|| Error::Provider("sources: bootstrap: no epoch candidates".into())))
    }

    /// GET the persisted episode-sources query, carrying the signed `aaReq`
    /// token the server now requires (missing/stale ones yield `AA_CRYPTO_*`
    /// and, downstream, "sources: missing episode.sourceUrls").
    async fn get_episode_persisted(
        &self,
        cfg: &AllAnimeConfig,
        query_hash: &str,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
        aa_req: &str,
    ) -> Result<String> {
        // POST the full query text with a persisted-query hash we compute from
        // that same text (Apollo APQ client registration). Unlike a bare GET the
        // server always accepts this — it never depends on the hash being
        // pre-registered server-side, which is what breaks when allanime rotates
        // its persisted-query hashes (`PersistedQueryNotFound`). The aaReq token
        // binds `qh` to this hash, so all three (query, hash, aaReq) agree.
        let variables = json!({
            "showId": show_id,
            "translationType": mode.as_str(),
            "episodeString": episode,
        });
        let extensions = json!({
            "persistedQuery": { "version": 1, "sha256Hash": query_hash },
            "k": cfg.episode_lane,
            "aaReq": aa_req,
        });
        let body = json!({
            "query": cfg.episode_query,
            "variables": variables,
            "extensions": extensions,
        });
        let resp = self
            .client
            .post(&cfg.api_url)
            .header("Referer", &cfg.referer)
            .header("Origin", &cfg.referer)
            .header("x-build-id", &cfg.build_id)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    async fn post_gql(
        &self,
        cfg: &AllAnimeConfig,
        variables: Value,
        query: &str,
    ) -> Result<String> {
        let body = json!({ "variables": variables, "query": query });
        let resp = self
            .client
            .post(&cfg.api_url)
            .header("Referer", &cfg.referer)
            .header("Origin", &cfg.referer)
            .header("x-build-id", &cfg.build_id)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    /// Fetch a deobfuscated `/clock.json?...` embed path against the base host.
    async fn fetch_clock(&self, cfg: &AllAnimeConfig, path: &str) -> Result<String> {
        let url = format!("https://{}{path}", cfg.base_host);
        let resp = self
            .client
            .get(&url)
            .header("Referer", &cfg.referer)
            .header("Origin", &cfg.referer)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    /// The sources GraphQL response may inline the sourceUrls JSON or wrap it
    /// in an encrypted `tobeparsed` blob. Return the plaintext JSON either way.
    fn unwrap_sources_response(&self, body: &str, key: &[u8; 32]) -> Result<String> {
        let v: Value = serde_json::from_str(body)
            .map_err(|e| Error::Provider(format!("sources: invalid json envelope: {e}")))?;
        if let Some(tbp) = find_tobeparsed(&v) {
            decrypt::decrypt_tobeparsed(tbp, key)
        } else {
            Ok(body.to_string())
        }
    }
}

/// Best-effort playability ranking for ordering sources (0 = best).
///
/// A webview `<video>` can only play a direct media file (MP4/HLS), not an
/// embed *page*. allanime mixes both kinds into one list; ani-cli sidesteps
/// this by only handling a known subset. We can't extract embed pages here, so
/// we at least float the directly-playable sources to the top:
///   0 — looks like a direct media file / known direct CDN
///   1 — unknown (could be either)
///   2 — looks like an HTML embed page (ok.ru, mp4upload, /e/…, …)
pub fn playability_rank(s: &crate::models::VideoSource) -> u8 {
    let url = s.url.to_ascii_lowercase();
    let path = url.split(['?', '#']).next().unwrap_or(&url);

    let is_media_ext = path.ends_with(".m3u8")
        || path.ends_with(".mp4")
        || path.ends_with(".m4v")
        || path.ends_with(".mkv")
        || path.ends_with(".webm");
    let is_direct_cdn = url.contains("fast4speed")
        || url.contains("wixmp")
        || (url.contains("sharepoint.com") && url.contains("download.aspx"));
    if matches!(s.kind, crate::models::StreamKind::Hls) || is_media_ext || is_direct_cdn {
        return 0;
    }

    let is_embed_page = path.ends_with(".html")
        || url.contains("/e/")
        || url.contains("/embed")
        || url.contains("videoembed")
        || url.contains("ok.ru")
        || url.contains("mp4upload")
        || url.contains("vidnest")
        // Fragment-routed single-page embeds (e.g. allanime.uns.bio/#abc123).
        || url.contains("uns.bio");
    if is_embed_page {
        return 2;
    }
    1
}

/// Recursively search for a `tobeparsed` string field anywhere in the value.
fn find_tobeparsed(v: &Value) -> Option<&str> {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("tobeparsed") {
                return Some(s);
            }
            map.values().find_map(find_tobeparsed)
        }
        Value::Array(arr) => arr.iter().find_map(find_tobeparsed),
        _ => None,
    }
}

#[async_trait]
impl Provider for AllAnime {
    fn name(&self) -> &'static str {
        "allanime"
    }

    async fn search(&self, query: &str, mode: TranslationType) -> Result<Vec<AnimeSummary>> {
        let variables = json!({
            "search": { "allowAdult": false, "allowUnknown": false, "query": query },
            "limit": 40,
            "page": 1,
            "translationType": mode.as_str(),
            "countryOrigin": "ALL"
        });
        let body = self.post_gql(&self.config(), variables, SEARCH_GQL).await?;
        parse::parse_search(&body)
    }

    async fn episodes(&self, show_id: &str, mode: TranslationType) -> Result<Vec<String>> {
        let variables = json!({ "showId": show_id });
        let body = self
            .post_gql(&self.config(), variables, EPISODES_LIST_GQL)
            .await?;
        parse::parse_episodes(&body, mode.as_str())
    }

    async fn sources(
        &self,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
    ) -> Result<Vec<VideoSource>> {
        // Self-heal on provider rotation: run against the current config, and if
        // it fails or comes back empty (the shape a fresh crypto rotation takes —
        // bootstrap rejected, or `tobeparsed`/sourceUrls missing), pull the
        // remote config override and retry once. A rotation the maintainer has
        // already published thus fixes itself on the next play attempt, no app
        // update required. When there's no remote config (or it's unchanged) the
        // first result stands.
        let first = self
            .sources_with(&self.config(), show_id, episode, mode)
            .await;
        if matches!(&first, Ok(v) if !v.is_empty()) {
            return first;
        }
        if self.refresh_config(false).await {
            return self
                .sources_with(&self.config(), show_id, episode, mode)
                .await
                .map_err(classify_rotation);
        }
        first.map_err(classify_rotation)
    }
}

impl AllAnime {
    /// One full sources fetch against a fixed config snapshot: bootstrap the
    /// per-epoch key, sign the aaReq, GET the persisted query, decrypt, and
    /// resolve every embed ref into concrete playable links.
    async fn sources_with(
        &self,
        cfg: &AllAnimeConfig,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
    ) -> Result<Vec<VideoSource>> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::Provider(format!("sources: system clock before epoch: {e}")))?
            .as_millis();
        let boot = self.fetch_bootstrap(cfg, &cfg.episode_lane, now_ms).await?;
        let key = decrypt::derive_key_xor(&boot.part_b, &cfg.qd_mask_hex)?;
        // Persisted-query hash is computed from our own query text (APQ), so it
        // always matches what we POST and what the aaReq attests to.
        let query_hash = decrypt::sha256_hex(&cfg.episode_query);
        let aa_req = decrypt::sign_aa_req(
            &key,
            &cfg.aa_req_seed_template,
            boot.epoch,
            &cfg.build_id,
            &query_hash,
            &cfg.episode_lane,
            now_ms,
        )?;

        let body = self
            .get_episode_persisted(cfg, &query_hash, show_id, episode, mode, &aa_req)
            .await?;
        let refs = self
            .unwrap_sources_response(&body, &key)
            .and_then(|j| parse::parse_source_refs(&j))?;

        // Resolve each embed reference into concrete links, skipping refs
        // that fail rather than aborting the whole set.
        let mut tasks = Vec::new();
        for r in refs {
            tasks.push(self.resolve_ref(cfg, r));
        }
        let mut sources = Vec::new();
        for mut links in futures_join(tasks).await.into_iter().flatten() {
            sources.append(&mut links);
        }

        // Order so the default (first) source is one that actually plays in a
        // webview <video>. Many allanime "sources" are HTML embed pages we
        // cannot drop straight into a media element; sort those last. Within a
        // playability tier, prefer higher numeric resolution.
        sources.sort_by(|a, b| {
            playability_rank(a).cmp(&playability_rank(b)).then_with(|| {
                let qa: i64 = a.quality.parse().unwrap_or(-1);
                let qb: i64 = b.quality.parse().unwrap_or(-1);
                qb.cmp(&qa)
            })
        });
        Ok(sources)
    }

    async fn resolve_ref(
        &self,
        cfg: &AllAnimeConfig,
        r: parse::SourceRef,
    ) -> Result<Vec<VideoSource>> {
        // Direct-download hosts (ani-cli's fast4speed/Yt case) are already
        // playable and need no clock indirection.
        if r.url.starts_with("http://") || r.url.starts_with("https://") {
            // Many allanime "sources" are HTML *embed* player pages, not media
            // files — a webview <video> can't play them. Resolve the hosts we
            // understand into direct media so they become real, top-ranked
            // sources instead of rank-2 dead ends. On a miss (unknown host, or a
            // known one whose page yielded nothing — deleted/region-locked) we
            // fall through and return the URL as-is, so nothing regresses.
            if let Some(links) = self.resolve_embed(&r.url, &r.name).await {
                if !links.is_empty() {
                    return Ok(links);
                }
            }
            let kind = if r.url.contains(".m3u8") {
                crate::models::StreamKind::Hls
            } else {
                crate::models::StreamKind::Mp4
            };
            return Ok(vec![VideoSource {
                provider_name: r.name,
                quality: "auto".into(),
                url: r.url,
                kind,
                referer: Some(cfg.referer.clone()),
                subtitles: Vec::new(),
            }]);
        }

        let path = decrypt::deobfuscate_source_url(&r.url)?;
        if !path.starts_with('/') {
            return Ok(Vec::new());
        }
        let clock_body = self.fetch_clock(cfg, &path).await?;
        Ok(parse::parse_clock_links(
            &clock_body,
            &r.name,
            Some(&cfg.referer),
        ))
    }

    /// Dispatch a direct embed URL to a host-specific extractor. Returns `None`
    /// for hosts we don't handle (streamsb is a dead ad-parked domain,
    /// streamlare walls every endpoint behind an anti-adblock shell — neither is
    /// extractable server-side), and `Some(vec![])` when a known host yielded
    /// nothing. Both cases leave [`resolve_ref`] to fall back to the raw URL.
    async fn resolve_embed(&self, url: &str, name: &str) -> Option<Vec<VideoSource>> {
        if url.contains("mp4upload.com") {
            return Some(self.extract_mp4upload(url, name).await.unwrap_or_default());
        }
        if url.contains("ok.ru") || url.contains("odnoklassniki") {
            return Some(self.extract_okru(url, name).await.unwrap_or_default());
        }
        None
    }

    /// Fetch an mp4upload embed page and pull the direct `/d/…/video.mp4` URL
    /// out of its `player.src({...})` call. Empty on a deleted file / layout
    /// change. The page host gates the fetch on the `www.` referer, same as the
    /// media file itself, which is why the source carries [`MP4UPLOAD_REFERER`].
    async fn extract_mp4upload(&self, embed_url: &str, name: &str) -> Result<Vec<VideoSource>> {
        let body = self
            .client
            .get(embed_url)
            .header("Referer", MP4UPLOAD_REFERER)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(find_player_src(&body)
            .map(|src| {
                vec![VideoSource {
                    provider_name: name.to_string(),
                    quality: "auto".into(),
                    url: src.to_string(),
                    kind: crate::models::StreamKind::Mp4,
                    referer: Some(MP4UPLOAD_REFERER.to_string()),
                    subtitles: Vec::new(),
                }]
            })
            .unwrap_or_default())
    }

    /// Resolve an ok.ru embed into direct renditions via its player-metadata
    /// API (`/dk?cmd=videoPlayerMetadata&mid=<id>`), which returns the
    /// progressive MP4 list plus an adaptive HLS manifest. Empty when the video
    /// is gone or `copyrightsRestricted` (the API answers `{"error":…}`).
    async fn extract_okru(&self, embed_url: &str, name: &str) -> Result<Vec<VideoSource>> {
        let mid = match okru_mid(embed_url) {
            Some(m) => m,
            None => return Ok(Vec::new()),
        };
        let body = self
            .client
            .post(format!(
                "https://ok.ru/dk?cmd=videoPlayerMetadata&mid={mid}"
            ))
            .header("Referer", "https://ok.ru/")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(parse_okru_metadata(&body, name))
    }
}

/// Extract the `src: "…"` string from the first `player.src({ … })` call in an
/// mp4upload embed page. Whitespace/newlines between tokens vary, so we scan for
/// the `player.src(` anchor, then the next `src:` and its quoted value.
fn find_player_src(html: &str) -> Option<&str> {
    let after = &html[html.find("player.src(")?..];
    let after = &after[after.find("src:")?..];
    let start = after.find('"')? + 1;
    let rest = &after[start..];
    let end = rest.find('"')?;
    let url = rest[..end].trim();
    url.starts_with("http").then_some(url)
}

/// Numeric video id from an ok.ru embed URL (`.../videoembed/<id>` or
/// `.../video/<id>`). ok.ru ids are all-digit; anything else is unrecognised.
fn okru_mid(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let seg = path.trim_end_matches('/').rsplit('/').next()?;
    (!seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit())).then(|| seg.to_string())
}

/// Map an ok.ru rendition label to an approximate vertical resolution; unknown
/// labels pass through so the quality chip still shows something meaningful.
fn okru_quality(name: &str) -> String {
    match name {
        "mobile" => "144",
        "lowest" => "240",
        "low" => "360",
        "sd" => "480",
        "hd" => "720",
        "full" => "1080",
        "quad" => "1440",
        "ultra" => "2160",
        other => other,
    }
    .to_string()
}

/// Parse ok.ru `videoPlayerMetadata` JSON into playable sources: the
/// progressive `videos:[{name,url}]` renditions plus the adaptive HLS manifest
/// (added last but floated to the top by its [`crate::models::StreamKind::Hls`]
/// playability rank). Empty on an `{"error":…}` body or unparseable JSON.
fn parse_okru_metadata(body: &str, name: &str) -> Vec<VideoSource> {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if v.get("error").is_some() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(arr) = v.get("videos").and_then(Value::as_array) {
        for item in arr {
            let Some(url) = item.get("url").and_then(Value::as_str) else {
                continue;
            };
            if url.is_empty() {
                continue;
            }
            let quality = item
                .get("name")
                .and_then(Value::as_str)
                .map_or_else(|| "auto".to_string(), okru_quality);
            out.push(VideoSource {
                provider_name: name.to_string(),
                quality,
                url: normalize_scheme(url),
                kind: crate::models::StreamKind::Mp4,
                referer: None,
                subtitles: Vec::new(),
            });
        }
    }
    for key in ["hlsManifestUrl", "hlsMasterPlaylistUrl", "ondemandHls"] {
        if let Some(u) = v.get(key).and_then(Value::as_str).filter(|u| !u.is_empty()) {
            out.push(VideoSource {
                provider_name: name.to_string(),
                quality: "auto".into(),
                url: normalize_scheme(u),
                kind: crate::models::StreamKind::Hls,
                referer: None,
                subtitles: Vec::new(),
            });
            break;
        }
    }
    out
}

/// Promote a protocol-relative (`//host/…`) URL to `https://`; pass others
/// through unchanged. ok.ru occasionally emits scheme-relative CDN URLs.
fn normalize_scheme(url: &str) -> String {
    match url.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

/// Minimal join over a Vec of futures without pulling in the `futures` crate.
async fn futures_join<F, T>(tasks: Vec<F>) -> Vec<T>
where
    F: std::future::Future<Output = T>,
{
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        out.push(t.await);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_candidates_adds_grace_bucket_early_in_window() {
        let bucket = 259_200_000u128;
        // 1 hour into a fresh bucket -> current + previous (grace) candidate.
        let early = 10 * bucket + 3_600_000;
        assert_eq!(epoch_candidates(bucket, early), vec![9, 10]);
        // 2 days in (past the 1-day grace) -> just the current bucket.
        let late = 10 * bucket + 2 * 86_400_000;
        assert_eq!(epoch_candidates(bucket, late), vec![10]);
    }

    #[tokio::test]
    async fn remote_config_refresh_applies_over_http() {
        use std::io::{Read, Write};

        // A published rotation: same valid mask, new build_id.
        let body = format!(r#"{{"build_id":"999","qd_mask_hex":"{QD_MASK_HEX}"}}"#);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let provider = AllAnime::new_with_config_url(format!("http://{addr}/config.json"));
        // Starts from baked-in defaults.
        assert_eq!(provider.config().build_id, BUILD_ID);
        // Fetch, validate, and swap in the override.
        let changed = provider.refresh_config(true).await;
        assert!(changed, "refresh should apply the fetched override");
        assert_eq!(provider.config().build_id, "999");
        // Untouched field kept its default.
        assert_eq!(provider.config().api_url, API_URL);
    }

    #[tokio::test]
    async fn refresh_config_noop_when_url_empty() {
        // Default construction (empty REMOTE_CONFIG_URL, no env) must never touch
        // the network and always reports "unchanged".
        let provider = AllAnime::new();
        if provider.config_url.trim().is_empty() {
            assert!(!provider.refresh_config(true).await);
            assert_eq!(provider.config().build_id, BUILD_ID);
        }
    }

    #[test]
    fn classify_rotation_prefixes_only_rotation_shapes() {
        let e = classify_rotation(Error::Provider(
            "sources: bootstrap rejected epoch 2957 (404 Not Found)".into(),
        ));
        assert!(e.to_string().contains(ROTATED_PREFIX), "{e}");
        // Idempotent.
        let again = classify_rotation(e);
        assert_eq!(again.to_string().matches(ROTATED_PREFIX).count(), 1);
        // A plain provider error (e.g. a bad search body) is left alone.
        let plain = classify_rotation(Error::Provider("search: invalid json: x".into()));
        assert!(!plain.to_string().contains(ROTATED_PREFIX));
    }

    #[test]
    fn find_tobeparsed_nested() {
        let v: Value =
            serde_json::from_str(r#"{"data":{"episode":{"tobeparsed":"AAAA","other":1}}}"#)
                .unwrap();
        assert_eq!(find_tobeparsed(&v), Some("AAAA"));
    }

    #[test]
    fn find_player_src_extracts_mp4() {
        let html = r#"
            <script>
            var player = videojs('vid');
            player.src({
                type: "video/mp4",
                src: "https://a3.mp4upload.com:183/d/abc/video.mp4"
            })
            </script>"#;
        assert_eq!(
            find_player_src(html),
            Some("https://a3.mp4upload.com:183/d/abc/video.mp4")
        );
    }

    #[test]
    fn find_player_src_none_when_absent() {
        assert_eq!(find_player_src("<html>File deleted</html>"), None);
        // player.src present but no usable http src (deleted-file placeholder).
        assert_eq!(find_player_src(r#"player.src({src: ""})"#), None);
    }

    #[test]
    fn okru_mid_from_embed_urls() {
        assert_eq!(
            okru_mid("https://ok.ru/videoembed/3201471154834").as_deref(),
            Some("3201471154834")
        );
        assert_eq!(
            okru_mid("https://ok.ru/video/123?st=1").as_deref(),
            Some("123")
        );
        assert_eq!(okru_mid("https://ok.ru/videoembed/abc").as_deref(), None);
    }

    #[test]
    fn parse_okru_metadata_extracts_mp4s_and_hls() {
        let body = r#"{
            "videos":[
                {"name":"sd","url":"//vd1.okcdn.ru/sd.mp4"},
                {"name":"full","url":"https://vd2.okcdn.ru/full.mp4"},
                {"name":"skip"}
            ],
            "hlsManifestUrl":"https://vd.okcdn.ru/hls/master.m3u8?p=1"
        }"#;
        let out = parse_okru_metadata(body, "Ok");
        // 2 mp4 renditions + 1 hls manifest; the "skip" entry (no url) dropped.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].quality, "480");
        assert_eq!(out[0].url, "https://vd1.okcdn.ru/sd.mp4"); // scheme promoted
        assert_eq!(out[1].quality, "1080");
        assert_eq!(out[2].kind, crate::models::StreamKind::Hls);
        // The HLS manifest outranks the progressive MP4s for defaulting.
        assert_eq!(playability_rank(&out[2]), 0);
    }

    #[test]
    fn parse_okru_metadata_empty_on_error_or_garbage() {
        assert!(parse_okru_metadata(r#"{"error":"copyrightsRestricted"}"#, "Ok").is_empty());
        assert!(parse_okru_metadata("not json", "Ok").is_empty());
    }

    fn src(url: &str, kind: crate::models::StreamKind) -> crate::models::VideoSource {
        crate::models::VideoSource {
            provider_name: "t".into(),
            quality: "auto".into(),
            url: url.into(),
            kind,
            referer: None,
            subtitles: Vec::new(),
        }
    }

    #[test]
    fn playability_rank_orders_direct_media_before_embeds() {
        use crate::models::StreamKind::*;
        // Direct media / known CDNs -> 0
        assert_eq!(playability_rank(&src("https://cdn/x.mp4", Mp4)), 0);
        assert_eq!(playability_rank(&src("https://cdn/x.m3u8", Hls)), 0);
        assert_eq!(
            playability_rank(&src(
                "https://tools.fast4speed.rsvp/media/1?Authorization=z",
                Mp4
            )),
            0
        );
        assert_eq!(
            playability_rank(&src(
                "https://x.sharepoint.com/_layouts/15/download.aspx?id=1",
                Mp4
            )),
            0
        );
        // HTML embed pages -> 2
        assert_eq!(
            playability_rank(&src("https://ok.ru/videoembed/123", Mp4)),
            2
        );
        assert_eq!(
            playability_rank(&src("https://mp4upload.com/embed-a.html", Mp4)),
            2
        );
        assert_eq!(playability_rank(&src("https://vidnest.io/e/abc", Mp4)), 2);
        assert_eq!(
            playability_rank(&src("https://allanime.uns.bio/#abc", Mp4)),
            2
        );
        // Unknown -> 1
        assert_eq!(playability_rank(&src("https://weird.host/thing", Mp4)), 1);
    }

    #[test]
    fn unwrap_sources_passes_through_plain() {
        let a = AllAnime::new();
        let plain = r#"{"data":{"episode":{"sourceUrls":[]}}}"#;
        assert_eq!(a.unwrap_sources_response(plain, &[0u8; 32]).unwrap(), plain);
    }
}
