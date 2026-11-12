//! The one gate every request Lightning sends the node goes through (`docs/LIGHTNING.md`, Load
//! on the node): validation's lookups, the close-watcher's reads and the server's funding-txid
//! requests. One request at a time, in the order they ask; starts at least `min_gap` apart; and
//! after a request that took `t`, the node is left alone for `IDLE × t`, so Lightning keeps the
//! node busy at most a quarter of the time and slows down by itself when the node does.

use std::future::Future;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Idle time after a request, as a multiple of how long it took: busy at most 1 / (1 + IDLE).
pub(crate) const IDLE: u32 = 3;

pub(crate) struct Pace {
    min_gap: Duration,
    /// When the next request may start. Held across a request: that is the one-at-a-time.
    next: Mutex<Instant>,
}

impl Pace {
    pub(crate) fn new(min_gap: Duration) -> Self {
        Self {
            min_gap,
            next: Mutex::new(Instant::now()),
        }
    }

    /// Waits for the gate (tokio's mutex is first come, first served), then runs `request`. A
    /// timeout belongs inside `request`, so a request that hangs is charged its whole wait.
    pub(crate) async fn run<F: Future>(&self, request: F) -> F::Output {
        let mut next = self.next.lock().await;
        tokio::time::sleep_until(*next).await;
        let started = Instant::now();
        let out = request.await;
        let took = started.elapsed();
        *next = (started + self.min_gap).max(Instant::now() + took * IDLE);
        out
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Requests that each take `took`, sent by three tasks at once, as validation, the
    /// close-watcher and the funding fill do. Returns when each started.
    async fn three_loops(pace: Arc<Pace>, took: Duration, each: usize) -> Vec<Instant> {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let (pace, in_flight, starts) = (pace.clone(), in_flight.clone(), starts.clone());
            tasks.push(tokio::spawn(async move {
                for _ in 0..each {
                    pace.run(async {
                        assert_eq!(in_flight.fetch_add(1, Ordering::SeqCst), 0, "two at once");
                        starts.lock().unwrap().push(Instant::now());
                        tokio::time::sleep(took).await;
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let mut s = starts.lock().unwrap().clone();
        s.sort();
        s
    }

    #[tokio::test(start_paused = true)]
    async fn a_fast_node_gets_one_request_at_a_time_at_most_every_gap() {
        let gap = Duration::from_millis(250);
        let starts = three_loops(Arc::new(Pace::new(gap)), Duration::from_millis(10), 20).await;
        assert_eq!(starts.len(), 60);
        assert!(starts.windows(2).all(|w| w[1] - w[0] >= gap));
        // At most four a second over the whole run.
        let span = *starts.last().unwrap() - starts[0];
        assert!(span >= gap * 59, "{span:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_node_is_left_three_times_as_long_as_it_took() {
        let took = Duration::from_secs(2);
        let starts = three_loops(
            Arc::new(Pace::new(Duration::from_millis(250))),
            took,
            4,
        )
        .await;
        // Two seconds busy, then six idle: a request every eight seconds, a quarter busy.
        for w in starts.windows(2) {
            assert!(w[1] - w[0] >= took * (1 + IDLE), "{:?}", w[1] - w[0]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_that_times_out_is_charged_its_whole_wait() {
        let pace = Pace::new(Duration::from_millis(250));
        let limit = Duration::from_secs(10);
        let r = pace
            .run(tokio::time::timeout(limit, std::future::pending::<()>()))
            .await;
        assert!(r.is_err());
        let before = Instant::now();
        pace.run(async {}).await;
        assert!(Instant::now() - before >= limit * IDLE);
    }
}
