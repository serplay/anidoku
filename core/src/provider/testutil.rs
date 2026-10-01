//! A scriptable loopback HTTP server for exercising scrapers end to end
//! without the network: each test supplies a closure mapping a request to a
//! status and body, so "the site is down", "the page changed", and "the happy
//! path" are all one-liners.

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Router;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub struct Req {
    pub method: String,
    /// Path plus query string, as sent.
    pub path: String,
    pub body: String,
    headers: Vec<(String, String)>,
}

impl Req {
    /// Header value by lowercase name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct TestServer {
    /// e.g. `http://127.0.0.1:52123`
    pub base: String,
    hits: Arc<AtomicUsize>,
}

impl TestServer {
    /// Requests served so far.
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

pub async fn serve<F>(handler: F) -> TestServer
where
    F: Fn(&Req) -> (u16, String) + Send + Sync + 'static,
{
    start(None, handler).await
}

/// Like [`serve`], with a `Set-Cookie` on every response.
pub async fn serve_with_cookie<F>(cookie: &'static str, handler: F) -> TestServer
where
    F: Fn(&Req) -> (u16, String) + Send + Sync + 'static,
{
    start(Some(cookie), handler).await
}

async fn start<F>(cookie: Option<&'static str>, handler: F) -> TestServer
where
    F: Fn(&Req) -> (u16, String) + Send + Sync + 'static,
{
    let hits = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(handler);
    let counter = hits.clone();
    let app = Router::new().fallback(move |request: Request| {
        let handler = handler.clone();
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let (parts, body) = request.into_parts();
            let body = to_bytes(body, usize::MAX).await.unwrap_or_default();
            let req = Req {
                method: parts.method.to_string(),
                path: parts
                    .uri
                    .path_and_query()
                    .map(|p| p.as_str().to_string())
                    .unwrap_or_default(),
                body: String::from_utf8_lossy(&body).into_owned(),
                headers: parts
                    .headers
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.as_str().to_ascii_lowercase(),
                            v.to_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect(),
            };
            let (status, text) = handler(&req);
            let mut resp = Response::builder()
                .status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK))
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8");
            if let Some(cookie) = cookie {
                resp = resp.header(header::SET_COOKIE, cookie);
            }
            resp.body(Body::from(text)).unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind test server");
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer { base, hits }
}
