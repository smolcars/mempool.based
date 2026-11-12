//! Per-client accounting and the request limits middleware (SECURITY.md rules 6 and 7).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::config::Limits;
use crate::error::ApiError;

/// What a client holds open. Each kind has its own per-client cap, passed to [`Client::hold`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// An HTTP request being handled.
    Request,
    /// An open TCP connection.
    Connection,
    /// An open WebSocket.
    Socket,
}

/// Rate and concurrency state for every recently active client, in a table of bounded size.
#[derive(Debug)]
pub struct Clients {
    table: Mutex<Table>,
    rate: f64,
    burst: f64,
    max_entries: usize,
}

#[derive(Debug)]
struct Table {
    entries: HashMap<IpAddr, Entry>,
    last_sweep: Instant,
}

#[derive(Debug)]
struct Entry {
    tokens: f64,
    refilled: Instant,
    requests: u32,
    connections: u32,
    sockets: u32,
}

impl Entry {
    fn count(&mut self, kind: Held) -> &mut u32 {
        match kind {
            Held::Request => &mut self.requests,
            Held::Connection => &mut self.connections,
            Held::Socket => &mut self.sockets,
        }
    }

    fn refill(&mut self, now: Instant, rate: f64, burst: f64) {
        let earned = now.saturating_duration_since(self.refilled).as_secs_f64() * rate;
        self.tokens = (self.tokens + earned).min(burst);
        self.refilled = now;
    }

    /// An entry with a full bucket and nothing held is the same as no entry, so it can be dropped.
    fn idle(&mut self, now: Instant, rate: f64, burst: f64) -> bool {
        self.refill(now, rate, burst);
        self.tokens >= burst && self.requests == 0 && self.connections == 0 && self.sockets == 0
    }
}

impl Clients {
    pub fn new(limits: &Limits) -> Arc<Self> {
        Self::with_rate(
            f64::from(limits.client_rate_per_sec),
            limits.client_burst,
            limits.max_tracked_clients,
        )
    }

    /// A separate table with its own rate (tokens per second) and burst, for a route that needs a
    /// stricter limit than the rest.
    pub fn with_rate(rate: f64, burst: u32, max_entries: usize) -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(Table {
                entries: HashMap::new(),
                last_sweep: Instant::now(),
            }),
            rate,
            burst: f64::from(burst),
            max_entries,
        })
    }

    /// Takes one rate token for `client` and holds nothing.
    pub fn take(self: &Arc<Self>, client: IpAddr) -> Result<(), ApiError> {
        self.acquire(client, Held::Request, u32::MAX, 1.0).map(drop)
    }

    /// Takes `cost` rate tokens (0 or 1) and one `kind` slot for `client`, all or nothing.
    fn acquire(
        self: &Arc<Self>,
        client: IpAddr,
        kind: Held,
        max: u32,
        cost: f64,
    ) -> Result<Hold, ApiError> {
        let now = Instant::now();
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        if !table.entries.contains_key(&client) && table.entries.len() >= self.max_entries {
            // Sweep at most once a second, so a flood of new addresses cannot make every request O(n).
            if now.saturating_duration_since(table.last_sweep) >= Duration::from_secs(1) {
                table.last_sweep = now;
                let (rate, burst) = (self.rate, self.burst);
                table.entries.retain(|_, e| !e.idle(now, rate, burst));
            }
            if table.entries.len() >= self.max_entries {
                return Err(ApiError::OVERLOADED);
            }
        }
        let entry = table.entries.entry(client).or_insert(Entry {
            tokens: self.burst,
            refilled: now,
            requests: 0,
            connections: 0,
            sockets: 0,
        });
        entry.refill(now, self.rate, self.burst);
        if entry.tokens < cost || *entry.count(kind) >= max {
            return Err(ApiError::TOO_MANY_REQUESTS);
        }
        entry.tokens -= cost;
        *entry.count(kind) += 1;
        Ok(Hold {
            clients: Arc::clone(self),
            client,
            kind,
        })
    }

    /// Number of clients currently tracked. Bounded by `MAX_TRACKED_CLIENTS`.
    pub fn tracked(&self) -> usize {
        self.table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .len()
    }
}

/// One slot a client holds; released on drop.
#[derive(Debug)]
pub struct Hold {
    clients: Arc<Clients>,
    client: IpAddr,
    kind: Held,
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut table = self
            .clients
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(e) = table.entries.get_mut(&self.client) {
            let n = e.count(self.kind);
            *n = n.saturating_sub(1);
        }
    }
}

/// The caller of the current request, as a request extension. The WebSocket hub uses
/// [`Client::hold`] with [`Held::Socket`] to cap sockets per client against the same table.
#[derive(Debug, Clone)]
pub struct Client {
    pub addr: IpAddr,
    clients: Arc<Clients>,
}

impl Client {
    /// Holds one `kind` slot for this client if it has fewer than `max`. No rate token is taken.
    pub fn hold(&self, kind: Held, max: u32) -> Result<Hold, ApiError> {
        self.clients.acquire(self.addr, kind, max, 0.0)
    }
}

/// The address limits are keyed on. IPv6 clients are grouped by /64, since one host is routinely
/// given a whole /64 and could otherwise rotate through addresses to dodge every per-client limit.
pub fn client_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (!0u128 << 64))),
        },
    }
}

/// The client address: from the trusted proxy header when one is configured and parses, else the
/// socket peer. The last value wins because a proxy appends the address it saw to any the client sent.
fn client_addr(
    headers: &HeaderMap,
    peer: IpAddr,
    trusted: Option<&axum::http::HeaderName>,
) -> IpAddr {
    let from_header = trusted.and_then(|name| {
        let value = headers.get_all(name).iter().next_back()?.to_str().ok()?;
        let last = value.rsplit(',').next()?.trim();
        last.parse::<IpAddr>()
            .ok()
            .or_else(|| last.parse::<SocketAddr>().ok().map(|s| s.ip()))
    });
    client_key(from_header.unwrap_or(peer))
}

/// Shared state for [`limit`].
#[derive(Debug, Clone)]
pub struct Limiter {
    pub clients: Arc<Clients>,
    pub limits: Limits,
    in_flight: Arc<Semaphore>,
}

impl Limiter {
    pub fn new(limits: &Limits) -> Self {
        Self {
            clients: Clients::new(limits),
            limits: limits.clone(),
            in_flight: Arc::new(Semaphore::new(limits.max_requests)),
        }
    }

    /// Holds a connection slot for `peer`, refusing one past its cap. Behind a trusted proxy every
    /// connection comes from the proxy, so there is no per-client cap (`Ok(None)`): the proxy owns it.
    pub fn connection(&self, peer: IpAddr) -> Result<Option<Hold>, ApiError> {
        if self.limits.trusted_proxy_header.is_some() {
            return Ok(None);
        }
        let max = self.limits.client_connections;
        let hold = self
            .clients
            .acquire(client_key(peer), Held::Connection, max, 0.0)?;
        Ok(Some(hold))
    }
}

/// Aborts the task when dropped, so work for a client that hung up or timed out stops with it.
struct AbortOnDrop(JoinHandle<Response>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Middleware applying, in order: the per-client rate and concurrency limits, the global
/// concurrency limit (shedding load instead of queueing), then the request timeout.
pub async fn limit(State(limiter): State<Limiter>, mut req: Request, next: Next) -> Response {
    let Some(ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() else {
        // Only reachable if the router is served without `serve`; refuse rather than share one bucket.
        return ApiError::INTERNAL.into_response();
    };
    let limits = &limiter.limits;
    let addr = client_addr(
        req.headers(),
        peer.ip(),
        limits.trusted_proxy_header.as_ref(),
    );
    let _hold = match limiter
        .clients
        .acquire(addr, Held::Request, limits.client_requests, 1.0)
    {
        Ok(hold) => hold,
        Err(e) => return e.into_response(),
    };
    let Ok(_permit) = limiter.in_flight.clone().try_acquire_owned() else {
        return ApiError::OVERLOADED.into_response();
    };
    req.extensions_mut().insert(Client {
        addr,
        clients: Arc::clone(&limiter.clients),
    });
    // A task of its own turns a handler panic into a 500 instead of a dropped connection.
    let mut task = AbortOnDrop(tokio::spawn(next.run(req)));
    match tokio::time::timeout(limits.request_timeout, &mut task.0).await {
        Ok(Ok(res)) => res,
        Ok(Err(_)) => ApiError::INTERNAL.into_response(),
        Err(_) => ApiError::TIMEOUT.into_response(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn clients(rate: u32, burst: u32, max: usize) -> Arc<Clients> {
        Clients::new(&Limits {
            client_rate_per_sec: rate,
            client_burst: burst,
            max_tracked_clients: max,
            ..Limits::default()
        })
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ipv6_is_keyed_by_64_and_mapped_v4_by_v4() {
        assert_eq!(client_key(ip("2001:db8:1:2:aaaa::1")), ip("2001:db8:1:2::"));
        assert_eq!(client_key(ip("::ffff:192.0.2.7")), ip("192.0.2.7"));
        assert_eq!(client_key(ip("192.0.2.7")), ip("192.0.2.7"));
    }

    #[test]
    fn rate_is_per_client() {
        let c = clients(1, 2, 10);
        let a = ip("192.0.2.1");
        assert!(c.acquire(a, Held::Request, 10, 1.0).is_ok());
        assert!(c.acquire(a, Held::Request, 10, 1.0).is_ok());
        assert_eq!(
            c.acquire(a, Held::Request, 10, 1.0).unwrap_err(),
            ApiError::TOO_MANY_REQUESTS
        );
        assert!(c.acquire(ip("192.0.2.2"), Held::Request, 10, 1.0).is_ok());
    }

    #[test]
    fn holds_are_capped_per_kind_and_released_on_drop() {
        let c = clients(100, 100, 10);
        let a = ip("192.0.2.1");
        let first = c.acquire(a, Held::Socket, 1, 0.0).unwrap();
        assert!(c.acquire(a, Held::Socket, 1, 0.0).is_err());
        assert!(c.acquire(a, Held::Request, 1, 0.0).is_ok());
        drop(first);
        assert!(c.acquire(a, Held::Socket, 1, 0.0).is_ok());
    }

    #[test]
    fn table_is_bounded_and_keeps_serving_known_clients() {
        let c = clients(1, 1, 2);
        let _a = c.acquire(ip("192.0.2.1"), Held::Request, 10, 0.0).unwrap();
        let _b = c.acquire(ip("192.0.2.2"), Held::Request, 10, 0.0).unwrap();
        assert_eq!(
            c.acquire(ip("192.0.2.3"), Held::Request, 10, 0.0)
                .unwrap_err(),
            ApiError::OVERLOADED
        );
        assert_eq!(c.tracked(), 2);
        assert!(c.acquire(ip("192.0.2.1"), Held::Request, 10, 0.0).is_ok());
    }

    #[test]
    fn idle_clients_are_swept_when_the_table_is_full() {
        let c = clients(1, 1, 2);
        drop(c.acquire(ip("192.0.2.1"), Held::Request, 10, 0.0).unwrap());
        drop(c.acquire(ip("192.0.2.2"), Held::Request, 10, 0.0).unwrap());
        c.table.lock().unwrap().last_sweep -= Duration::from_secs(2);
        assert!(c.acquire(ip("192.0.2.3"), Held::Request, 10, 0.0).is_ok());
        assert_eq!(c.tracked(), 1);
    }

    #[test]
    fn proxy_header_is_used_only_when_trusted() {
        let mut h = HeaderMap::new();
        h.append(
            "x-forwarded-for",
            "203.0.113.9, 198.51.100.4".parse().unwrap(),
        );
        let peer = ip("127.0.0.1");
        let name = axum::http::HeaderName::from_static("x-forwarded-for");
        assert_eq!(client_addr(&h, peer, None), peer);
        assert_eq!(client_addr(&h, peer, Some(&name)), ip("198.51.100.4"));
        h.insert("x-forwarded-for", "garbage".parse().unwrap());
        assert_eq!(client_addr(&h, peer, Some(&name)), peer);
    }
}
