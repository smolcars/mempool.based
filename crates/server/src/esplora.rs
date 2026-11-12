//! The Esplora pass-through at `/api` (`docs/API.md`).
//!
//! Every request is parsed into a typed [`Route`] before anything reaches the node, and the
//! upstream path, query and body are written from that value alone (SECURITY.md rules 1–3).
//! Identical reads share one upstream call and the hottest tip-scoped answers are reused for
//! [`HOT_TTL`] (rule 8). Broadcast is the only write (rule 5).

use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Extension;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Request, State};
use axum::http::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use bitcoin::address::NetworkUnchecked;
use bitcoin::{Address, BlockHash, Transaction, Txid};
use node::esplora::{
    BlockTxsStart, Error, OutPosition, OutPositions, RawResponse, Route, ScriptHash, ScriptRef,
};
use tokio::sync::OnceCell;

use crate::config::Network;
use crate::limits::{Client, Clients};
use crate::{ApiError, IMMUTABLE, NetworkContext};

/// How long the hot tip-scoped answers ([`is_hot`]) are reused.
pub const HOT_TTL: Duration = Duration::from_secs(1);
/// Everything that is not content-addressed may change with the next block or mempool update.
pub const LIVE: &str = "public, max-age=1";
/// Largest transaction broadcast accepts: no standard transaction weighs more than 400,000
/// weight units, and a transaction's size in bytes never exceeds its weight.
pub const MAX_TX_BYTES: usize = 400_000;
/// Body limit of `POST /api/tx`. Hex doubles the size; the slack covers a trailing newline or two.
pub const MAX_BROADCAST_BODY: usize = 2 * MAX_TX_BYTES + 16;
/// Broadcasts per client: a burst of this many, then one per [`BROADCAST_EVERY`].
pub const BROADCAST_BURST: u32 = 5;
pub const BROADCAST_EVERY: Duration = Duration::from_secs(10);
/// The node takes at most this many txids on `/txs/outspends`.
pub const MAX_OUTSPENDS_TXIDS: usize = 50;

const CHAIN_TIP: HeaderName = HeaderName::from_static("x-bitcoin-chain-tip");
const CHAIN_TIP_HEIGHT: HeaderName = HeaderName::from_static("x-bitcoin-chain-tip-height");

/// Per-network state of the pass-through, held in [`NetworkContext`].
#[derive(Debug)]
pub struct Api {
    flights: Flights,
    /// `None` when broadcast is switched off (`BROADCAST_ENABLED=false`).
    broadcast: Option<Arc<Clients>>,
}

impl Api {
    pub fn new(broadcast: bool, max_tracked_clients: usize) -> Self {
        let rate = 1.0 / BROADCAST_EVERY.as_secs_f64();
        Self {
            flights: Flights::default(),
            broadcast: broadcast
                .then(|| Clients::with_rate(rate, BROADCAST_BURST, max_tracked_clients)),
        }
    }
}

/// What the pass-through did for one request that reached the node, attached to the response as
/// an extension for the request log. Requests refused before that carry none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiOutcome {
    /// The matched route, for example `tx_status`.
    pub route: &'static str,
    pub source: Source,
    /// Why the upstream call failed, when it did not get an answer: `timeout`, `unavailable`,
    /// `response_too_big` or `decode`. A non-200 answer from the node is not a failure here.
    pub upstream_error: Option<&'static str>,
}

/// Where a response's upstream answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// This request made the one upstream call, which took this long.
    Upstream(Duration),
    /// It joined an identical call already in flight: no upstream call of its own.
    Coalesced,
    /// It got a hot answer younger than [`HOT_TTL`]: no upstream call.
    Hot,
}

/// What a request asks for, once its path and query have parsed.
#[derive(Debug, PartialEq)]
enum Call {
    Read(Route),
    /// `POST /tx`; the body is parsed by the handler.
    Broadcast,
    /// `POST /block-heights/txs/outs`; the body is parsed by the handler.
    Outputs {
        asof: Option<BlockHash>,
    },
}

/// Handles everything under `/api/` that no other route claimed.
pub async fn handle(
    State(ctx): State<Arc<NetworkContext>>,
    Extension(client): Extension<Client>,
    req: Request,
) -> Response {
    match serve(&ctx, &client, req).await {
        Ok(res) => res,
        Err(e) => e.into_response(),
    }
}

async fn serve(ctx: &NetworkContext, client: &Client, req: Request) -> Result<Response, ApiError> {
    let path = req
        .uri()
        .path()
        .strip_prefix("/api/")
        .ok_or(ApiError::NOT_FOUND)?;
    let call = parse(path, req.uri().query(), ctx.config.network)?;
    let reading = matches!(call, Call::Read(_));
    let method = req.method();
    let allowed = if reading {
        method == Method::GET || method == Method::HEAD
    } else {
        method == Method::POST
    };
    if !allowed {
        let mut res =
            ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response();
        let allow = if reading { "GET, HEAD" } else { "POST" };
        res.headers_mut()
            .insert(ALLOW, HeaderValue::from_static(allow));
        return Ok(res);
    }
    match call {
        Call::Read(route) => Ok(read(ctx, route).await),
        Call::Outputs { asof } => {
            let body = read_body(req).await?;
            let positions: Vec<[u32; 3]> = serde_json::from_slice(&body).map_err(|_| {
                ApiError::bad_request("body must be a JSON array of [height, index, vout]")
            })?;
            let positions = positions
                .into_iter()
                .map(|[height, index, vout]| OutPosition {
                    height,
                    index,
                    vout,
                })
                .collect();
            let positions = OutPositions::new(positions).ok_or(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "at most 300 positions",
            ))?;
            Ok(read(ctx, Route::OutputsAt { positions, asof }).await)
        }
        Call::Broadcast => {
            let limiter = ctx.api.broadcast.as_ref().ok_or(ApiError::new(
                StatusCode::FORBIDDEN,
                "broadcast is disabled on this server",
            ))?;
            if let Err(e) = limiter.take(client.addr) {
                let mut res = e.into_response();
                res.headers_mut()
                    .insert(RETRY_AFTER, HeaderValue::from(BROADCAST_EVERY.as_secs()));
                return Ok(res);
            }
            let body = read_body(req).await?;
            let route = Route::Broadcast(parse_tx(&body)?);
            let started = Instant::now();
            let answer = ctx.esplora.raw(&route).await;
            Ok(respond(&route, answer, Source::Upstream(started.elapsed())))
        }
    }
}

/// The body, up to the route's `DefaultBodyLimit`: `MAX_BODY_BYTES`, or [`MAX_BROADCAST_BODY`].
async fn read_body(req: Request) -> Result<Bytes, ApiError> {
    Bytes::from_request(req, &()).await.map_err(|r| {
        if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::PAYLOAD_TOO_LARGE
        } else {
            ApiError::bad_request("could not read the request body")
        }
    })
}

/// Raw transaction hex, optionally surrounded by whitespace, holding exactly one transaction.
fn parse_tx(body: &[u8]) -> Result<Transaction, ApiError> {
    let bad = || ApiError::bad_request("body must be one raw transaction in hex");
    let hex = std::str::from_utf8(body).map_err(|_| bad())?.trim();
    if hex.len() > 2 * MAX_TX_BYTES {
        return Err(ApiError::PAYLOAD_TOO_LARGE);
    }
    bitcoin::consensus::encode::deserialize_hex(hex).map_err(|_| bad())
}

async fn read(ctx: &NetworkContext, route: Route) -> Response {
    let mut key = String::with_capacity(160);
    route.write_path(&mut key);
    if let Some((body, _)) = route.body() {
        // Only the outputs batch gets here; its body is JSON we wrote ourselves.
        key.push('\n');
        key.push_str(&String::from_utf8_lossy(&body));
    }
    let ttl = if is_hot(&route) {
        HOT_TTL
    } else {
        Duration::ZERO
    };
    let (answer, source) = ctx.api.flights.run(key, ttl, ctx.esplora.raw(&route)).await;
    respond(&route, answer, source)
}

/// Fixed-key answers that every page polls and that cost the node 200–300 ms or more.
fn is_hot(route: &Route) -> bool {
    matches!(
        route,
        Route::TipHeight
            | Route::TipHash
            | Route::Blocks { start_height: None }
            | Route::Mempool
            | Route::MempoolTxids
            | Route::MempoolRecent
            | Route::FeeEstimates
    )
}

/// The node's answer as our response, with an [`ApiOutcome`] for the request log.
fn respond(route: &Route, answer: Answer, source: Source) -> Response {
    let upstream_error = answer.as_ref().err().map(|e| match e {
        Error::Timeout => "timeout",
        Error::ResponseTooBig => "response_too_big",
        Error::Decode(_) => "decode",
        _ => "unavailable",
    });
    let mut res = forward(route, answer).unwrap_or_else(IntoResponse::into_response);
    res.extensions_mut().insert(ApiOutcome {
        route: route_name(route),
        source,
        upstream_error,
    });
    res
}

/// Same status, body bytes and content type as the node for a 200; our own fixed message for
/// anything else (SECURITY.md rule 17).
fn forward(route: &Route, answer: Answer) -> Result<Response, ApiError> {
    let raw = answer.map_err(|e| upstream_error(&e, route))?;
    if raw.status != StatusCode::OK {
        if raw.status == StatusCode::SERVICE_UNAVAILABLE
            && matches!(
                Error::from_status(raw.status, &raw.body),
                Error::Unavailable(_)
            )
        {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "node unavailable",
            ));
        }
        return Err(upstream_error(
            &Error::from_status(raw.status, &raw.body),
            route,
        ));
    }
    let cache = cache_control(route, &raw.body);
    let mut res = Response::new(Body::from(raw.body));
    let h = res.headers_mut();
    if let Some(ct) = raw.content_type {
        h.insert(CONTENT_TYPE, ct);
    }
    if let Some(tip) = raw.tip {
        if let Ok(v) = HeaderValue::from_str(&tip.hash.to_string()) {
            h.insert(CHAIN_TIP, v);
        }
        h.insert(CHAIN_TIP_HEIGHT, HeaderValue::from(tip.height));
    }
    h.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    Ok(res)
}

/// Immutable only for answers fully determined by the hash in the URL, with no status in them.
fn cache_control(route: &Route, body: &[u8]) -> &'static str {
    match route {
        Route::Block(_)
        | Route::BlockHeader(_)
        | Route::BlockRaw(_)
        | Route::BlockTxids(_)
        | Route::BlockTxid { .. } => IMMUTABLE,
        // A txid does not commit to the witness, so a segwit transaction's bytes can still change
        // (a witness replacement, or a reorg). Only a transaction without a witness is fixed.
        // Byte 4 of a witness serialization is the 0x00 marker; otherwise it is the input count.
        Route::TxRaw(_) if body.get(4).is_some_and(|&b| b != 0) => IMMUTABLE,
        Route::TxHex(_) if body.get(8..10).is_some_and(|b| b != b"00") => IMMUTABLE,
        Route::Broadcast(_) => "no-store",
        _ => LIVE,
    }
}

fn upstream_error(e: &Error, route: &Route) -> ApiError {
    let status = |s: StatusCode, m| ApiError::new(s, m);
    match e {
        Error::NotFound => ApiError::NOT_FOUND,
        Error::AddressIndexDisabled => status(
            StatusCode::SERVICE_UNAVAILABLE,
            "address index is disabled on this server",
        ),
        Error::TooLarge => status(
            StatusCode::SERVICE_UNAVAILABLE,
            "address history too large for this route; use the paged /txs routes",
        ),
        Error::ChainViewMoved => status(
            StatusCode::SERVICE_UNAVAILABLE,
            "chain tip moved during the request; retry",
        ),
        Error::Rejected { message, .. } if matches!(route, Route::Broadcast(_)) => {
            ApiError::bad_request(reject_reason(message))
        }
        Error::Rejected { status: s, .. } if *s == StatusCode::UNPROCESSABLE_ENTITY => {
            status(*s, "after_txid not found")
        }
        Error::Rejected { .. } => ApiError::bad_request("bad request"),
        Error::Timeout => ApiError::TIMEOUT,
        Error::Unavailable(_)
        | Error::ResponseTooBig
        | Error::Decode(_)
        | Error::Config(_)
        | Error::InternalUnavailable => ApiError::UPSTREAM,
    }
}

/// A stable name for the request log.
fn route_name(route: &Route) -> &'static str {
    use Route as R;
    match route {
        R::TipHeight => "tip_height",
        R::TipHash => "tip_hash",
        R::Blocks { .. } => "blocks",
        R::BlockHash(_) => "block_height",
        R::Block(_) => "block",
        R::BlockHeader(_) => "block_header",
        R::BlockStatus(_) => "block_status",
        R::BlockTxids(_) => "block_txids",
        R::BlockTxs { .. } => "block_txs",
        R::BlockRaw(_) => "block_raw",
        R::BlockTxid { .. } => "block_txid",
        R::Tx(_) => "tx",
        R::TxStatus { .. } => "tx_status",
        R::TxHex(_) => "tx_hex",
        R::TxRaw(_) => "tx_raw",
        R::TxMerkleProof(_) => "tx_merkle_proof",
        R::TxMerkleblockProof(_) => "tx_merkleblock_proof",
        R::TxOutspend { .. } => "tx_outspend",
        R::TxOutspends { .. } => "tx_outspends",
        R::TxsOutspends(_) => "txs_outspends",
        R::ScriptStats { .. } => "script_stats",
        R::ScriptUtxo { .. } => "script_utxo",
        R::ScriptTxs { .. } => "script_txs",
        R::ScriptTxsChain { .. } => "script_txs_chain",
        R::ScriptTxsMempool(_) => "script_txs_mempool",
        R::ScriptTxsSummary { .. } => "script_txs_summary",
        R::Mempool => "mempool",
        R::MempoolTxids => "mempool_txids",
        R::MempoolTxidsPage { .. } => "mempool_txids_page",
        R::MempoolRecent => "mempool_recent",
        R::FeeEstimates => "fee_estimates",
        R::Broadcast(_) => "broadcast",
        R::TxAt { .. } => "tx_at",
        R::TxOut { .. } => "tx_out",
        R::OutputAt { .. } => "output_at",
        R::OutputsAt { .. } => "outputs_at",
        // Never parsed from a client request.
        _ => "internal",
    }
}

/// rbitcoin's broadcast rejects, mapped onto fixed text: its own messages can carry internal
/// detail (`durable: …`), so only reasons from this list reach the client.
fn reject_reason(message: &str) -> &'static str {
    const KNOWN: &[&str] = &[
        "mempool full",
        "negative fee",
        "mempool double-spend",
        "txn-same-nonwitness-data-in-mempool",
        "bad-txns-spends-conflicting-tx",
        "adds unconfirmed input",
        "min relay fee",
        "mempool min fee",
        "too-large-cluster",
        "rbf insufficient fee",
        "coinbase",
        "inputs-duplicate",
        "coinbase immature",
        "not final",
        "non-BIP68-final",
        "bad-txns-too-many-sigops",
        "dust",
        "libre annex",
        "scriptpubkey",
        "tx weight",
        "bad-txns-vin-empty",
        "bad-txns-vout-empty",
        "bad-txns-oversize",
        "bad-txns-vout-negative",
        "bad-txns-vout-toolarge",
        "bad-txns-txouttotal-toolarge",
        "bad-txns-inputs-duplicate",
        "bad-cb-length",
        "bad-txns-prevout-null",
        "bad-txns-in-belowout",
    ];
    let reason = message.strip_prefix("policy: ").unwrap_or(message);
    if let Some(known) = KNOWN.iter().find(|k| **k == reason) {
        return known;
    }
    if message.starts_with("duplicate ") {
        "transaction already in the mempool or chain"
    } else if message.starts_with("missing prevout ") || message.starts_with("orphaned ") {
        "missing or spent inputs"
    } else if message.starts_with("script: ") {
        "script verification failed"
    } else {
        "transaction rejected"
    }
}

/// Parses the path after `/api/` and the query into a [`Call`]. Unknown shapes are 404; a known
/// shape with a bad parameter is 400. Nothing is percent-decoded: no valid parameter contains `%`.
fn parse(path: &str, query: Option<&str>, network: Network) -> Result<Call, ApiError> {
    let segs: Vec<&str> = path.split('/').collect();
    // A trailing or doubled slash is not a route.
    if segs.contains(&"") {
        return Err(ApiError::NOT_FOUND);
    }
    let mut q = Query::parse(query)?;
    let call = parse_segments(&segs, &mut q, network)?;
    if !q.0.is_empty() {
        return Err(ApiError::bad_request("unexpected query parameter"));
    }
    Ok(call)
}

fn parse_segments(segs: &[&str], q: &mut Query<'_>, network: Network) -> Result<Call, ApiError> {
    use Route as R;
    let route = match *segs {
        ["tx"] => return Ok(Call::Broadcast),
        ["block-heights", "txs", "outs"] => return Ok(Call::Outputs { asof: q.asof()? }),

        ["blocks", "tip", "height"] => R::TipHeight,
        ["blocks", "tip", "hash"] => R::TipHash,
        ["blocks"] => R::Blocks { start_height: None },
        ["blocks", h] => R::Blocks {
            start_height: Some(int(h, "invalid height")?),
        },
        ["block-height", h] => R::BlockHash(int(h, "invalid height")?),
        ["block-height", h, "tx", i] => R::TxAt {
            height: int(h, "invalid height")?,
            index: int(i, "invalid index")?,
            asof: q.asof()?,
        },
        ["block-height", h, "tx", i, "out", v] => R::OutputAt {
            position: OutPosition {
                height: int(h, "invalid height")?,
                index: int(i, "invalid index")?,
                vout: int(v, "invalid vout")?,
            },
            asof: q.asof()?,
        },

        ["block", h] => R::Block(block_hash(h)?),
        ["block", h, "header"] => R::BlockHeader(block_hash(h)?),
        ["block", h, "status"] => R::BlockStatus(block_hash(h)?),
        ["block", h, "raw"] => R::BlockRaw(block_hash(h)?),
        ["block", h, "txids"] => R::BlockTxids(block_hash(h)?),
        ["block", h, "txid", i] => R::BlockTxid {
            hash: block_hash(h)?,
            index: int(i, "invalid index")?,
        },
        // The node answers `/txs` and `/txs/0` with the same page.
        ["block", h, "txs"] => R::BlockTxs {
            hash: block_hash(h)?,
            start: BlockTxsStart::FIRST,
        },
        ["block", h, "txs", s] => R::BlockTxs {
            hash: block_hash(h)?,
            start: BlockTxsStart::new(int(s, "invalid start index")?).ok_or(
                ApiError::bad_request("start index must be a multiple of 25"),
            )?,
        },

        ["tx", t] => R::Tx(txid(t)?),
        ["tx", t, "hex"] => R::TxHex(txid(t)?),
        ["tx", t, "raw"] => R::TxRaw(txid(t)?),
        ["tx", t, "status"] => R::TxStatus {
            txid: txid(t)?,
            asof: q.asof()?,
        },
        ["tx", t, "merkle-proof"] => R::TxMerkleProof(txid(t)?),
        ["tx", t, "merkleblock-proof"] => R::TxMerkleblockProof(txid(t)?),
        ["tx", t, "outspend", v] => R::TxOutspend {
            txid: txid(t)?,
            vout: int(v, "invalid vout")?,
            asof: q.asof()?,
        },
        ["tx", t, "outspends"] => R::TxOutspends {
            txid: txid(t)?,
            asof: q.asof()?,
        },
        ["tx", t, "out", v] => R::TxOut {
            txid: txid(t)?,
            vout: int(v, "invalid vout")?,
            asof: q.asof()?,
        },
        ["txs", "outspends"] => R::TxsOutspends(q.txids()?),

        [kind @ ("address" | "scripthash"), s, ref rest @ ..] => {
            let script = if kind == "address" {
                ScriptRef::Address(address(s, network)?)
            } else {
                ScriptRef::ScriptHash(
                    ScriptHash::from_str(s)
                        .map_err(|_| ApiError::bad_request("invalid scripthash"))?,
                )
            };
            match *rest {
                [] => R::ScriptStats {
                    script,
                    asof: q.asof()?,
                },
                ["utxo"] => R::ScriptUtxo {
                    script,
                    asof: q.asof()?,
                },
                ["txs"] => R::ScriptTxs {
                    script,
                    after: q.after_txid()?,
                    asof: q.asof()?,
                },
                ["txs", "chain"] => R::ScriptTxsChain {
                    script,
                    last_seen: None,
                    asof: q.asof()?,
                },
                ["txs", "chain", t] => R::ScriptTxsChain {
                    script,
                    last_seen: Some(txid(t)?),
                    asof: q.asof()?,
                },
                ["txs", "mempool"] => R::ScriptTxsMempool(script),
                // `?after_txid=` and the path cursor mean the same on the node.
                ["txs", "summary"] => R::ScriptTxsSummary {
                    script,
                    last_seen: q.after_txid()?,
                    asof: q.asof()?,
                },
                ["txs", "summary", t] => R::ScriptTxsSummary {
                    script,
                    last_seen: Some(txid(t)?),
                    asof: q.asof()?,
                },
                _ => return Err(ApiError::NOT_FOUND),
            }
        }

        ["mempool"] => R::Mempool,
        ["mempool", "txids"] => R::MempoolTxids,
        ["mempool", "txids", "page"] => R::MempoolTxidsPage {
            after: None,
            max_txs: q.max_txs()?,
        },
        ["mempool", "txids", "page", t] => R::MempoolTxidsPage {
            after: Some(txid(t)?),
            max_txs: q.max_txs()?,
        },
        ["mempool", "recent"] => R::MempoolRecent,
        ["fee-estimates"] => R::FeeEstimates,
        _ => return Err(ApiError::NOT_FOUND),
    };
    Ok(Call::Read(route))
}

/// Decimal digits only: `u32::from_str` would also take a leading `+`.
fn int(s: &str, msg: &'static str) -> Result<u32, ApiError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::bad_request(msg));
    }
    s.parse().map_err(|_| ApiError::bad_request(msg))
}

fn txid(s: &str) -> Result<Txid, ApiError> {
    hash(s).ok_or(ApiError::bad_request("invalid txid"))
}

fn block_hash(s: &str) -> Result<BlockHash, ApiError> {
    hash(s).ok_or(ApiError::bad_request("invalid block hash"))
}

/// Exactly 64 hex digits.
fn hash<T: FromStr>(s: &str) -> Option<T> {
    (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| s.parse().ok())
        .flatten()
}

fn address(s: &str, network: Network) -> Result<Address, ApiError> {
    let chain = match network {
        Network::Mainnet => bitcoin::Network::Bitcoin,
        Network::Signet => bitcoin::Network::Signet,
        Network::Testnet4 => bitcoin::Network::Testnet4,
        Network::Regtest => bitcoin::Network::Regtest,
    };
    // Longer than any address; refuse before the decoder looks at it.
    if s.len() > 128 {
        return Err(ApiError::bad_request("invalid address"));
    }
    s.parse::<Address<NetworkUnchecked>>()
        .map_err(|_| ApiError::bad_request("invalid address"))?
        .require_network(chain)
        .map_err(|_| ApiError::bad_request("address is for another network"))
}

/// The query string as `name=value` pairs, each name at most once.
struct Query<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Query<'a> {
    fn parse(query: Option<&'a str>) -> Result<Self, ApiError> {
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for part in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            if pairs.iter().any(|(n, _)| *n == name) {
                return Err(ApiError::bad_request("repeated query parameter"));
            }
            pairs.push((name, value));
        }
        Ok(Self(pairs))
    }

    fn take(&mut self, name: &str) -> Option<&'a str> {
        let i = self.0.iter().position(|(n, _)| *n == name)?;
        Some(self.0.swap_remove(i).1)
    }

    fn asof(&mut self) -> Result<Option<BlockHash>, ApiError> {
        self.take("asof")
            .map(|s| hash(s).ok_or(ApiError::bad_request("invalid asof")))
            .transpose()
    }

    /// An empty value counts as absent, as on the node.
    fn after_txid(&mut self) -> Result<Option<Txid>, ApiError> {
        match self.take("after_txid") {
            None | Some("") => Ok(None),
            Some(s) => hash(s)
                .map(Some)
                .ok_or(ApiError::bad_request("invalid after_txid")),
        }
    }

    fn max_txs(&mut self) -> Result<Option<u32>, ApiError> {
        self.take("max_txs")
            .map(|s| int(s, "invalid max_txs"))
            .transpose()
    }

    /// `txids=` as comma-separated txids. Browsers' URL encoders write the comma as `%2C`.
    fn txids(&mut self) -> Result<Vec<Txid>, ApiError> {
        let list = self
            .take("txids")
            .filter(|s| !s.is_empty())
            .ok_or(ApiError::bad_request("txids is required"))?;
        let list = list.replace("%2C", ",").replace("%2c", ",");
        let ids = list.split(',').map(txid).collect::<Result<Vec<_>, _>>()?;
        if ids.len() > MAX_OUTSPENDS_TXIDS {
            return Err(ApiError::bad_request("at most 50 txids"));
        }
        Ok(ids)
    }
}

type Answer = Result<RawResponse, Error>;

#[derive(Debug)]
struct Done {
    at: Instant,
    answer: Answer,
}

/// Upstream calls in flight, by upstream path, plus the last answer for each hot key.
///
/// The map holds an entry only while some request holds it, or for a hot key, whose answer stays
/// for its TTL. Hot keys are a fixed set ([`is_hot`]), so the map is bounded by the number of
/// requests in flight (`MAX_CONCURRENT_REQUESTS`) plus that set.
#[derive(Debug, Default)]
pub struct Flights {
    map: Mutex<HashMap<String, Arc<OnceCell<Done>>>>,
}

impl Flights {
    /// Answers with `fetch`, unless an identical call is in flight or a hot answer for `key` is
    /// younger than `ttl`; then with that one. `fetch` is not polled when it is not needed.
    pub async fn run(
        &self,
        key: String,
        ttl: Duration,
        fetch: impl Future<Output = Answer>,
    ) -> (Answer, Source) {
        let cell = self.join(&key, ttl);
        let ready = cell.initialized();
        let mut took = None;
        let took_ref = &mut took;
        let _leave = Leave {
            flights: self,
            key: &key,
            cell: &cell,
            ttl,
        };
        // If the request driving the call is dropped, the next one waiting takes over.
        let done = cell
            .get_or_init(move || async move {
                let started = Instant::now();
                let answer = fetch.await;
                *took_ref = Some(started.elapsed());
                Done {
                    at: Instant::now(),
                    answer,
                }
            })
            .await;
        let source = match took {
            Some(d) => Source::Upstream(d),
            None if ready => Source::Hot,
            None => Source::Coalesced,
        };
        (done.answer.clone(), source)
    }

    fn join(&self, key: &str, ttl: Duration) -> Arc<OnceCell<Done>> {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cell) = map.get(key)
            && cell.get().is_none_or(|d| d.at.elapsed() < ttl)
        {
            return Arc::clone(cell);
        }
        let cell = Arc::new(OnceCell::new());
        map.insert(key.to_owned(), Arc::clone(&cell));
        cell
    }

    /// Entries in the map, for tests.
    pub fn len(&self) -> usize {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Drops a request's hold on its entry; the last holder removes it unless it is a fresh hot answer.
struct Leave<'a> {
    flights: &'a Flights,
    key: &'a str,
    cell: &'a Arc<OnceCell<Done>>,
    ttl: Duration,
}

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        let mut map = self
            .flights
            .map
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(cell) = map.get(self.key) else {
            return;
        };
        let fresh = cell.get().is_some_and(|d| d.at.elapsed() < self.ttl);
        // Two holders: the map and this request. Others join only under this lock.
        if Arc::ptr_eq(cell, self.cell) && Arc::strong_count(cell) <= 2 && !fresh {
            map.remove(self.key);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const H: &str = "00000000000000000002a7c4c1e48d76c5a37902165a270156b7a8d72728a054";
    const MAIN_ADDR: &str = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
    const TEST_ADDR: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
    /// One input spending a null outpoint, one empty output: decodable, though not valid.
    const TX: &str = concat!(
        "01000000",
        "01",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "ffffffff",
        "0100",
        "ffffffff",
        "01",
        "0000000000000000",
        "00",
        "00000000"
    );

    fn path(c: &Call) -> String {
        let Call::Read(r) = c else { panic!("{c:?}") };
        let mut s = String::new();
        r.write_path(&mut s);
        s
    }

    fn ok(p: &str) -> String {
        let (p, q) = p.split_once('?').map_or((p, None), |(p, q)| (p, Some(q)));
        path(&parse(p, q, Network::Mainnet).unwrap())
    }

    fn err(p: &str) -> ApiError {
        let (p, q) = p.split_once('?').map_or((p, None), |(p, q)| (p, Some(q)));
        parse(p, q, Network::Mainnet).unwrap_err()
    }

    #[test]
    fn every_route_rebuilds_its_own_path() {
        let same = [
            "blocks/tip/height".to_owned(),
            "blocks/tip/hash".into(),
            "blocks".into(),
            "blocks/800000".into(),
            "block-height/800000".into(),
            "block-height/800000/tx/3".into(),
            "block-height/800000/tx/3/out/1".into(),
            format!("block-height/1/tx/2?asof={H}"),
            format!("block/{H}"),
            format!("block/{H}/header"),
            format!("block/{H}/status"),
            format!("block/{H}/raw"),
            format!("block/{H}/txids"),
            format!("block/{H}/txid/7"),
            format!("block/{H}/txs/25"),
            format!("tx/{H}"),
            format!("tx/{H}/hex"),
            format!("tx/{H}/raw"),
            format!("tx/{H}/status"),
            format!("tx/{H}/status?asof={H}"),
            format!("tx/{H}/merkle-proof"),
            format!("tx/{H}/merkleblock-proof"),
            format!("tx/{H}/outspend/4"),
            format!("tx/{H}/outspends?asof={H}"),
            format!("tx/{H}/out/0"),
            format!("txs/outspends?txids={H},{H}"),
            format!("address/{MAIN_ADDR}"),
            format!("address/{MAIN_ADDR}/utxo?asof={H}"),
            format!("address/{MAIN_ADDR}/txs"),
            format!("address/{MAIN_ADDR}/txs?after_txid={H}&asof={H}"),
            format!("address/{MAIN_ADDR}/txs/chain"),
            format!("address/{MAIN_ADDR}/txs/chain/{H}"),
            format!("address/{MAIN_ADDR}/txs/mempool"),
            format!("address/{MAIN_ADDR}/txs/summary"),
            format!("address/{MAIN_ADDR}/txs/summary/{H}"),
            format!("scripthash/{H}"),
            format!("scripthash/{H}/txs/chain/{H}?asof={H}"),
            "mempool".into(),
            "mempool/txids".into(),
            "mempool/txids/page".into(),
            format!("mempool/txids/page/{H}?max_txs=10"),
            "mempool/recent".into(),
            "fee-estimates".into(),
        ];
        for p in same {
            assert_eq!(ok(&p), format!("/{p}"), "{p}");
        }
    }

    #[test]
    fn equivalent_forms_are_normalized() {
        assert_eq!(ok(&format!("block/{H}/txs")), format!("/block/{H}/txs/0"));
        assert_eq!(
            ok(&format!("address/{MAIN_ADDR}/txs/summary?after_txid={H}")),
            format!("/address/{MAIN_ADDR}/txs/summary/{H}")
        );
        // Bech32 is case-insensitive; the node gets the canonical form.
        assert_eq!(
            ok(&format!("address/{}", MAIN_ADDR.to_uppercase())),
            format!("/address/{MAIN_ADDR}")
        );
        assert_eq!(
            ok(&format!("txs/outspends?txids={H}%2C{H}")),
            format!("/txs/outspends?txids={H},{H}")
        );
        assert_eq!(
            ok(&format!("address/{MAIN_ADDR}/txs?after_txid=")),
            format!("/address/{MAIN_ADDR}/txs")
        );
        // Uppercase hex is a valid hash; the node gets lowercase.
        assert_eq!(ok(&format!("tx/{}", H.to_uppercase())), format!("/tx/{H}"));
    }

    #[test]
    fn posts_and_unknown_shapes() {
        assert_eq!(
            parse("tx", None, Network::Mainnet).unwrap(),
            Call::Broadcast
        );
        assert_eq!(
            parse(
                "block-heights/txs/outs",
                Some(&format!("asof={H}")),
                Network::Mainnet
            )
            .unwrap(),
            Call::Outputs {
                asof: Some(H.parse().unwrap())
            }
        );
        for p in [
            "",
            "blocks/",
            "block-template",
            "internal/txs",
            "internal/mempool/txs/all",
            "txs/test",
            "txs/package",
            "addresses/txs",
            "v1/fees/recommended",
            "tx/../internal/txs",
            &format!("block/{H}/txs/25/x"),
            &format!("address/{MAIN_ADDR}/foo"),
        ] {
            assert_eq!(err(p), ApiError::NOT_FOUND, "{p}");
        }
    }

    #[test]
    fn hostile_parameters_are_400() {
        let long = "1".repeat(10_000);
        for p in [
            "blocks/+5".to_owned(),
            "blocks/tip".into(),
            "blocks/-1".into(),
            "blocks/4294967296".into(),
            "blocks/1.5".into(),
            "block-height/%31".into(),
            format!("block/{H}0"),
            format!("block/0x{}", &H[2..]),
            format!("block/{}", &H[1..]),
            format!("tx/{}%2F..", &H[..61]),
            format!("tx/{}", "g".repeat(64)),
            format!("tx/{long}"),
            "tx/..".into(),
            "tx/%2e%2e".into(),
            format!("block/{H}/txs/26"),
            format!("tx/{H}/outspend/x"),
            "address/notanaddress".into(),
            format!("address/{long}"),
            format!("address/{TEST_ADDR}"),
            format!("scripthash/{MAIN_ADDR}"),
            format!("tx/{H}/status?asof=zz"),
            format!("tx/{H}/status?asof="),
            format!("tx/{H}?asof={H}"),
            format!("tx/{H}/status?asof={H}&asof={H}"),
            format!("tx/{H}?foo=bar"),
            "mempool?cachebust".into(),
            format!("address/{MAIN_ADDR}/txs/mempool?asof={H}"),
            "txs/outspends".into(),
            "txs/outspends?txids=".into(),
            format!("txs/outspends?txids={H},zz"),
            format!("txs/outspends?txids={}", vec![H; 51].join(",")),
            "mempool/txids/page?max_txs=-1".into(),
        ] {
            let e = err(&p);
            assert_eq!(e.status(), StatusCode::BAD_REQUEST, "{p}: {e:?}");
        }
    }

    #[test]
    fn addresses_are_checked_for_the_network() {
        let p = format!("address/{TEST_ADDR}");
        for net in [Network::Signet, Network::Testnet4] {
            assert!(parse(&p, None, net).is_ok());
        }
        assert!(parse(&format!("address/{MAIN_ADDR}"), None, Network::Signet).is_err());
        assert!(parse(&p, None, Network::Regtest).is_err());
    }

    #[test]
    fn cache_classes() {
        let h: BlockHash = H.parse().unwrap();
        let t: Txid = H.parse().unwrap();
        assert_eq!(cache_control(&Route::Block(h), b""), IMMUTABLE);
        assert_eq!(cache_control(&Route::BlockRaw(h), b""), IMMUTABLE);
        assert_eq!(cache_control(&Route::BlockStatus(h), b""), LIVE);
        assert_eq!(
            cache_control(
                &Route::BlockTxs {
                    hash: h,
                    start: BlockTxsStart::FIRST
                },
                b""
            ),
            LIVE
        );
        assert_eq!(cache_control(&Route::Tx(t), b""), LIVE);
        assert_eq!(cache_control(&Route::BlockHash(1), b""), LIVE);
        assert_eq!(cache_control(&Route::TxMerkleProof(t), b""), LIVE);
        // Legacy serialization: version, then a non-zero input count.
        assert_eq!(cache_control(&Route::TxHex(t), b"0200000001ab"), IMMUTABLE);
        assert_eq!(
            cache_control(&Route::TxRaw(t), &[2, 0, 0, 0, 1, 9]),
            IMMUTABLE
        );
        // Witness serialization: marker 0x00, flag 0x01.
        assert_eq!(cache_control(&Route::TxHex(t), b"020000000001ab"), LIVE);
        assert_eq!(cache_control(&Route::TxRaw(t), &[2, 0, 0, 0, 0, 1]), LIVE);
        assert_eq!(cache_control(&Route::TxRaw(t), &[2, 0]), LIVE);
    }

    #[test]
    fn upstream_errors_map_to_fixed_messages() {
        let r = Route::TipHeight;
        let b = Route::Broadcast(parse_tx(TX.as_bytes()).unwrap());
        let s = |status: u16, body: &str, route: &Route| {
            let e = Error::from_status(StatusCode::from_u16(status).unwrap(), body.as_bytes());
            let a = upstream_error(&e, route);
            (a.status().as_u16(), format!("{a:?}"))
        };
        assert_eq!(s(404, "Not Found", &r).0, 404);
        let (code, msg) = s(503, "scripthash index disabled (--sh-index)", &r);
        assert_eq!(code, 503);
        assert!(!msg.contains("--sh-index"), "{msg}");
        let (code, msg) = s(503, "too many rows; raise --max-sh-creates", &r);
        assert_eq!(code, 503);
        assert!(!msg.contains("--max"), "{msg}");
        assert_eq!(s(503, "chain view moved", &r).0, 503);
        assert_eq!(s(422, "after_txid not found", &r).0, 422);
        let (code, msg) = s(400, "policy: min relay fee", &b);
        assert_eq!(code, 400);
        assert!(msg.contains("\"min relay fee\""), "{msg}");
        let (_, msg) = s(400, "durable: /var/lib/node/x: disk full", &b);
        assert!(!msg.contains("/var"), "{msg}");
        let (_, msg) = s(400, &format!("duplicate {H}"), &b);
        assert!(msg.contains("already"), "{msg}");
        assert_eq!(s(500, "panic at src/x.rs", &r).0, 502);
    }

    fn answer(n: u8) -> Answer {
        Ok(RawResponse {
            status: StatusCode::OK,
            content_type: None,
            tip: None,
            body: Bytes::from(vec![n]),
        })
    }

    #[tokio::test]
    async fn concurrent_identical_calls_share_one_fetch() {
        let flights = Arc::new(Flights::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..50 {
            let (f, c) = (Arc::clone(&flights), Arc::clone(&calls));
            tasks.push(tokio::spawn(async move {
                f.run("/k".into(), Duration::ZERO, async {
                    c.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    answer(1)
                })
                .await
            }));
        }
        let mut sources = Vec::new();
        for t in tasks {
            let (a, source) = t.await.unwrap();
            assert_eq!(a.unwrap().body[0], 1);
            sources.push(source);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let fetched = sources
            .iter()
            .filter(|s| matches!(s, Source::Upstream(d) if *d >= Duration::from_millis(50)))
            .count();
        let joined = sources.iter().filter(|s| **s == Source::Coalesced).count();
        assert_eq!((fetched, joined), (1, 49));
        assert!(flights.is_empty(), "a finished call leaves nothing behind");
        // A later request is a new call: answers are shared only while in flight.
        flights
            .run("/k".into(), Duration::ZERO, async {
                calls.fetch_add(1, Ordering::SeqCst);
                answer(2)
            })
            .await
            .0
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn hot_answers_are_reused_for_their_ttl_only() {
        let flights = Flights::default();
        let ttl = Duration::from_millis(100);
        let a = flights.run("/hot".into(), ttl, async { answer(1) }).await;
        let b = flights.run("/hot".into(), ttl, async { answer(2) }).await;
        assert_eq!((a.0.unwrap().body[0], b.0.unwrap().body[0]), (1, 1));
        assert!(matches!(a.1, Source::Upstream(_)));
        assert_eq!(b.1, Source::Hot);
        assert_eq!(flights.len(), 1);
        tokio::time::sleep(ttl).await;
        let c = flights.run("/hot".into(), ttl, async { answer(3) }).await;
        assert_eq!(c.0.unwrap().body[0], 3);
        assert_eq!(flights.len(), 1, "one entry per hot key");
    }

    #[tokio::test]
    async fn cancelled_and_distinct_calls_do_not_accumulate() {
        let flights = Arc::new(Flights::default());
        for i in 0..1000 {
            let f = Arc::clone(&flights);
            let t = tokio::spawn(async move {
                f.run(format!("/k{i}"), Duration::ZERO, std::future::pending())
                    .await
            });
            tokio::task::yield_now().await;
            t.abort();
            let _ = t.await;
        }
        assert!(flights.is_empty(), "{} left", flights.len());
        // A waiter whose driver is cancelled takes over the call.
        let f = Arc::clone(&flights);
        let driver = tokio::spawn(async move {
            f.run("/x".into(), Duration::ZERO, std::future::pending())
                .await
        });
        tokio::task::yield_now().await;
        let f = Arc::clone(&flights);
        let waiter = tokio::spawn(async move {
            f.run("/x".into(), Duration::ZERO, async { answer(7) })
                .await
        });
        tokio::task::yield_now().await;
        driver.abort();
        let (a, source) = waiter.await.unwrap();
        assert_eq!(a.unwrap().body[0], 7);
        assert!(matches!(source, Source::Upstream(_)));
        assert!(flights.is_empty());
    }

    #[test]
    fn transactions_parse_strictly() {
        assert!(parse_tx(format!(" {TX}\n").as_bytes()).is_ok());
        assert!(
            parse_tx(format!("{TX}00").as_bytes()).is_err(),
            "trailing bytes"
        );
        assert!(parse_tx(b"zz").is_err());
        assert!(parse_tx(b"").is_err());
        assert!(parse_tx(&[0xff, 0xfe]).is_err());
        assert_eq!(
            parse_tx("00".repeat(MAX_TX_BYTES + 1).as_bytes()).unwrap_err(),
            ApiError::PAYLOAD_TOO_LARGE
        );
    }
}
