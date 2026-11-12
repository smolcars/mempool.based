//! One `http_request` event per request, and an `X-Request-Id` on every response
//! (`docs/LOGGING.md`).
//!
//! The event names the matched route template, never the raw path or query: a path says which
//! address or transaction someone looked up, and the log must not become a record of that.

use std::hash::BuildHasher;
use std::hash::RandomState;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::body::HttpBody;
use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use logging::events::HttpRequest;

use crate::config::{Config, Network, RequestLog};
use crate::esplora::{ApiOutcome, Source};
use crate::limits::{Refused, client_addr};

pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Route label for everything the frontend fallback serves: pages, the app shell, static files.
const FRONTEND: &str = "frontend";

/// Settings for [`log_request`].
#[derive(Debug, Clone)]
pub struct RequestLogger {
    mode: RequestLog,
    client_addr: bool,
    trusted_proxy_header: Option<HeaderName>,
}

impl RequestLogger {
    pub fn new(config: &Config) -> Self {
        Self {
            mode: config.log_requests,
            client_addr: config.log_client_addr,
            trusted_proxy_header: config.limits.trusted_proxy_header.clone(),
        }
    }
}

/// Outermost middleware: times the request, tags the response with its id and records it.
pub async fn log_request(State(log): State<RequestLogger>, req: Request, next: Next) -> Response {
    let started = Instant::now();
    let id = request_id();
    let method = req.method().clone();
    let route = req.extensions().get::<MatchedPath>().cloned();
    let route = route.as_ref().map_or(FRONTEND, MatchedPath::as_str);
    let network = network(req.uri().path(), route);
    let client = log
        .client_addr
        .then(|| {
            let peer = req.extensions().get::<ConnectInfo<SocketAddr>>()?.0;
            let addr = client_addr(req.headers(), peer.ip(), log.trusted_proxy_header.as_ref());
            Some(addr.to_string())
        })
        .flatten();

    let mut res = next.run(req).await;
    if let Ok(v) = HeaderValue::from_str(&id) {
        res.headers_mut().insert(REQUEST_ID, v);
    }
    let status = res.status();
    let failed = status.is_client_error() || status.is_server_error();
    let wanted = match log.mode {
        RequestLog::All => true,
        RequestLog::Failures => failed,
        RequestLog::Off => false,
    };
    // Health checks poll constantly; only a failing one is news.
    if !wanted || (route == "/healthz" && !failed) {
        return res;
    }
    let outcome = res.extensions().get::<ApiOutcome>().copied();
    let (source, upstream_ms) = match outcome.map(|o| o.source) {
        Some(Source::Upstream(d)) => (Some("upstream"), Some(ms(d.as_micros()))),
        Some(Source::Coalesced) => (Some("coalesced"), None),
        Some(Source::Hot) => (Some("hot"), None),
        Some(Source::Cached) => (Some("cached"), None),
        None => (None, None),
    };
    logging::emit(&HttpRequest {
        request_id: &id,
        network,
        method: method.as_str(),
        route,
        status: status.as_u16(),
        duration_ms: ms(started.elapsed().as_micros()),
        bytes: res.body().size_hint().exact(),
        refused: res.extensions().get::<Refused>().map(|r| r.reason),
        endpoint: outcome.map(|o| o.route),
        source,
        upstream_ms,
        upstream_error: outcome.and_then(|o| o.upstream_error),
        client,
    });
    res
}

/// Milliseconds with microsecond precision.
fn ms(micros: u128) -> f64 {
    u64::try_from(micros).unwrap_or(u64::MAX) as f64 / 1000.0
}

/// Unique within the process and unguessable across restarts: a counter through a randomly
/// keyed SipHash, as 16 hex digits.
fn request_id() -> String {
    static KEYS: OnceLock<RandomState> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}", KEYS.get_or_init(RandomState::new).hash_one(n))
}

/// The network a request was for. The raw path is only matched against fixed prefixes, so
/// nothing from it reaches the log.
fn network(path: &str, route: &str) -> Option<&'static str> {
    if route == "/healthz" || path.starts_with("/assets/") || route.starts_with("/{network}") {
        return None;
    }
    let under = |prefix: &str| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    let network = Network::ALL
        .into_iter()
        .find(|n| !n.path_prefix().is_empty() && under(n.path_prefix()))
        .unwrap_or(Network::Mainnet);
    Some(network.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn networks_come_from_fixed_prefixes() {
        assert_eq!(network("/api/tx/ab", "/api/{*rest}"), Some("mainnet"));
        assert_eq!(network("/", FRONTEND), Some("mainnet"));
        assert_eq!(
            network("/signet/api/x", "/signet/api/{*rest}"),
            Some("signet")
        );
        assert_eq!(network("/testnet4", FRONTEND), Some("testnet4"));
        assert_eq!(network("/signetx/tx", FRONTEND), Some("mainnet"));
        assert_eq!(network("/healthz", "/healthz"), None);
        assert_eq!(network("/assets/a.js", FRONTEND), None);
        assert_eq!(network("/foo/api/x", "/{network}/api/{*rest}"), None);
    }

    #[test]
    fn request_ids_are_distinct_hex() {
        let a = request_id();
        let b = request_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
    }
}
