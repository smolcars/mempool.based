//! HTTP surface of the mempool.based server.
//!
//! [`App::new`] builds everything: per-network routes from [`network_routes`] mounted under each
//! enabled network's prefix, `/healthz` (liveness), `/readyz` (readiness, [`health`]), the
//! frontend, and the limits around all of it.

mod api_v1;
mod app_config;
mod audits;
pub mod blocks;
pub mod config;
pub mod difficulty;
pub mod error;
pub mod esplora;
pub mod health;
pub mod limits;
pub mod mining;
pub mod relatives;
pub mod replacements;
pub mod report;
pub mod request_log;
mod serve;
pub mod statistics;
pub mod summary;
mod v1_blocks;
pub mod ws;

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

pub use config::{Config, Limits, Network, NetworkConfig, WsLimits};
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
    /// The node's push channel; feeds [`ws::feed`].
    pub electrum: node::electrum::Client,
    /// Read-only JSON-RPC, shared by every caller: at most four calls in flight.
    pub rpc: node::rpc::Rpc,
    /// State of the block summary routes.
    pub summaries: summary::Summaries,
    pub hub: Arc<ws::Hub>,
    /// Followed transactions and addresses, and the replacement record.
    pub track: Arc<ws::Tracking>,
    pub sockets: Arc<ws::Sockets>,
    /// Kept current by [`difficulty::run`]; read by `/api/v1/difficulty-adjustment`.
    pub difficulty: difficulty::Difficulty,
    /// The block table, when `DATA_DIR` is set and the file opened.
    pub blocks: Option<blocks::BlockTable>,
    /// The mining routes' answers, kept current by [`mining::run`].
    pub mining: mining::Mining,
    /// What the last node check found, for `/readyz`; kept by [`health::watch`].
    pub node: health::NodeCheck,
    /// Fee recommendations and projected-block totals, kept current by `ws::fees`.
    pub fees: ws::fees::Fees,
    pub history: statistics::History,
    /// The mempool.space-shaped socket's own state (`ws::v1`).
    pub v1: Arc<ws::v1::Compat>,
    /// Esplora headers by hash for the `/api/v1` block routes.
    pub block_headers: v1_blocks::Headers,
}

impl std::fmt::Debug for NetworkContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkContext")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl NetworkContext {
    /// Starts the network's Electrum client, so it must run inside a tokio runtime.
    fn new(config: &NetworkConfig, all: &Config, sockets: &Arc<ws::Sockets>) -> Option<Self> {
        let url = match &config.esplora {
            config::EsploraEndpoint::Http(url) => url.clone(),
            config::EsploraEndpoint::Unix(path) => path.to_string_lossy().into_owned(),
        };
        let rpc = &config.rpc;
        let electrum = node::electrum::Client::spawn(node::electrum::Config::new(&config.electrum));
        let hub = Arc::new(ws::Hub::new(config.network, Arc::clone(sockets)));
        let track = Arc::new(ws::Tracking::new(
            config.network,
            Arc::clone(&hub),
            Some(electrum.clone()),
            config.mempool,
            &all.ws,
        ));
        let table = all
            .data_dir
            .as_deref()
            .and_then(|dir| blocks::BlockTable::open(dir, config, &url));
        Some(Self {
            blocks: table,
            rpc: node::rpc::Rpc::new(&rpc.host, rpc.port, &rpc.user, rpc.password.expose()).ok()?,
            summaries: summary::Summaries::new(
                all.limits.summary_cache_bytes,
                all.limits.max_tracked_clients,
            ),
            config: config.clone(),
            esplora: Esplora::new(&url).ok()?,
            api: esplora::Api::new(all.broadcast, &all.limits, &url).ok()?,
            electrum,
            hub,
            track,
            sockets: Arc::clone(sockets),
            difficulty: difficulty::Difficulty::default(),
            mining: mining::Mining::default(),
            node: health::NodeCheck::default(),
            fees: ws::fees::Fees::default(),
            history: statistics::History::default(),
            v1: Arc::new(ws::v1::Compat::new(Some(&url))),
            block_headers: v1_blocks::Headers::default(),
        })
    }
}

/// Routes every enabled network serves, written with paths starting `/api` or `/app`. They are
/// mounted at the root for mainnet and under `/signet`, `/testnet4` and `/regtest` for the others.
pub fn network_routes() -> Router<Arc<NetworkContext>> {
    Router::new()
        .route("/app/ws", get(ws::handle))
        .route("/api/v1/ws", get(ws::v1::handle))
        .route("/app/block/{hash}/summary", get(summary::facts))
        .route("/app/block/{hash}/txs", get(summary::txs))
        .route("/app/blocks/summary", get(summary::batch))
        .route("/app/tx/{txid}/relatives", get(relatives::route))
        .route("/app/tx/{txid}/replacements", get(replacements::route))
        .merge(statistics::app_routes())
        .merge(api_v1::routes())
        .merge(audits::routes())
}

/// The whole application, ready to serve.
#[derive(Debug, Clone)]
pub struct App {
    router: Router,
    limiter: Limiter,
    sockets: Arc<ws::Sockets>,
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
        let sockets = ws::Sockets::new(&config.ws, config.limits.request_timeout);
        let mut enabled_networks = Vec::new();
        for network in Network::ALL {
            let prefix = network.path_prefix();
            let context = config
                .networks
                .iter()
                .find(|n| n.network == network)
                // Cannot fail: `Config` checked the endpoints by the clients' own rules.
                .and_then(|n| NetworkContext::new(n, config, &sockets));
            let enabled = context.is_some();
            let scope = match context {
                Some(ctx) => {
                    let ctx = Arc::new(ctx);
                    enabled_networks.push(Arc::clone(&ctx));
                    tokio::spawn(health::watch(Arc::clone(&ctx)));
                    let events = ctx.electrum.events();
                    tokio::spawn(ws::feed(Arc::clone(&ctx), events));
                    tokio::spawn(summary::worker(Arc::clone(&ctx)));
                    tokio::spawn(difficulty::run(Arc::clone(&ctx), ctx.electrum.events()));
                    blocks::spawn(&ctx, config.backfill);
                    ws::mempool::start(&ctx);
                    tokio::spawn(mining::run(Arc::clone(&ctx)));
                    tokio::spawn(ws::v1::run(Arc::clone(&ctx)));
                    tokio::spawn(ws::track::follow_addresses(
                        Arc::clone(&ctx.track),
                        ctx.electrum.events(),
                        ctx.electrum.health(),
                    ));
                    report::report_electrum(network, &ctx.electrum);
                    routes
                        .clone()
                        .route("/api/{*rest}", any(esplora::handle))
                        .route(
                            "/api/tx",
                            any(esplora::handle)
                                .layer(DefaultBodyLimit::max(esplora::MAX_BROADCAST_BODY)),
                        )
                        .with_state(ctx)
                }
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
        let networks = Arc::new(enabled_networks);
        app = app.route(
            "/readyz",
            get(move || {
                let networks = Arc::clone(&networks);
                async move { health::readyz(&networks) }
            }),
        );
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
        Self {
            router,
            limiter,
            sockets,
        }
    }

    /// The router alone, for driving with `tower::ServiceExt::oneshot`. Requests must carry a
    /// `ConnectInfo<SocketAddr>` extension, as [`App::serve`] adds.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// WebSocket state shared by every network.
    pub fn sockets(&self) -> &Arc<ws::Sockets> {
        &self.sockets
    }

    /// Serves until `shutdown` resolves. Then open requests get the request timeout to finish,
    /// and open WebSockets are sent a close frame and get the same time, counted from the signal.
    pub async fn serve(self, listener: TcpListener, shutdown: impl Future<Output = ()>) {
        let sockets = self.sockets;
        let signal = async {
            shutdown.await;
            sockets.close_all();
        };
        serve::serve(listener, self.router, self.limiter, signal).await;
        sockets.drain().await;
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
                "X-Bitcoin-Chain-Tip, X-Bitcoin-Chain-Tip-Height, X-Request-Id, X-Data-From-Height, \
                 X-Data-From-Time, X-Data-Complete, X-Fee-Source",
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
/// An error is `no-store` unless its route said otherwise: the same URL can succeed a moment later.
async fn finish(res: Response) -> Response {
    let mut res = error::sanitize(res);
    let failed = res.status().is_client_error() || res.status().is_server_error();
    let headers = res.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        headers.insert(name, HeaderValue::from_static(value));
    }
    if failed && !headers.contains_key(CACHE_CONTROL) {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}
