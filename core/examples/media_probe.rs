//! Standalone media-server probe, for verifying Range passthrough with curl.
//!
//! Serve an explicit URL:
//!   cargo run -p anidoku-core --example media_probe -- <url> [referer]
//!
//! Or resolve a real provider source and serve the first playable one:
//!   cargo run -p anidoku-core --example media_probe -- --resolve "frieren"
//!
//! Prints `MEDIA_URL=...` then blocks so you can curl it, e.g.:
//!   curl -s -D- -H 'Range: bytes=0-99' "$MEDIA_URL" -o /dev/null

use anidoku_core::media_server;
use anidoku_core::models::TranslationType;
use anidoku_core::provider::{allanime::AllAnime, Provider};
use anidoku_core::proxy::ProxyClient;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().unwrap_or_else(|| {
        eprintln!("usage: media_probe <url> [referer] | --resolve <query>");
        std::process::exit(2);
    });

    let (url, referer): (String, Option<String>) = if first == "--resolve" {
        let query = args.next().unwrap_or_else(|| "frieren".into());
        resolve_first(&query).await
    } else {
        (first, args.next())
    };

    let proxy = Arc::new(ProxyClient::new());
    let handle = media_server::spawn(proxy).await.expect("spawn media server");
    let media_url = media_server::make_media_url(&handle.base, &url, referer.as_deref());

    println!("MEDIA_BASE={}", handle.base);
    println!("UPSTREAM={url}");
    if let Some(r) = &referer {
        println!("REFERER={r}");
    }
    println!("MEDIA_URL={media_url}");
    println!("(serving; Ctrl-C to stop)");

    // Block forever.
    std::future::pending::<()>().await;
}

async fn resolve_first(query: &str) -> (String, Option<String>) {
    let p = AllAnime::new();
    let results = p.search(query, TranslationType::Sub).await.expect("search");
    let show = results.first().expect("at least one result");
    eprintln!("resolved show: {} ({})", show.title, show.provider_id);
    let eps = p
        .episodes(&show.provider_id, TranslationType::Sub)
        .await
        .expect("episodes");
    let ep = eps.first().expect("at least one episode");
    let sources = p
        .sources(&show.provider_id, ep, TranslationType::Sub)
        .await
        .expect("sources");
    let s = sources.first().expect("at least one source");
    eprintln!(
        "resolved source: [{}] quality={} kind={:?} url={}",
        s.provider_name, s.quality, s.kind, s.url
    );
    (s.url.clone(), s.referer.clone())
}
