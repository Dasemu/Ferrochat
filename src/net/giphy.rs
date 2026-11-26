//! GIF search through GIPHY with the user's own API key.
//!
//! WhatsApp Desktop searches Tenor with WhatsApp's own key; Ferrochat never
//! borrows that key or fakes WhatsApp's origin. Instead the user can paste a
//! free GIPHY key (developers.giphy.com) in Settings; it lives in the OS
//! keyring (`auth::secrets`, account [`KEY_ACCOUNT`]) and is only sent to
//! `api.giphy.com`. Without a key the GIF tab still offers recent GIFs and
//! "send a GIF from a file".
//!
//! Blocking `ureq` calls, run on a detached thread by the caller (never on
//! tokio's blocking pool). Only GIPHY's hosts (or a loopback server in
//! tests, through `FERRO_GIPHY_API`) are reached, redirects are not
//! followed and URLs are never logged with their query (the key is a query
//! parameter).

use std::io::Read;
use std::time::Duration;

use serde_json::Value;
use url::Url;

/// Keyring account of the user's GIPHY API key.
pub const KEY_ACCOUNT: &str = "giphy-api-key";
/// Default API root.
pub const API: &str = "https://api.giphy.com/v1/gifs";
/// Results per search.
pub const PAGE: usize = 24;
/// Largest JSON answer read.
const MAX_JSON: u64 = 2 * 1024 * 1024;
/// Largest tile (animated WebP preview).
pub const MAX_TILE: u64 = 1024 * 1024;
/// Largest MP4 downloaded to send.
pub const MAX_MP4: u64 = 15 * 1024 * 1024;

/// One search result.
#[derive(Clone, PartialEq, Eq)]
pub struct GifHit {
    /// GIPHY id.
    pub id: String,
    /// Title (used as the tooltip / accessibility label).
    pub title: String,
    /// Small animated WebP for the picker tile.
    pub tile_url: String,
    /// Tile size.
    pub tile_size: (u32, u32),
    /// The MP4 that is sent.
    pub mp4_url: String,
    /// Its size.
    pub size: (u32, u32),
}

impl std::fmt::Debug for GifHit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GifHit({}x{})", self.size.0, self.size.1)
    }
}

/// Where searches go: GIPHY over HTTPS, or a loopback test server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// API root (`.../v1/gifs`).
    pub api: String,
    /// Plain-HTTP loopback allowed (tests).
    pub loopback: bool,
}

impl Endpoint {
    /// GIPHY, or `FERRO_GIPHY_API` when it names a loopback server.
    pub fn from_env() -> Self {
        match std::env::var("FERRO_GIPHY_API") {
            Ok(v) if Self::for_api(v.trim()).loopback => Self::for_api(v.trim()),
            _ => Self::giphy(),
        }
    }

    /// The real service.
    pub fn giphy() -> Self {
        Self {
            api: API.into(),
            loopback: false,
        }
    }

    /// An explicit root: loopback HTTP is allowed for tests, anything else
    /// falls back to GIPHY.
    pub fn for_api(api: &str) -> Self {
        match Url::parse(api) {
            Ok(u)
                if u.scheme() == "http"
                    && matches!(u.host_str(), Some("127.0.0.1" | "localhost")) =>
            {
                Self {
                    api: api.trim_end_matches('/').to_string(),
                    loopback: true,
                }
            }
            _ => Self::giphy(),
        }
    }

    /// Whether `url` may be fetched: GIPHY's API and media hosts over
    /// HTTPS, or the loopback server.
    pub fn allows(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if !url.username().is_empty() {
            return false;
        }
        if url.scheme() == "https" && giphy_host(host) {
            return true;
        }
        self.loopback && url.scheme() == "http" && matches!(host, "127.0.0.1" | "localhost")
    }
}

/// `api.giphy.com`, `giphy.com`, `i.giphy.com`, `media.giphy.com`,
/// `media0..4.giphy.com`.
pub fn giphy_host(host: &str) -> bool {
    host == "giphy.com"
        || host
            .strip_suffix(".giphy.com")
            .is_some_and(|sub| matches!(sub, "api" | "i" | "media") || is_media_n(sub))
}

fn is_media_n(sub: &str) -> bool {
    sub.strip_prefix("media")
        .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
}

/// `scheme://host/path` without query (the key) or fragment, for logs.
pub fn redact(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => "<invalid url>".into(),
    }
}

/// A GIPHY failure the user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    /// 401 / 403: the key is wrong or revoked.
    BadKey,
    /// 429: GIPHY's rate limit (beta keys allow ~100 searches an hour).
    RateLimited,
    /// Anything else.
    Failed(String),
}

impl SearchError {
    /// Text for the GIF tab.
    pub fn user_message(&self) -> String {
        match self {
            SearchError::BadKey => "GIPHY did not accept the API key. Check it in Settings.".into(),
            SearchError::RateLimited => {
                "GIPHY's limit for this key was reached. Try again later.".into()
            }
            SearchError::Failed(e) => format!("GIF search failed: {e}"),
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .https_only(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(format!("ferrochat/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// GET `url` (policy-checked, no redirects), at most `max` bytes.
pub fn fetch(endpoint: &Endpoint, url: &str, max: u64) -> Result<Vec<u8>, SearchError> {
    let parsed = Url::parse(url).map_err(|_| SearchError::Failed("invalid address".into()))?;
    if !endpoint.allows(&parsed) {
        return Err(SearchError::Failed(format!(
            "refusing to fetch from {}",
            parsed.host_str().unwrap_or("?")
        )));
    }
    let mut resp = agent().get(parsed.as_str()).call().map_err(|e| {
        tracing::debug!(url = %redact(url), error = %e, "GIPHY request failed");
        SearchError::Failed("network error".into())
    })?;
    match resp.status().as_u16() {
        200 => {}
        401 | 403 => return Err(SearchError::BadKey),
        429 => return Err(SearchError::RateLimited),
        s => return Err(SearchError::Failed(format!("HTTP {s}"))),
    }
    let mut body = Vec::new();
    resp.body_mut()
        .with_config()
        .limit(max + 1)
        .reader()
        .read_to_end(&mut body)
        .map_err(|_| SearchError::Failed("the download was cut off".into()))?;
    if body.len() as u64 > max {
        return Err(SearchError::Failed("the file is too large".into()));
    }
    Ok(body)
}

/// The search URL (`q`, `limit`, `rating=pg-13`, the key).
pub fn search_url(endpoint: &Endpoint, key: &str, query: &str) -> Option<String> {
    let mut u = Url::parse(&format!("{}/search", endpoint.api)).ok()?;
    u.query_pairs_mut()
        .append_pair("api_key", key)
        .append_pair("q", query)
        .append_pair("limit", &PAGE.to_string())
        .append_pair("rating", "pg-13")
        .append_pair("bundle", "messaging_non_clips");
    Some(u.into())
}

/// Searches GIPHY. Blocking.
pub fn search(endpoint: &Endpoint, key: &str, query: &str) -> Result<Vec<GifHit>, SearchError> {
    let url = search_url(endpoint, key, query)
        .ok_or_else(|| SearchError::Failed("invalid search".into()))?;
    let body = fetch(endpoint, &url, MAX_JSON)?;
    parse_search(&body).ok_or_else(|| SearchError::Failed("unexpected answer".into()))
}

fn num(v: &Value) -> Option<u32> {
    // GIPHY sends sizes as strings ("200"); accept numbers too.
    v.as_u64()
        .map(|n| n as u32)
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn mp4_size(r: &Value) -> u64 {
    r.get("mp4_size").and_then(num).map_or(0, u64::from)
}

/// Parses a GIPHY search answer. Results without an MP4 or a tile are
/// skipped; `None` when the answer is not GIPHY's shape.
pub fn parse_search(body: &[u8]) -> Option<Vec<GifHit>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let data = v.get("data")?.as_array()?;
    let mut out = Vec::new();
    for item in data.iter().take(PAGE) {
        let Some(images) = item.get("images") else {
            continue;
        };
        let tile = images
            .get("fixed_width_small")
            .or_else(|| images.get("fixed_width"));
        let Some(tile) = tile else { continue };
        let Some(tile_url) = str_of(tile, "webp") else {
            continue;
        };
        // The original when it is small enough to send, else the 200 px one.
        let send = [images.get("original"), images.get("fixed_width")]
            .into_iter()
            .flatten()
            .find(|r| str_of(r, "mp4").is_some() && mp4_size(r) <= MAX_MP4);
        let Some(send) = send else { continue };
        let (Some(w), Some(h)) = (
            send.get("width").and_then(num),
            send.get("height").and_then(num),
        ) else {
            continue;
        };
        out.push(GifHit {
            id: str_of(item, "id").unwrap_or_default(),
            title: str_of(item, "title").unwrap_or_default(),
            tile_url,
            tile_size: (
                tile.get("width").and_then(num).unwrap_or(100),
                tile.get("height").and_then(num).unwrap_or(100),
            ),
            mp4_url: str_of(send, "mp4").unwrap_or_default(),
            size: (w, h),
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"data":[
      {"id":"abc","title":"Cat GIF","images":{
        "original":{"width":"480","height":"270","mp4":"https://media2.giphy.com/media/abc/giphy.mp4","mp4_size":"900000"},
        "fixed_width":{"width":"200","height":"113","mp4":"https://media2.giphy.com/media/abc/200w.mp4","mp4_size":"90000"},
        "fixed_width_small":{"width":"100","height":"56","webp":"https://media2.giphy.com/media/abc/100w.webp"}}},
      {"id":"huge","images":{
        "original":{"width":"1920","height":"1080","mp4":"https://media.giphy.com/x.mp4","mp4_size":"99999999"},
        "fixed_width":{"width":200,"height":112,"mp4":"https://media.giphy.com/y.mp4","mp4_size":50000},
        "fixed_width_small":{"width":"100","height":"56","webp":"https://media.giphy.com/z.webp"}}},
      {"id":"no-mp4","images":{"fixed_width_small":{"webp":"https://media.giphy.com/q.webp"}}}
    ],"pagination":{},"meta":{"status":200}}"#;

    #[test]
    fn search_answers_become_hits() {
        let hits = parse_search(ANSWER.as_bytes()).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].size, (480, 270));
        assert_eq!(hits[0].tile_size, (100, 56));
        assert!(hits[0].mp4_url.ends_with("giphy.mp4"));
        // Too big to send: the 200 px rendition instead.
        assert_eq!(hits[1].size, (200, 112));
        assert!(hits[1].mp4_url.ends_with("y.mp4"));
        assert!(parse_search(b"not json").is_none());
        assert!(parse_search(b"{\"meta\":{}}").is_none());
        assert!(!format!("{:?}", hits[0]).contains("abc"));
    }

    #[test]
    fn only_giphy_hosts_and_loopback_tests_are_reached() {
        let g = Endpoint::giphy();
        let ok = |u: &str| g.allows(&Url::parse(u).unwrap());
        assert!(ok("https://api.giphy.com/v1/gifs/search"));
        assert!(ok("https://media3.giphy.com/media/a/giphy.mp4"));
        assert!(ok("https://i.giphy.com/a.webp"));
        assert!(!ok("http://api.giphy.com/v1/gifs/search"));
        assert!(!ok("https://giphy.com.evil.example/x"));
        assert!(!ok("https://media10.giphy.com/x"));
        assert!(!ok("https://user@media.giphy.com/x"));
        assert!(!ok("http://127.0.0.1:9/x"));
        let t = Endpoint::for_api("http://127.0.0.1:4000/v1/gifs/");
        assert!(t.loopback);
        assert_eq!(t.api, "http://127.0.0.1:4000/v1/gifs");
        assert!(t.allows(&Url::parse("http://127.0.0.1:4000/x").unwrap()));
        assert_eq!(
            Endpoint::for_api("http://example.com/v1"),
            Endpoint::giphy()
        );
    }

    #[test]
    fn the_key_never_reaches_logs() {
        let url = search_url(&Endpoint::giphy(), "SECRETKEY", "cats & dogs").unwrap();
        assert!(url.contains("api_key=SECRETKEY"));
        assert!(url.contains("q=cats+%26+dogs"));
        assert!(!redact(&url).contains("SECRETKEY"));
    }

    #[test]
    fn a_loopback_server_answers_like_giphy() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for status in ["200 OK", "401 Unauthorized"] {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let body = ANSWER.as_bytes();
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(body);
            }
        });
        let ep = Endpoint::for_api(&format!("http://127.0.0.1:{port}/v1/gifs"));
        let hits = search(&ep, "k", "cat").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(search(&ep, "bad", "cat"), Err(SearchError::BadKey));
        server.join().unwrap();
    }
}
