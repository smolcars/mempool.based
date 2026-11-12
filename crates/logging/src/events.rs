//! Every event the server can record. `docs/LOGGING.md` lists them all; a test fails when one
//! here is missing there.
//!
//! Field rules: snake_case, units in the name (`duration_ms`, `bytes`), the same name for the same
//! thing everywhere. An optional field that does not apply is left out of the line. No field may
//! hold a password, a credentialed URL, or a raw request path or query.

use serde::Serialize;

use crate::{Event, Level};

macro_rules! event {
    ($ty:ident $(<$lt:lifetime>)?, $name:literal, $level:ident) => {
        impl$(<$lt>)? Event for $ty $(<$lt>)? {
            const NAME: &'static str = $name;
            const LEVEL: Level = Level::$level;
        }
    };
}

// Lifecycle.

/// The server parsed its configuration and is about to listen. Non-secret settings only.
#[derive(Debug, Serialize)]
pub struct ServerStarting<'a> {
    pub version: &'a str,
    pub listen_addr: String,
    pub web_dir: String,
    pub networks: Vec<&'a str>,
    pub log_format: &'a str,
    pub log_level: &'a str,
    pub log_requests: &'a str,
    pub log_client_addr: bool,
    pub broadcast_enabled: bool,
    pub max_body_bytes: usize,
    pub request_timeout_secs: u64,
    pub max_connections: usize,
    pub max_concurrent_requests: usize,
    pub client_max_connections: u32,
    pub client_max_concurrent_requests: u32,
    pub client_rate_per_sec: u32,
    pub client_burst: u32,
    pub max_tracked_clients: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_proxy_header: Option<&'a str>,
    pub ws_max_sockets: usize,
    pub ws_client_max_sockets: u32,
    pub ws_max_message_bytes: usize,
    pub ws_messages_per_sec: u32,
    pub ws_max_subscriptions: usize,
    pub ws_send_queue: usize,
    pub ws_idle_timeout_secs: u64,
    pub block_summary_cache_mb: usize,
}
event!(ServerStarting<'a>, "server_starting", Info);

/// One network is enabled, with the node endpoints it uses. The RPC password is never here and
/// the Esplora URL cannot hold credentials (config refuses them).
#[derive(Debug, Serialize)]
pub struct NetworkEnabled<'a> {
    pub network: &'a str,
    /// `host:port` of the JSON-RPC listener.
    pub rpc_addr: String,
    /// Esplora base URL or unix socket path.
    pub esplora: String,
    pub electrum_addr: &'a str,
}
event!(NetworkEnabled<'a>, "network_enabled", Info);

/// No network is configured: only the frontend and `/healthz` are served.
#[derive(Debug, Serialize)]
pub struct NoNetworks {}
event!(NoNetworks, "no_networks", Warn);

/// `WEB_DIR` has no `index.html`, so pages will 404.
#[derive(Debug, Serialize)]
pub struct WebDirMissing {
    pub web_dir: String,
}
event!(WebDirMissing, "web_dir_missing", Warn);

/// The listener is bound; `addr` shows the real port when `LISTEN_ADDR` asked for port 0.
#[derive(Debug, Serialize)]
pub struct Listening {
    pub addr: String,
}
event!(Listening, "listening", Info);

/// A shutdown signal arrived; in-flight requests get up to the request timeout to finish.
#[derive(Debug, Serialize)]
pub struct ShutdownRequested {
    /// `sigterm` or `ctrl_c`.
    pub signal: &'static str,
}
event!(ShutdownRequested, "shutdown_requested", Info);

/// The server stopped serving and the process is exiting normally.
#[derive(Debug, Serialize)]
pub struct ServerStopped {}
event!(ServerStopped, "server_stopped", Info);

/// The configuration is invalid; the process exits. `error` names the variable, never its value.
#[derive(Debug, Serialize)]
pub struct ConfigInvalid<'a> {
    pub error: &'a str,
}
event!(ConfigInvalid<'a>, "config_invalid", Error);

/// The listen address could not be bound; the process exits.
#[derive(Debug, Serialize)]
pub struct ListenFailed {
    pub addr: String,
    pub error: String,
}
event!(ListenFailed, "listen_failed", Error);

/// Code panicked. In a request handler the client gets a 500 and the server keeps running.
#[derive(Debug, Serialize)]
pub struct Panic<'a> {
    pub message: &'a str,
    /// `file:line` in our source.
    pub location: &'a str,
    pub thread: &'a str,
}
event!(Panic<'a>, "panic", Error);

/// The log queue was full and this many events were dropped. Written when it drains.
#[derive(Debug, Serialize)]
pub struct LogEventsDropped {
    pub count: u64,
}
event!(LogEventsDropped, "log_events_dropped", Warn);

// HTTP.

/// One HTTP request, written when its response head is ready.
#[derive(Debug, Serialize)]
pub struct HttpRequest<'a> {
    /// Also sent to the client in the `X-Request-Id` response header.
    pub request_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<&'static str>,
    pub method: &'a str,
    /// The matched route template, e.g. `/signet/api/{*rest}`; `frontend` for pages and files.
    /// Never the raw path: that would record which address or transaction was looked up.
    pub route: &'a str,
    pub status: u16,
    /// Until the response head was ready; body streaming is not included.
    pub duration_ms: f64,
    /// Response body size, when known up front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// Which limit refused the request, if one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<&'static str>,
    /// `/api` pass-through: the named Esplora route, e.g. `tx_status`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<&'static str>,
    /// `/api` pass-through: `upstream`, `coalesced` or `hot`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    /// `/api` pass-through: time of this request's own upstream call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_ms: Option<f64>,
    /// `/api` pass-through: why the upstream call got no answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_error: Option<&'static str>,
    /// The address the limiter keys on (IPv6 as its /64). Only with `LOG_CLIENT_ADDR=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}
event!(HttpRequest<'a>, "http_request", Info);

// Node clients.

/// The Electrum connection is up: handshake done, subscriptions restored.
#[derive(Debug, Serialize)]
pub struct ElectrumConnected<'a> {
    pub network: &'static str,
    /// The node's `server.version` software string.
    pub server: &'a str,
    pub protocol: &'a str,
}
event!(ElectrumConnected<'a>, "electrum_connected", Info);

/// A ready Electrum connection was lost; the client reconnects with backoff.
#[derive(Debug, Serialize)]
pub struct ElectrumDisconnected {
    pub network: &'static str,
}
event!(ElectrumDisconnected, "electrum_disconnected", Warn);

/// Connection attempts keep failing. Written at the 1st, 2nd, 4th, 8th… failure in a row.
#[derive(Debug, Serialize)]
pub struct ElectrumUnreachable {
    pub network: &'static str,
    pub attempts: u32,
}
event!(ElectrumUnreachable, "electrum_unreachable", Warn);

/// Updates may have been missed (a reconnect, or a tip jump too far to follow); derived state
/// is refreshed.
#[derive(Debug, Serialize)]
pub struct ElectrumResynced {
    pub network: &'static str,
}
event!(ElectrumResynced, "electrum_resynced", Info);

/// Lines from the node that were not valid JSON-RPC and were skipped. Written when the total
/// reaches 1, 2, 4, 8…
#[derive(Debug, Serialize)]
pub struct ElectrumLinesSkipped {
    pub network: &'static str,
    pub total: u64,
}
event!(ElectrumLinesSkipped, "electrum_lines_skipped", Warn);

/// Blocks above `fork_height` left the best chain.
#[derive(Debug, Serialize)]
pub struct ChainReorg {
    pub network: &'static str,
    pub fork_height: u32,
}
event!(ChainReorg, "chain_reorg", Warn);

// WebSocket hub.

/// A socket closed. Debug level: one per socket. Clients dropped for a limit are also summarised
/// by [`WsClientsDropped`] at the default level.
#[derive(Debug, Serialize)]
pub struct WsClosed {
    pub network: &'static str,
    /// The close code sent, or 1006 when the connection dropped without a close frame.
    pub code: u16,
    /// Why, as a fixed label: `client_closed`, `shutdown`, `too_slow`, `idle`, …
    pub reason: &'static str,
    /// How long the socket was open.
    pub duration_ms: f64,
    /// Sockets still open, on every network.
    pub open: usize,
}
event!(WsClosed, "ws_closed", Debug);

/// Clients disconnected for breaking one limit. Written when the count for that reason on that
/// network reaches 1, 2, 4, 8…
#[derive(Debug, Serialize)]
pub struct WsClientsDropped {
    pub network: &'static str,
    /// `too_slow`, `idle`, `rate`, `too_big`, `bad_message`, `too_many_subscriptions`,
    /// `protocol_error`, `binary` or `invalid_utf8`.
    pub reason: &'static str,
    pub total: u64,
}
event!(WsClientsDropped, "ws_clients_dropped", Warn);

/// One message sent to every subscriber of a topic.
#[derive(Debug, Serialize)]
pub struct WsFanout {
    pub network: &'static str,
    pub topic: &'static str,
    /// The message's `type`: `block`, `summary`, `reorg`, `resync`.
    pub kind: &'static str,
    pub subscribers: usize,
    /// Sockets open on every network.
    pub open: usize,
    /// Frame size; every subscriber shares these bytes.
    pub bytes: usize,
    /// Subscribers whose send queue was full, now being disconnected.
    pub dropped_slow: usize,
    /// From the node's notification to the last subscriber's queue, upstream fetch included.
    pub duration_ms: f64,
    /// The queueing part of `duration_ms`.
    pub fanout_ms: f64,
}
event!(WsFanout, "ws_fanout", Info);

/// A new block's JSON could not be fetched from Esplora, so subscribers were told to resync.
/// Written at the 1st, 2nd, 4th, 8th… failure in a row.
#[derive(Debug, Serialize)]
pub struct BlockUnavailable {
    pub network: &'static str,
    pub height: u32,
    pub hash: String,
    pub failures: u32,
}
event!(BlockUnavailable, "block_unavailable", Warn);

// Block summaries.

/// One block's summary was computed from the node. Debug: up to a few a second under load.
#[derive(Debug, Serialize)]
pub struct BlockSummaryComputed {
    pub network: &'static str,
    pub height: u32,
    /// Transactions in the block, the coinbase included.
    pub txs: usize,
    /// The block's serialized size.
    pub block_bytes: u64,
    /// Size of the per-transaction records kept in the cache.
    pub bytes: usize,
    /// The `getblock` call alone.
    pub node_ms: f64,
    /// From the start, the wait for a computation slot included.
    pub duration_ms: f64,
    /// `request` (a client asked) or `new_block` (computed ahead of demand).
    pub cause: &'static str,
}
event!(BlockSummaryComputed, "block_summary_computed", Debug);

/// Computing summaries failed at the node. Written at the 1st, 2nd, 4th, 8th… failure in a row.
#[derive(Debug, Serialize)]
pub struct BlockSummaryFailed {
    pub network: &'static str,
    /// `busy`, `timeout`, `unavailable`, `response_too_big` or `decode`.
    pub error: &'static str,
    pub failures: u32,
}
event!(BlockSummaryFailed, "block_summary_failed", Warn);

/// A summary was computed again after failures.
#[derive(Debug, Serialize)]
pub struct BlockSummariesRecovered {
    pub network: &'static str,
    pub failures: u32,
}
event!(BlockSummariesRecovered, "block_summaries_recovered", Info);

/// Requests for uncached summaries were refused. Written when the count for that reason on that
/// network reaches 1, 2, 4, 8…
#[derive(Debug, Serialize)]
pub struct BlockSummariesRefused {
    pub network: &'static str,
    /// `summary_busy` (the network's queue is full) or `route_rate` (one client's limit).
    pub reason: &'static str,
    pub total: u64,
}
event!(BlockSummariesRefused, "block_summaries_refused", Warn);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::time::UNIX_EPOCH;

    use serde_json::Value;

    use super::*;
    use crate::Format;

    /// One line per event type, in both formats.
    fn samples() -> Vec<(&'static str, Vec<u8>, Vec<u8>)> {
        fn both<E: Event>(e: &E) -> (&'static str, Vec<u8>, Vec<u8>) {
            let at = UNIX_EPOCH;
            (
                E::NAME,
                crate::format::line(e, Format::Json, at),
                crate::format::line(e, Format::Text, at),
            )
        }
        let n = "signet";
        vec![
            both(&ServerStarting {
                version: "0.1.0",
                listen_addr: "127.0.0.1:0".into(),
                web_dir: "web/dist".into(),
                networks: vec![n],
                log_format: "json",
                log_level: "info",
                log_requests: "all",
                log_client_addr: false,
                broadcast_enabled: true,
                max_body_bytes: 1,
                request_timeout_secs: 1,
                max_connections: 1,
                max_concurrent_requests: 1,
                client_max_connections: 1,
                client_max_concurrent_requests: 1,
                client_rate_per_sec: 1,
                client_burst: 1,
                max_tracked_clients: 1,
                trusted_proxy_header: Some("x-real-ip"),
                ws_max_sockets: 1,
                ws_client_max_sockets: 1,
                ws_max_message_bytes: 1,
                ws_messages_per_sec: 1,
                ws_max_subscriptions: 1,
                ws_send_queue: 1,
                ws_idle_timeout_secs: 1,
                block_summary_cache_mb: 1,
            }),
            both(&NetworkEnabled {
                network: n,
                rpc_addr: "node.example:1".into(),
                esplora: "http://node.example:2".into(),
                electrum_addr: "node.example:3",
            }),
            both(&NoNetworks {}),
            both(&WebDirMissing {
                web_dir: "web/dist".into(),
            }),
            both(&Listening {
                addr: "127.0.0.1:1".into(),
            }),
            both(&ShutdownRequested { signal: "sigterm" }),
            both(&ServerStopped {}),
            both(&ConfigInvalid { error: "bad" }),
            both(&ListenFailed {
                addr: "127.0.0.1:1".into(),
                error: "in use".into(),
            }),
            both(&Panic {
                message: "boom",
                location: "a.rs:1",
                thread: "main",
            }),
            both(&LogEventsDropped { count: 3 }),
            both(&HttpRequest {
                request_id: "00ff",
                network: Some(n),
                method: "GET",
                route: "/signet/api/{*rest}",
                status: 200,
                duration_ms: 0.412,
                bytes: Some(10),
                refused: None,
                endpoint: Some("tx_status"),
                source: Some("upstream"),
                upstream_ms: Some(0.3),
                upstream_error: None,
                client: None,
            }),
            both(&ElectrumConnected {
                network: n,
                server: "rbitcoin",
                protocol: "1.6",
            }),
            both(&ElectrumDisconnected { network: n }),
            both(&ElectrumUnreachable {
                network: n,
                attempts: 4,
            }),
            both(&ElectrumResynced { network: n }),
            both(&ElectrumLinesSkipped {
                network: n,
                total: 2,
            }),
            both(&ChainReorg {
                network: n,
                fork_height: 9,
            }),
            both(&WsClosed {
                network: n,
                code: 4003,
                reason: "too_slow",
                duration_ms: 1500.0,
                open: 7,
            }),
            both(&WsClientsDropped {
                network: n,
                reason: "rate",
                total: 4,
            }),
            both(&WsFanout {
                network: n,
                topic: "blocks",
                kind: "block",
                subscribers: 2,
                open: 3,
                bytes: 1200,
                dropped_slow: 0,
                duration_ms: 4.2,
                fanout_ms: 0.05,
            }),
            both(&BlockUnavailable {
                network: n,
                height: 9,
                hash: "00".repeat(32),
                failures: 1,
            }),
            both(&BlockSummaryComputed {
                network: n,
                height: 9,
                txs: 4200,
                block_bytes: 1_600_000,
                bytes: 84_008,
                node_ms: 640.5,
                duration_ms: 650.1,
                cause: "request",
            }),
            both(&BlockSummaryFailed {
                network: n,
                error: "timeout",
                failures: 2,
            }),
            both(&BlockSummariesRecovered {
                network: n,
                failures: 3,
            }),
            both(&BlockSummariesRefused {
                network: n,
                reason: "summary_busy",
                total: 8,
            }),
        ]
    }

    /// Names declared in this file, read from its source so a new event cannot be missed.
    fn declared() -> Vec<&'static str> {
        include_str!("events.rs")
            .lines()
            .filter_map(|l| l.strip_prefix("event!("))
            .filter_map(|l| l.split('"').nth(1))
            .collect()
    }

    #[test]
    fn every_event_has_a_sample_and_a_unique_name() {
        let mut names: Vec<_> = samples().iter().map(|s| s.0).collect();
        let mut declared = declared();
        names.sort_unstable();
        declared.sort_unstable();
        assert_eq!(names, declared, "add a sample for each new event");
        names.dedup();
        assert_eq!(names.len(), declared.len(), "event names must be unique");
        for name in names {
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{name} is not snake_case"
            );
        }
    }

    #[test]
    fn every_event_is_one_json_line_with_the_required_keys() {
        for (name, json, text) in samples() {
            let json = String::from_utf8(json).unwrap();
            assert_eq!(json.matches('\n').count(), 1, "{json}");
            let v: Value = serde_json::from_str(&json).unwrap();
            let prefix = format!(
                r#"{{"ts":"1970-01-01T00:00:00.000Z","level":"{}","event":"{name}""#,
                v["level"].as_str().unwrap()
            );
            assert!(json.starts_with(&prefix), "{json}");
            // A field named like a header key would repeat it and make the line ambiguous.
            for key in ["\"ts\":", "\"level\":", "\"event\":"] {
                assert_eq!(json.matches(key).count(), 1, "{json}");
            }
            let text = String::from_utf8(text).unwrap();
            assert_eq!(text.matches('\n').count(), 1, "{text}");
            assert!(text.contains(&format!(" {name}")), "{text}");
        }
    }

    #[test]
    fn every_event_is_in_the_catalogue() {
        let doc = include_str!("../../../docs/LOGGING.md");
        for name in declared() {
            assert!(
                doc.contains(&format!("| `{name}` |")),
                "{name} is missing from the catalogue in docs/LOGGING.md"
            );
        }
    }
}
