//! The one error type every handler returns (SECURITY.md rule 17).

use axum::Json;
use axum::http::header::{CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Renders as `{"error": "<message>"}`. The message is `'static` on purpose: it can say what was
/// wrong with the request, but cannot carry upstream URLs, paths or error text formatted at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiError {
    status: StatusCode,
    message: &'static str,
}

impl ApiError {
    pub const NOT_FOUND: Self = Self::new(StatusCode::NOT_FOUND, "not found");
    pub const PAYLOAD_TOO_LARGE: Self = Self::new(StatusCode::PAYLOAD_TOO_LARGE, "body too large");
    pub const TOO_MANY_REQUESTS: Self =
        Self::new(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    pub const OVERLOADED: Self = Self::new(StatusCode::SERVICE_UNAVAILABLE, "server busy");
    pub const TIMEOUT: Self = Self::new(StatusCode::GATEWAY_TIMEOUT, "request timed out");
    /// The node failed or answered nonsense. Log the cause server-side; the client learns only this.
    pub const UPSTREAM: Self = Self::new(StatusCode::BAD_GATEWAY, "upstream unavailable");
    pub const INTERNAL: Self = Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error");

    /// A 400 explaining what is wrong with the request, e.g. `"invalid txid"`.
    pub const fn bad_request(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub const fn new(status: StatusCode, message: &'static str) -> Self {
        Self { status, message }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut res = (self.status, Json(json!({ "error": self.message }))).into_response();
        if matches!(
            self.status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        ) {
            res.headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("1"));
        }
        res
    }
}

/// Replaces any error response that did not come from [`ApiError`] (axum's plain-text extractor
/// rejections, empty 405s, file-server errors) with one that did, so no error body escapes the model.
pub fn sanitize(res: Response) -> Response {
    let is_error = res.status().is_client_error() || res.status().is_server_error();
    let is_json = res
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"application/json"));
    if !is_error || is_json {
        return res;
    }
    let message = match res.status() {
        StatusCode::NOT_FOUND => ApiError::NOT_FOUND.message,
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::PAYLOAD_TOO_LARGE.message,
        StatusCode::METHOD_NOT_ALLOWED => "method not allowed",
        s if s.is_client_error() => "bad request",
        _ => ApiError::INTERNAL.message,
    };
    let mut clean = ApiError::new(res.status(), message).into_response();
    // Keep headers a client needs to act on the error, e.g. Allow on a 405.
    for name in [axum::http::header::ALLOW, RETRY_AFTER] {
        if let Some(v) = res.headers().get(&name) {
            clean.headers_mut().insert(name, v.clone());
        }
    }
    clean
}
