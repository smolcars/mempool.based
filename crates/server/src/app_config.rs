//! `GET /app/config`: what the frontend needs before it can draw the shell. Server-wide, so it is
//! served at the root only, whichever networks are enabled.

use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::Config;

pub fn routes(config: &Config) -> Router {
    // The answer never changes while the process runs, so it is built once.
    let body = json!({
        "networks": config.networks.iter().map(|n| n.network.name()).collect::<Vec<_>>(),
        "version": env!("CARGO_PKG_VERSION"),
    });
    Router::new().route(
        "/app/config",
        get(move || async move { Json::<Value>(body) }),
    )
}
