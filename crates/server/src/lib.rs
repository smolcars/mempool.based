//! HTTP surface of the mempool.based server.
//!
//! [`App::new`] builds everything: per-network routes from [`network_routes`] mounted under each
//! enabled network's prefix, `/healthz`, the frontend, and the limits around all of it.

pub mod config;
pub mod error;
pub mod limits;
mod serve;

use std::future::Future;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::HeaderValue;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, HeaderName, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::response::Response;
use axum::routing::{any, get};
use axum::{Json, Router, middleware};
use serde_json::json;
use tokio::net::TcpListener;
use tower::Layer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

pub use config::{Config, Limits, Network, NetworkConfig};
pub use error::ApiError;
use limits::Limiter;

/// Sent on every response. The CSP is SECURITY.md rule 15.
pub const SECURITY_HEADERS: [(HeaderName, &str); 4] = [
    (
        CONTENT_SECURITY_POLICY,
        "default-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'",
    ),
    (X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (REFERRER_POLICY, "no-referrer"),
    (X_FRAME_OPTIONS, "DENY"),
];

/// Vite puts content-hashed files under `/assets/`, so a URL there never changes content.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// API prefixes inside a network scope. Unmatched paths under them are a JSON 404, never the app shell.
const API_PATHS: [&str; 6] = [
    "/api",
    "/api/",
    "/api/{*rest}",
    "/app",
    "/app/",
    "/app/{*rest}",
];

/// One enabled network. Handlers in [`network_routes`] receive it as `State<Arc<NetworkContext>>`;
/// later tasks add the node clients and engine here.
#[derive(Debug)]
pub struct NetworkContext {
    pub config: NetworkConfig,
}

/// Routes every enabled network serves, written with paths starting `/api` or `/app`. They are
/// mounted at the root for mainnet and under `/signet`, `/testnet4` and `/regtest` for the others.
pub fn network_routes() -> Router<Arc<NetworkContext>> {
    Router::new()
}

/// The whole application, ready to serve.
#[derive(Debug, Clone)]
pub struct App {
    router: Router,
    limiter: Limiter,
}

impl App {
    pub fn new(config: &Config) -> Self {
        Self::with_routes(config, network_routes())
    }

    /// Like [`App::new`] with other per-network routes. Tests use it to mount probe handlers.
    pub fn with_routes(config: &Config, routes: Router<Arc<NetworkContext>>) -> Self {
        let mut app = Router::new().route("/healthz", get(healthz));
        for network in Network::ALL {
            let prefix = network.path_prefix();
            let scope = match config.networks.iter().find(|n| n.network == network) {
                Some(n) => routes
                    .clone()
                    .with_state(Arc::new(NetworkContext { config: n.clone() })),
                None if prefix.is_empty() => Router::new(),
                None => {
                    // A disabled network has no pages either.
                    for path in [
                        prefix.to_owned(),
                        format!("{prefix}/"),
                        format!("{prefix}/{{*rest}}"),
                    ] {
                        app = app.route(&path, any(not_found));
                    }
                    continue;
                }
            };
            let scope = API_PATHS
                .iter()
                .fold(scope, |r, p| r.route(p, any(not_found)));
            app = if prefix.is_empty() {
                app.merge(scope)
            } else {
                app.nest(prefix, scope)
            };
        }
        // `/<anything>/api/...` is an API call for a network we do not know, never a page.
        for path in API_PATHS {
            app = app.route(&format!("/{{network}}{path}"), any(not_found));
        }

        let dir = &config.web_dir;
        let assets = ServeDir::new(dir.join("assets"))
            .precompressed_br()
            .precompressed_gzip();
        let shell = ServeFile::new(dir.join("index.html"))
            .precompressed_br()
            .precompressed_gzip();
        // ServeDir rejects `..` and absolute components, so nothing outside web_dir is reachable.
        let pages = ServeDir::new(dir)
            .precompressed_br()
            .precompressed_gzip()
            .fallback(shell);
        let cache =
            |v| SetResponseHeaderLayer::overriding(CACHE_CONTROL, HeaderValue::from_static(v));

        let limiter = Limiter::new(&config.limits);
        let router = app
            .nest_service("/assets", cache(IMMUTABLE).layer(assets))
            // The shell and unhashed files are revalidated on every load, so a deploy shows at once.
            .fallback_service(cache("no-cache").layer(pages))
            .layer(DefaultBodyLimit::max(config.limits.max_body_bytes))
            .layer(middleware::from_fn_with_state(
                limiter.clone(),
                limits::limit,
            ))
            .layer(middleware::map_response(finish));
        Self { router, limiter }
    }

    /// The router alone, for driving with `tower::ServiceExt::oneshot`. Requests must carry a
    /// `ConnectInfo<SocketAddr>` extension, as [`App::serve`] adds.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    pub async fn serve(self, listener: TcpListener, shutdown: impl Future<Output = ()>) {
        serve::serve(listener, self.router, self.limiter, shutdown).await;
    }
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

async fn not_found() -> ApiError {
    ApiError::NOT_FOUND
}

/// Last step for every response: errors through the error model, then the security headers.
async fn finish(res: Response) -> Response {
    let mut res = error::sanitize(res);
    let headers = res.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        headers.insert(name, HeaderValue::from_static(value));
    }
    res
}
