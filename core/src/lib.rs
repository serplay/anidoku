//! AniDoku shared core.
//!
//! This crate holds everything that must work identically on every platform:
//! the provider engine (allanime port of ani-cli's flow), the SQLite store,
//! the streaming proxy that injects referer headers for the webview player,
//! and subtitle conversion.

pub mod anilist;
pub mod db;
pub mod error;
pub mod media_server;
pub mod models;
pub mod provider;
pub mod proxy;
pub mod range;
pub mod subs;
pub mod sync;

pub use error::{Error, Result};
