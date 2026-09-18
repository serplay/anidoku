//! Namespaced show ids: `"<source>:<show_id>"`.
//!
//! Before multi-source, a show was identified by a bare provider id (an
//! allanime `_id`) used as a DB primary key, a URL segment, and a download
//! directory name. With more than one scraper those ids can collide and, worse,
//! say nothing about who can resolve them.
//!
//! Prefixing the source keeps every one of those uses working unchanged: the
//! id stays an opaque string to routing and to the schema, and
//! [`downloads::sanitize_component`](crate::downloads::sanitize_component)
//! already maps `:` to `_`, so it remains a legal path segment on every
//! platform.
//!
//! Ids persisted before the migration have no prefix. [`SourceId::parse`]
//! therefore treats a colon-free id as allanime's — which is what it is, since
//! allanime was the only source that could have written one.

use std::fmt;

/// The source a bare, un-prefixed legacy id must have come from.
pub const LEGACY_SOURCE: &str = "allanime";

/// A show id scoped to the source that can resolve it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId {
    pub source: String,
    pub show: String,
}

impl SourceId {
    pub fn new(source: impl Into<String>, show: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            show: show.into(),
        }
    }

    /// Split a stored/routed id. A colon-free id is a pre-migration allanime
    /// id; only the *first* colon separates, because a show id may contain one.
    pub fn parse(id: &str) -> Self {
        match id.split_once(':') {
            Some((source, show)) if !source.is_empty() && !show.is_empty() => {
                Self::new(source, show)
            }
            _ => Self::new(LEGACY_SOURCE, id),
        }
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source, self.show)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let id = SourceId::new("hianime", "one-piece-100");
        assert_eq!(id.to_string(), "hianime:one-piece-100");
        assert_eq!(SourceId::parse(&id.to_string()), id);
    }

    #[test]
    fn a_bare_id_is_a_legacy_allanime_id() {
        // Every id written before migration 007 looks like this.
        let id = SourceId::parse("ReooPAxPMsHM4KPMY");
        assert_eq!(id.source, "allanime");
        assert_eq!(id.show, "ReooPAxPMsHM4KPMY");
    }

    #[test]
    fn only_the_first_colon_separates() {
        let id = SourceId::parse("animepahe:abc:def");
        assert_eq!(id.source, "animepahe");
        assert_eq!(id.show, "abc:def");
    }

    #[test]
    fn a_malformed_id_degrades_to_legacy_rather_than_failing() {
        // Better a lookup that misses than a panic on user data.
        assert_eq!(SourceId::parse(":x").source, LEGACY_SOURCE);
        assert_eq!(SourceId::parse("x:").source, LEGACY_SOURCE);
        assert_eq!(SourceId::parse("").show, "");
    }
}
