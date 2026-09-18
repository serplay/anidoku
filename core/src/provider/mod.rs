pub mod allanime;
pub mod id;
pub mod rank;
pub mod registry;

pub use id::SourceId;
pub use rank::playability_rank;
pub use registry::Registry;

use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::Result;
use async_trait::async_trait;

/// What a source can and cannot do, so callers don't have to special-case it
/// by name. A source with `dub: false` is skipped for dub requests instead of
/// being asked and returning nothing; `carries_anilist_id` says whether the
/// cheap exact mapping path in [`crate::sync::matching`] is available or the
/// title heuristic has to carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Capabilities {
    pub dub: bool,
    pub carries_anilist_id: bool,
    pub subtitles: bool,
}

/// Snapshot of where a source's volatile config currently comes from —
/// surfaced in the app's Settings so a user (or a bug report) can tell whether
/// the self-heal has kicked in.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceStatus {
    /// Stable source slug ([`Provider::id`]).
    pub source: String,
    pub display_name: String,
    /// Client build id, when the source has one that rotates.
    pub build_id: String,
    /// `"remote"` once a remote override has been applied, `"baked"` when the
    /// compiled-in values are live, `"static"` when the source has no rotating
    /// config at all.
    pub config_source: &'static str,
    /// The remote config URL in effect (empty = self-heal disabled).
    pub config_url: String,
}

impl SourceStatus {
    /// For a source with nothing volatile to report.
    pub fn r#static(source: &str, display_name: &str) -> Self {
        Self {
            source: source.to_string(),
            display_name: display_name.to_string(),
            build_id: String::new(),
            config_source: "static",
            config_url: String::new(),
        }
    }
}

/// Volatility containment boundary: everything the UI knows about scraping
/// goes through this trait. When one source breaks, only its module changes;
/// another source is additive.
///
/// Every method a command needs lives here, so the app holds
/// `Arc<dyn Provider>` and never a concrete scraper type.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable slug used in namespaced ids, settings, and the DB (`"allanime"`).
    /// Never shown to users and never changed once shipped — persisted rows
    /// point at it.
    fn id(&self) -> &'static str;

    /// Human label for the UI.
    fn display_name(&self) -> &'static str;

    fn capabilities(&self) -> Capabilities;

    /// Full-text search, returns summaries for the results grid.
    async fn search(&self, query: &str, mode: TranslationType) -> Result<Vec<AnimeSummary>>;

    /// Available episode numbers for a show, ascending. Episode numbers are
    /// strings because providers use fractional specials ("5.5").
    async fn episodes(&self, show_id: &str, mode: TranslationType) -> Result<Vec<String>>;

    /// Resolve one episode into directly playable links (already decrypted,
    /// deobfuscated, and tagged with the referer needed to fetch them).
    async fn sources(
        &self,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
    ) -> Result<Vec<VideoSource>>;

    /// Where this source's volatile config currently comes from. Defaulted so a
    /// source with nothing that rotates implements neither this nor
    /// [`Provider::refresh_config`].
    fn status(&self) -> SourceStatus {
        SourceStatus::r#static(self.id(), self.display_name())
    }

    /// Re-pull the remote config override. Returns whether the live config
    /// actually changed. `force` bypasses the caller-side throttle.
    async fn refresh_config(&self, _force: bool) -> bool {
        false
    }
}
