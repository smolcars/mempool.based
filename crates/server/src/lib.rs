//! HTTP surface of the mempool.based server.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::http::header::{
    CONTENT_SECURITY_POLICY, HeaderName, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router, middleware};
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};

/// Process configuration, read from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// `LISTEN_ADDR`, default `127.0.0.1:8080`.
    pub listen: SocketAddr,
    /// `WEB_DIR`: the built frontend, default `web/dist`.
    pub web_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let listen = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
        let listen = listen
            .parse()
            .map_err(|e| format!("LISTEN_ADDR {listen:?} is not a socket address: {e}"))?;
        let web_dir = std::env::var_os("WEB_DIR").map_or_else(|| "web/dist".into(), PathBuf::from);
        Ok(Self { listen, web_dir })
    }
}

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

/// The full application: `/healthz`, reserved API prefixes, and the frontend
/// from `web_dir` with `index.html` as the fallback for client-side routes.
pub fn router(web_dir: &Path) -> Router {
    // ServeDir rejects `..` and absolute components, so nothing outside web_dir is reachable.
    let frontend = ServeDir::new(web_dir).fallback(ServeFile::new(web_dir.join("index.html")));
    let mut app = Router::new().route("/healthz", get(healthz));
    // A wildcard needs a non-empty remainder, so the bare and trailing-slash roots are listed too.
    for path in [
        "/api",
        "/api/",
        "/api/{*rest}",
        "/app",
        "/app/",
        "/app/{*rest}",
    ] {
        app = app.route(path, any(api_not_found));
    }
    app.fallback_service(frontend)
        .layer(middleware::map_response(security_headers))
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

// API paths never fall through to the app shell: a typo'd endpoint must be a 404, not HTML.
async fn api_not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
}

async fn security_headers(mut res: Response) -> Response {
    let headers = res.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        headers.insert(name, HeaderValue::from_static(value));
    }
    res
}
