//! HTTP surface of the mempool.based server.
//!
//! [`App::new`] builds everything: per-network routes from [`network_routes`] mounted under each
//! enabled network's prefix, `/healthz`, the frontend, and the limits around all of it.

mod app_config;
pub mod config;
pub mod error;
pub mod esplora;
pub mod limits;
pub mod report;
pub mod request_log;
mod serve;

use std::future::Future;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, CACHE_CONTROL, CONTENT_SECURITY_POLICY,
    HeaderName, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router, middleware};
use node::esplora::Esplora;
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

/// API prefixes inside a network scope. Unmatched paths under them are a JSON 404, never the app
/// shell; inside an enabled network, `/api/{*rest}` is the Esplora pass-through, which does the same.
const API_PATHS: [&str; 6] = [
    "/api",
    "/api/",
    "/api/{*rest}",
    "/app",
    "/app/",
    "/app/{*rest}",
];

/// One enabled network. Handlers in [`network_routes`] receive it as `State<Arc<NetworkContext>>`;
/// later tasks add the other node clients and the engine here.
pub struct NetworkContext {
    pub config: NetworkConfig,
    pub esplora: Esplora,
    /// State of the `/api` pass-through.
    pub api: esplora::Api,
}

impl std::fmt::Debug for NetworkContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkContext")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl NetworkContext {
    fn new(config: &NetworkConfig, all: &Config) -> Result<Self, node::esplora::Error> {
        let url = match &config.esplora {
            config::EsploraEndpoint::Http(url) => url.clone(),
            config::EsploraEndpoint::Unix(path) => path.to_string_lossy().into_owned(),
        };
        Ok(Self {
            config: config.clone(),
            esplora: Esplora::new(&url)?,
            api: esplora::Api::new(all.broadcast, all.limits.max_tracked_clients),
        })
    }
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
        let mut app = Router::new()
            .route("/healthz", get(healthz))
            .merge(app_config::routes(config));
        for network in Network::ALL {
            let prefix = network.path_prefix();
            let context = config
                .networks
                .iter()
                .find(|n| n.network == network)
                // Cannot fail: `Config` checked the Esplora URL with the client's own parser.
                .and_then(|n| NetworkContext::new(n, config).ok());
            let enabled = context.is_some();
            let scope = match context {
                Some(ctx) => routes
                    .clone()
                    .route("/api/{*rest}", any(esplora::handle))
                    .route(
                        "/api/tx",
                        any(esplora::handle)
                            .layer(DefaultBodyLimit::max(esplora::MAX_BROADCAST_BODY)),
                    )
                    .with_state(Arc::new(ctx)),
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
                .filter(|p| !(enabled && **p == "/api/{*rest}"))
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
            .layer(middleware::map_response(finish))
            .layer(middleware::from_fn(cors))
            .layer(middleware::from_fn_with_state(
                request_log::RequestLogger::new(config),
                request_log::log_request,
            ));
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

/// Whether `path` is under some network's `/api`.
fn is_api(path: &str) -> bool {
    Network::ALL.iter().any(|n| {
        path.strip_prefix(n.path_prefix())
            .is_some_and(|rest| rest == "/api" || rest.starts_with("/api/"))
    })
}

/// `/api` is a public API that web wallets call from other origins. Nothing in it depends on
/// cookies or other ambient credentials, so any origin may read it, broadcast included. Outermost,
/// so refusals from the limits (429, 503, 504) are readable cross-origin too.
async fn cors(req: Request, next: Next) -> Response {
    if !is_api(req.uri().path()) {
        return next.run(req).await;
    }
    let preflight = req.method() == Method::OPTIONS;
    let mut res = if preflight {
        finish(StatusCode::NO_CONTENT.into_response()).await
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    if preflight {
        h.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, HEAD, POST"),
        );
        h.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Content-Type"),
        );
        h.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
    } else {
        h.insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(
                "X-Bitcoin-Chain-Tip, X-Bitcoin-Chain-Tip-Height, X-Request-Id",
            ),
        );
    }
    // Errors and refusals must not be cached: the same URL can succeed a moment later.
    if !h.contains_key(CACHE_CONTROL) {
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
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
