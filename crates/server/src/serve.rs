//! The accept loop. axum's own `serve` has no header-read timeout, write timeout or connection
//! caps, so one client could hold every socket open with partial requests or unread responses.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use axum::response::Response;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::time::Sleep;
use tower::ServiceExt;

use crate::limits::{Checked, Hold, Limiter};
use crate::refusals::Refusals;

/// What a request needs before it is routed: the per-client limits, checked here so a refusal
/// costs no routing, middleware or log line ([`refused_early`](crate::refused_early)).
pub(crate) struct Early {
    pub limiter: Limiter,
    pub refusals: Arc<Refusals>,
}

impl Early {
    /// `Ok` to route the request, holding the client's request slot if it was admitted here;
    /// `Err` with the answer to a refusal.
    #[expect(
        clippy::result_large_err,
        reason = "the error is the response, handed straight to hyper: boxing it would add an allocation to every refusal"
    )]
    fn step(
        &self,
        req: &mut hyper::Request<Incoming>,
        peer: SocketAddr,
    ) -> Result<Option<Hold>, Response> {
        let path = req.uri().path();
        match self
            .limiter
            .check(req.method(), req.headers(), path, peer.ip())
        {
            Ok((hold, addr, who)) => {
                req.extensions_mut().insert(Checked::Admitted { addr, who });
                Ok(Some(hold))
            }
            // The first of its reason in a while: routed, so it is logged with its route.
            Err((refused, who)) if self.refusals.record(refused.reason, path, who) => {
                req.extensions_mut().insert(Checked::Refused(refused, who));
                Ok(None)
            }
            Err((refused, _)) => Err(crate::refused_early(path, refused)),
        }
    }
}

/// Largest request head (request line and headers) a client may send.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// Serves `router` on `listener` until `shutdown` resolves, then lets open requests finish for up
/// to the request timeout.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    early: Arc<Early>,
    shutdown: impl Future<Output = ()>,
) {
    let limiter = &early.limiter;
    let limits = limiter.limits.clone();
    let open = Arc::new(Semaphore::new(limits.max_connections));
    let (stop, stopped) = watch::channel(());
    let mut shutdown = pin!(shutdown);
    let mut refused = 0u64;
    loop {
        let accepted = tokio::select! {
            () = &mut shutdown => break,
            accepted = accept(&listener, &open) => accepted,
        };
        let Some((stream, peer, permit)) = accepted else {
            continue;
        };
        // Over its connection cap: dropping the stream closes it. The client sees only a reset,
        // so the log says it happened.
        let Ok(hold) = limiter.connection(peer.ip()) else {
            refused += 1;
            if refused.is_power_of_two() {
                logging::emit(&logging::events::ConnectionsRefused { total: refused });
            }
            continue;
        };
        let router = router.clone();
        let early = Arc::clone(&early);
        let mut stopped = stopped.clone();
        let timeout = limits.request_timeout;
        tokio::spawn(async move {
            let _slots = (permit, hold);
            let service = service_fn(move |mut req: hyper::Request<Incoming>| {
                req.extensions_mut().insert(ConnectInfo(peer));
                // Cloned only to route: a shared count every core would contend on.
                let routed = early
                    .step(&mut req, peer)
                    .map(|hold| (hold, router.clone()));
                async move {
                    match routed {
                        Ok((hold, router)) => {
                            let res = router.oneshot(req).await;
                            drop(hold);
                            res
                        }
                        Err(res) => Ok(res),
                    }
                }
            });
            let mut builder = http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                // Also closes keep-alive connections that sit idle this long.
                .header_read_timeout(timeout)
                .max_buf_size(MAX_HEAD_BYTES);
            let mut conn = pin!(
                builder
                    .serve_connection(TokioIo::new(WriteDeadline::new(stream, timeout)), service)
                    .with_upgrades()
            );
            tokio::select! {
                _ = conn.as_mut() => {}
                _ = stopped.changed() => {
                    conn.as_mut().graceful_shutdown();
                    let _ = conn.await;
                }
            }
        });
    }
    drop(stop);
    let all = u32::try_from(limits.max_connections).unwrap_or(u32::MAX);
    let _ = tokio::time::timeout(limits.request_timeout, open.acquire_many(all)).await;
}

/// Waits for a free connection slot, then for a connection.
async fn accept(
    listener: &TcpListener,
    open: &Arc<Semaphore>,
) -> Option<(TcpStream, SocketAddr, OwnedSemaphorePermit)> {
    let permit = Arc::clone(open).acquire_owned().await.ok()?;
    match listener.accept().await {
        Ok((stream, peer)) => Some((stream, peer, permit)),
        Err(_) => {
            // Usually out of file descriptors: back off instead of spinning on the error.
            tokio::time::sleep(Duration::from_millis(50)).await;
            None
        }
    }
}

/// A stream whose writes fail once they make no progress for `limit` (nginx calls this
/// `send_timeout`). Without it a client that stops reading holds its connection slot forever.
struct WriteDeadline {
    io: TcpStream,
    limit: Duration,
    stalled: Option<Pin<Box<Sleep>>>,
}

impl WriteDeadline {
    fn new(io: TcpStream, limit: Duration) -> Self {
        Self {
            io,
            limit,
            stalled: None,
        }
    }

    fn watch<T>(&mut self, cx: &mut Context<'_>, poll: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if poll.is_ready() {
            self.stalled = None;
            return poll;
        }
        let limit = self.limit;
        let stalled = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
        match stalled.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncRead for WriteDeadline {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_read(cx, buf)
    }
}

impl AsyncWrite for WriteDeadline {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.io).poll_write(cx, buf);
        this.watch(cx, poll)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.io).poll_write_vectored(cx, bufs);
        this.watch(cx, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.io).poll_flush(cx);
        this.watch(cx, poll)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.io).poll_shutdown(cx);
        this.watch(cx, poll)
    }
}
