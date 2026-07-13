pub mod allanime;

use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::Result;
use async_trait::async_trait;

/// Volatility containment boundary: everything the UI knows about scraping
/// goes through this trait. When allanime breaks, only `allanime/` changes;
/// a second provider is additive.
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

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
}
