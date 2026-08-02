//! AniList OAuth (implicit grant) + token storage.
//!
//! ## Flow
//! AniList desktop auth uses the *implicit grant*: the browser is sent to
//! `.../authorize?client_id=..&response_type=token` and AniList redirects back
//! with the token in the URL **fragment** (`#access_token=..&expires_in=..`).
//! Fragments never reach an HTTP server, so we run a one-shot loopback server
//! on a fixed port and serve a tiny HTML bridge at the redirect path that reads
//! `location.hash` in JS and re-requests it as a query string we *can* capture.
//!
//! The port is fixed (not random) because AniList requires the redirect URL to
//! match the one registered on the developer client exactly. The Settings page
//! shows the user the exact string to register.
//!
//! ## Token storage
//! AniList tokens live ~1 year with **no refresh token**. We persist the token,
//! its expiry, the client id, and the cached Viewer to an app-data JSON file
//! with `0600` perms. (A keychain plugin was considered; the file keeps M2
//! self-contained and cross-platform without a native credential prompt during
//! development — noted as a follow-up in the report.)

use anidoku_core::models::Viewer;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Fixed loopback port for the OAuth redirect. The registered redirect URL must
/// match `REDIRECT_URL` exactly.
pub const OAUTH_PORT: u16 = 8737;
/// Uses `127.0.0.1` (not `localhost`) so it matches the loopback bind exactly —
/// `localhost` can resolve to IPv6 `::1` and miss the listener.
pub const REDIRECT_URL: &str = "http://127.0.0.1:8737/callback";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Persisted {
    pub client_id: Option<String>,
    pub access_token: Option<String>,
    /// Unix seconds at which the token expires.
    pub expires_at: Option<i64>,
    pub viewer: Option<Viewer>,
}

/// Auth state: the on-disk `Persisted` blob plus its path, guarded by a mutex.
pub struct AuthStore {
    path: PathBuf,
    inner: Mutex<Persisted>,
}

impl AuthStore {
    pub fn load(config_dir: &std::path::Path) -> Self {
        let path = config_dir.join("anilist_auth.json");
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Persisted>(&s).ok())
            .unwrap_or_default();
        AuthStore {
            path,
            inner: Mutex::new(inner),
        }
    }

    fn persist(&self, p: &Persisted) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(json) = serde_json::to_string_pretty(p) else {
            return;
        };
        if let Ok(mut f) = std::fs::File::create(&self.path) {
            let _ = f.write_all(json.as_bytes());
            set_owner_only(&f);
        }
    }

    pub fn client_id(&self) -> Option<String> {
        self.inner.lock().unwrap().client_id.clone()
    }

    pub fn set_client_id(&self, id: Option<String>) {
        let mut g = self.inner.lock().unwrap();
        g.client_id = id.filter(|s| !s.trim().is_empty());
        self.persist(&g);
    }

    /// The current token if present and not expired; `None` otherwise.
    pub fn valid_token(&self) -> Option<String> {
        let g = self.inner.lock().unwrap();
        let token = g.access_token.clone()?;
        match g.expires_at {
            Some(exp) if exp <= now() => None,
            _ => Some(token),
        }
    }

    /// True when a token exists but has expired (drives the re-login prompt).
    pub fn is_expired(&self) -> bool {
        let g = self.inner.lock().unwrap();
        g.access_token.is_some() && matches!(g.expires_at, Some(exp) if exp <= now())
    }

    pub fn viewer(&self) -> Option<Viewer> {
        self.inner.lock().unwrap().viewer.clone()
    }

    pub fn save_token(&self, access_token: String, expires_at: i64) {
        let mut g = self.inner.lock().unwrap();
        g.access_token = Some(access_token);
        g.expires_at = Some(expires_at);
        self.persist(&g);
    }

    pub fn save_viewer(&self, viewer: Viewer) {
        let mut g = self.inner.lock().unwrap();
        g.viewer = Some(viewer);
        self.persist(&g);
    }

    pub fn logout(&self) {
        let mut g = self.inner.lock().unwrap();
        g.access_token = None;
        g.expires_at = None;
        g.viewer = None;
        self.persist(&g);
    }
}

#[cfg(unix)]
fn set_owner_only(f: &std::fs::File) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = f.metadata() {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = f.set_permissions(perms);
    }
}

#[cfg(not(unix))]
fn set_owner_only(_f: &std::fs::File) {}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Build the AniList authorize URL for the implicit grant.
pub fn authorize_url(client_id: &str) -> String {
    format!(
        "{}?client_id={}&response_type=token",
        anidoku_core::anilist::AUTHORIZE_URL,
        urlencoding::encode(client_id)
    )
}

/// Bind the fixed loopback OAuth port with `SO_REUSEADDR`.
///
/// The port is fixed (AniList requires an exact redirect URL), so back-to-back
/// login attempts are prone to `EADDRINUSE`: each capture opens real
/// connections on the port, and the connections that a *completed* attempt
/// leaves in `TIME_WAIT` make the plain `TcpListener::bind` reject a fresh bind
/// for a minute or so. `SO_REUSEADDR` lets us rebind past those lingering
/// connections. (This is the intermittent "cannot bind loopback port" the iOS
/// login hits when the user re-taps Sign in.)
fn bind_loopback_reuse() -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let addr: std::net::SocketAddr = (std::net::Ipv4Addr::LOCALHOST, OAUTH_PORT).into();
    let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    sock.set_reuse_address(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    sock.listen(128)?;
    tokio::net::TcpListener::from_std(sock.into())
}

/// Bind with a few retries. `SO_REUSEADDR` clears the `TIME_WAIT` case, but a
/// *superseded* prior capture task (see the single-flight abort in the login
/// command) releases its still-live listener asynchronously when it is dropped,
/// so the first rebind can still race it. A short bounded retry bridges that
/// hand-off gap; the loopback bind either succeeds within a few hundred ms or
/// something else genuinely owns the port.
async fn bind_loopback_retrying() -> Result<tokio::net::TcpListener, String> {
    let mut last = String::new();
    for attempt in 0..6u32 {
        match bind_loopback_reuse() {
            Ok(listener) => return Ok(listener),
            Err(e) => {
                last = e.to_string();
                tokio::time::sleep(Duration::from_millis(150 * (attempt as u64 + 1))).await;
            }
        }
    }
    Err(format!("cannot bind loopback port {OAUTH_PORT}: {last}"))
}

/// Result of a completed OAuth capture.
pub struct Captured {
    pub access_token: String,
    /// Unix seconds.
    pub expires_at: i64,
}

/// Run the one-shot loopback server: wait for AniList to redirect the browser
/// back, bridge the fragment through JS, and capture the token. Times out.
pub async fn run_loopback_capture(timeout: Duration) -> Result<Captured, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = bind_loopback_retrying().await?;

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let accept = tokio::time::timeout_at(deadline, listener.accept()).await;
        let (mut stream, _) = match accept {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => return Err(format!("accept failed: {e}")),
            Err(_) => return Err("login timed out (no response from browser)".into()),
        };

        // Read the request line (up to the first CRLF is enough for the path).
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/");

        if path.starts_with("/callback") && !path.contains('?') {
            // First hit: the fragment isn't sent. Serve the JS bridge that
            // re-requests the hash as a query string.
            let body = BRIDGE_HTML;
            let _ = write_http(&mut stream, "200 OK", "text/html", body).await;
            continue;
        }

        if path.starts_with("/capture") {
            let query = path.split_once('?').map(|x| x.1).unwrap_or("");
            let params = parse_query(query);
            let _ = write_http(&mut stream, "200 OK", "text/html", DONE_HTML).await;
            let _ = stream.shutdown().await;

            if let Some(token) = params.get("access_token") {
                let expires_in: i64 = params
                    .get("expires_in")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(31_536_000); // ~1 year default
                return Ok(Captured {
                    access_token: token.clone(),
                    expires_at: now() + expires_in,
                });
            }
            let err = params
                .get("error_description")
                .or_else(|| params.get("error"))
                .cloned()
                .unwrap_or_else(|| "no access_token in redirect".into());
            return Err(err);
        }

        // Anything else (favicon, etc.): 404 and keep waiting.
        let _ = write_http(&mut stream, "404 Not Found", "text/plain", "not found").await;
    }
}

async fn write_http(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await
}

fn parse_query(q: &str) -> std::collections::HashMap<String, String> {
    q.split('&')
        .filter_map(|kv| {
            let mut it = kv.splitn(2, '=');
            let k = it.next()?;
            let v = it.next().unwrap_or("");
            Some((
                urlencoding::decode(k).ok()?.into_owned(),
                urlencoding::decode(v).ok()?.into_owned(),
            ))
        })
        .collect()
}

const BRIDGE_HTML: &str = "<!doctype html><html><head><meta charset=utf-8><title>AniDoku</title></head>\
<body style=\"background:#0b0e11;color:#eaecef;font-family:system-ui;text-align:center;padding-top:20vh\">\
<p>Finishing sign-in…</p>\
<script>var h=location.hash.substring(1);location.replace('/capture?'+h);</script>\
</body></html>";

const DONE_HTML: &str = "<!doctype html><html><head><meta charset=utf-8><title>AniDoku</title></head>\
<body style=\"background:#0b0e11;color:#eaecef;font-family:system-ui;text-align:center;padding-top:20vh\">\
<h2 style=\"color:#fcd535\">Signed in to AniList</h2>\
<p>You can close this tab and return to AniDoku.</p>\
</body></html>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_encodes_client_id() {
        let u = authorize_url("123");
        assert!(u.contains("client_id=123"));
        assert!(u.contains("response_type=token"));
    }

    #[test]
    fn parse_query_decodes_pairs() {
        let m = parse_query("access_token=abc%2Fdef&expires_in=3600&token_type=Bearer");
        assert_eq!(m.get("access_token").unwrap(), "abc/def");
        assert_eq!(m.get("expires_in").unwrap(), "3600");
    }

    #[test]
    fn valid_token_respects_expiry() {
        let dir = std::env::temp_dir().join(format!("anidoku-auth-test-{}", std::process::id()));
        let store = AuthStore::load(&dir);
        store.save_token("tok".into(), now() + 1000);
        assert_eq!(store.valid_token().as_deref(), Some("tok"));
        store.save_token("tok".into(), now() - 10);
        assert!(store.valid_token().is_none());
        assert!(store.is_expired());
        store.logout();
        assert!(!store.is_expired());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
