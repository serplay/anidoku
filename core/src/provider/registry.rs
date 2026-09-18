//! The set of scraping sources the app can use, and the user's ordering.
//!
//! Every command reaches a scraper through here rather than holding a concrete
//! type, so adding a source is a one-line registration plus its module.
//!
//! Order is the failover order: `enabled_ordered` returns the sources to try,
//! best first. It is user-editable and persisted in `app_settings` (two keys,
//! no new table), and is deliberately forgiving — an unknown slug left over
//! from an older build is ignored, and a source the user has never seen is
//! appended in registration order rather than hidden.

use super::Provider;
use crate::db::Database;
use std::sync::Arc;

/// `app_settings` key holding the comma-separated failover order.
pub const ORDER_KEY: &str = "source_order";
/// `app_settings` key holding the comma-separated *disabled* slugs. Storing the
/// disabled set (not the enabled one) means a newly shipped source is on by
/// default for existing installs.
pub const DISABLED_KEY: &str = "sources_disabled";

pub struct Registry {
    sources: Vec<Arc<dyn Provider>>,
}

impl Registry {
    /// Registration order doubles as the default failover order.
    pub fn new(sources: Vec<Arc<dyn Provider>>) -> Self {
        Self { sources }
    }

    pub fn all(&self) -> &[Arc<dyn Provider>] {
        &self.sources
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.sources.iter().find(|p| p.id() == id).cloned()
    }

    /// Sources to try, best first, honouring the user's order and disabled set.
    /// Never returns an empty list when any source is registered: if the user
    /// has disabled everything, that is reported by `enabled_ids` being empty,
    /// and callers surface it rather than silently falling back.
    pub fn enabled_ordered(&self, db: &Database) -> Vec<Arc<dyn Provider>> {
        let order = csv_setting(db, ORDER_KEY);
        let disabled = csv_setting(db, DISABLED_KEY);

        let mut out: Vec<Arc<dyn Provider>> = Vec::with_capacity(self.sources.len());
        // User-ordered first, skipping slugs this build no longer ships.
        for slug in &order {
            if let Some(p) = self.get(slug) {
                if !out.iter().any(|q| q.id() == p.id()) {
                    out.push(p);
                }
            }
        }
        // Then anything registered but not mentioned (a newly shipped source).
        for p in &self.sources {
            if !out.iter().any(|q| q.id() == p.id()) {
                out.push(p.clone());
            }
        }
        out.retain(|p| !disabled.iter().any(|d| d == p.id()));
        out
    }

    pub fn set_order(&self, db: &Database, order: &[String]) -> crate::Result<()> {
        db.set_setting(ORDER_KEY, &order.join(","))
    }

    pub fn set_enabled(&self, db: &Database, id: &str, enabled: bool) -> crate::Result<()> {
        let mut disabled = csv_setting(db, DISABLED_KEY);
        disabled.retain(|d| d != id);
        if !enabled {
            disabled.push(id.to_string());
        }
        db.set_setting(DISABLED_KEY, &disabled.join(","))
    }
}

/// A missing or unreadable setting is simply "no preference" — source
/// selection must never be the thing that breaks playback.
fn csv_setting(db: &Database, key: &str) -> Vec<String> {
    db.get_setting(key)
        .ok()
        .flatten()
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AnimeSummary, TranslationType, VideoSource};
    use crate::provider::{Capabilities, Provider};
    use crate::Result;
    use async_trait::async_trait;

    struct Stub(&'static str);

    #[async_trait]
    impl Provider for Stub {
        fn id(&self) -> &'static str {
            self.0
        }
        fn display_name(&self) -> &'static str {
            self.0
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                dub: true,
                carries_anilist_id: false,
                subtitles: true,
            }
        }
        async fn search(&self, _q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
            Ok(vec![])
        }
        async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
            Ok(vec![])
        }
        async fn sources(
            &self,
            _s: &str,
            _e: &str,
            _m: TranslationType,
        ) -> Result<Vec<VideoSource>> {
            Ok(vec![])
        }
    }

    fn registry() -> Registry {
        Registry::new(vec![
            Arc::new(Stub("allanime")),
            Arc::new(Stub("hianime")),
            Arc::new(Stub("animepahe")),
        ])
    }

    fn ids(v: &[Arc<dyn Provider>]) -> Vec<&str> {
        v.iter().map(|p| p.id()).collect()
    }

    fn db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn defaults_to_registration_order() {
        let db = db();
        assert_eq!(
            ids(&registry().enabled_ordered(&db)),
            ["allanime", "hianime", "animepahe"]
        );
    }

    #[test]
    fn honours_the_user_order() {
        let (db, r) = (db(), registry());
        r.set_order(&db, &["hianime".into(), "allanime".into()])
            .unwrap();
        // animepahe was not mentioned, so it keeps its registration position
        // at the end rather than disappearing.
        assert_eq!(
            ids(&r.enabled_ordered(&db)),
            ["hianime", "allanime", "animepahe"]
        );
    }

    #[test]
    fn disabling_removes_it_and_enabling_restores_its_place() {
        let (db, r) = (db(), registry());
        r.set_enabled(&db, "hianime", false).unwrap();
        assert_eq!(ids(&r.enabled_ordered(&db)), ["allanime", "animepahe"]);
        r.set_enabled(&db, "hianime", true).unwrap();
        assert_eq!(
            ids(&r.enabled_ordered(&db)),
            ["allanime", "hianime", "animepahe"]
        );
    }

    #[test]
    fn a_slug_from_an_older_build_is_ignored_not_fatal() {
        let (db, r) = (db(), registry());
        db.set_setting(ORDER_KEY, "gogoanime,hianime").unwrap();
        assert_eq!(
            ids(&r.enabled_ordered(&db)),
            ["hianime", "allanime", "animepahe"]
        );
    }

    #[test]
    fn a_duplicated_slug_appears_once() {
        let (db, r) = (db(), registry());
        db.set_setting(ORDER_KEY, "hianime,hianime,allanime")
            .unwrap();
        assert_eq!(
            ids(&r.enabled_ordered(&db)),
            ["hianime", "allanime", "animepahe"]
        );
    }

    #[test]
    fn disabling_everything_yields_an_empty_list_rather_than_a_fallback() {
        let (db, r) = (db(), registry());
        for id in ["allanime", "hianime", "animepahe"] {
            r.set_enabled(&db, id, false).unwrap();
        }
        assert!(r.enabled_ordered(&db).is_empty());
    }
}
