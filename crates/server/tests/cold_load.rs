//! Replays the frontend's recorded cold page loads (`fixtures/cold-load.json`, recorded by
//! `dev/cold-load/record.mjs`) against the default per-client limits. If the frontend grows past
//! them, re-recording the fixture makes this fail, before any user is refused. The derivation of
//! the defaults is in docs/CONFIG.md, "Per-client defaults".
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::middleware;
use rbitcoin_space::limits::{self, Limiter, is_static};
use rbitcoin_space::{Limits, WsLimits};
use serde_json::Value;
use tokio::sync::watch;
use tower::ServiceExt;

/// Tabs loading at the same moment from one address: the acceptance check's three browsers.
const AT_ONCE: usize = 3;
/// Tabs a restored browser session loads one after another, inside one burst.
const SESSION: usize = 8;

fn pages() -> Vec<(String, Vec<String>, u64)> {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/cold-load.json")).unwrap();
    fixture["pages"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, p)| {
            let requests = p["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_str().unwrap().to_owned())
                .collect();
            (name.clone(), requests, p["sockets"].as_u64().unwrap())
        })
        .collect()
}

/// The real limits middleware in front of a handler that holds every request until released, so
/// all of them are in flight together: the worst case, harder than the measured peaks.
struct Server {
    router: Router,
    inside: Arc<AtomicUsize>,
    release: watch::Sender<bool>,
}

impl Server {
    fn new() -> Self {
        let inside = Arc::new(AtomicUsize::new(0));
        let (release, open) = watch::channel(false);
        let count = Arc::clone(&inside);
        let router = Router::new()
            .fallback(move || {
                let (count, mut open) = (Arc::clone(&count), open.clone());
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    let _ = open.wait_for(|o| *o).await;
                    StatusCode::OK
                }
            })
            .layer(middleware::from_fn_with_state(
                Arc::new(Limiter::new(&Limits::default(), false)),
                limits::limit,
            ));
        Self {
            router,
            inside,
            release,
        }
    }

    /// Sends every path at once from one client; returns each refused path with its status.
    async fn load(&self, paths: &[String]) -> Vec<(String, u16)> {
        let peer: SocketAddr = "192.0.2.1:4000".parse().unwrap();
        let tasks: Vec<_> = paths
            .iter()
            .map(|p| {
                let req = Request::get(p.as_str())
                    .extension(ConnectInfo(peer))
                    .body(Body::empty())
                    .unwrap();
                let router = self.router.clone();
                let path = p.clone();
                tokio::spawn(async move {
                    let status = router.oneshot(req).await.unwrap().status().as_u16();
                    (path, status)
                })
            })
            .collect();
        // Refused requests answer at once; the rest wait inside until all have arrived.
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while self.inside.load(Ordering::SeqCst) < paths.len() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        self.release.send_replace(true);
        let mut refused = Vec::new();
        for t in tasks {
            let (path, status) = t.await.unwrap();
            if status != 200 {
                refused.push((path, status));
            }
        }
        refused
    }
}

#[tokio::test]
async fn cold_loads_of_several_tabs_at_once_fit_the_default_limits() {
    let ws = WsLimits::default();
    for (name, requests, sockets) in pages() {
        let tabs: Vec<String> = (0..AT_ONCE).flat_map(|_| requests.clone()).collect();
        let refused = Server::new().load(&tabs).await;
        assert!(
            refused.is_empty(),
            "{AT_ONCE} cold loads of {name} at once were refused {refused:?}"
        );
        assert!(
            sockets * SESSION as u64 <= u64::from(ws.client_max_sockets),
            "{SESSION} tabs of {name} need more sockets than one client may open"
        );
    }
}

#[tokio::test]
async fn a_restored_session_fits_one_burst() {
    for (name, requests, _) in pages() {
        let server = Server::new();
        server.release.send_replace(true);
        for tab in 0..SESSION {
            let refused = server.load(&requests).await;
            assert!(
                refused.is_empty(),
                "tab {tab} of {SESSION} cold loads of {name} in a row was refused {refused:?}"
            );
        }
    }
}

#[test]
fn the_fixture_is_classified_as_the_limiter_sees_it() {
    // Every recorded page load has both kinds, and the API part is what the derivation counts.
    let mut heaviest_api = 0;
    for (name, requests, _) in pages() {
        let api = requests.iter().filter(|p| !is_static(p)).count();
        assert!(api > 0 && api < requests.len(), "{name}");
        heaviest_api = heaviest_api.max(api);
    }
    let l = Limits::default();
    assert!(heaviest_api * AT_ONCE <= l.client_requests as usize);
    assert!(heaviest_api * SESSION <= l.client_burst as usize);
}
