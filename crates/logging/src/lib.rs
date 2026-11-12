//! Structured logging. Every line is a declared event type; `docs/LOGGING.md` has the rules and
//! the catalogue.
//!
//! [`emit`] formats an event on the caller's thread and hands the line to a bounded queue. One
//! writer thread drains it to the output. When the queue is full the event is dropped and
//! counted, and the count is written as its own event once the queue drains, so a slow or
//! blocked stdout never blocks a caller. A failed write is ignored, never a panic.
//!
//! Above a ceiling of bytes per second ([`Settings::max_bytes_per_sec`]), events that are not
//! [`Event::KEEP`] are dropped before they are formatted, and counted by kind the same way.

pub mod events;
mod format;

use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

pub use format::timestamp;

/// Lines waiting for the writer. Past this, events are dropped and counted.
pub const QUEUE_LINES: usize = 4096;
/// How often the writer wakes to report drops when nothing else is logged.
const WAKE_EVERY: Duration = Duration::from_millis(100);
/// At most one `log_events_dropped` per this, so a sustained overload writes one line a second.
/// The first after a quiet second is written at once.
const REPORT_DROPS_EVERY: Duration = Duration::from_secs(1);
/// The ceiling's burst: this many seconds' worth of bytes may be written at once.
pub const BURST_SECS: u64 = 10;
/// Default ceiling (`LOG_MAX_BYTES_PER_SEC`): about 900 `http_request` lines a second, which
/// a busy honest site does not reach, and at most 22.6 GB a day (docs/LOGGING.md).
pub const DEFAULT_MAX_BYTES_PER_SEC: u64 = 256 * 1024;

/// Severity, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

/// One thing worth recording. Implement it on a struct with named fields in [`events`] and
/// add the event to `docs/LOGGING.md`.
pub trait Event: Serialize {
    /// Stable snake_case name, written as the `event` key.
    const NAME: &'static str;
    const LEVEL: Level;
    /// Never dropped by the ceiling. Every `warn` and `error` is, since rule 3 already summarises
    /// them; an `info` event only when the code fixes its rate whatever clients do.
    const KEEP: bool = matches!(Self::LEVEL, Level::Warn | Level::Error);
}

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JSON object per line: `ts`, `level`, `event`, then the event's fields.
    Json,
    /// `ts LEVEL event key=value ...`, for a terminal.
    Text,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Text => "text",
        }
    }
}

/// What to write and how, from `LOG_FORMAT` and `LOG_LEVEL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub format: Format,
    /// Events below this level are not formatted at all.
    pub level: Level,
    /// The ceiling (`LOG_MAX_BYTES_PER_SEC`): events that are not [`Event::KEEP`] are dropped
    /// past this rate, after a burst of [`BURST_SECS`] seconds' worth.
    pub max_bytes_per_sec: u64,
}

impl Settings {
    /// JSON at `info`, or text at `info` when stdout is a terminal.
    pub fn default_for_stdout() -> Self {
        Settings {
            format: if io::stdout().is_terminal() {
                Format::Text
            } else {
                Format::Json
            },
            level: Level::Info,
            max_bytes_per_sec: DEFAULT_MAX_BYTES_PER_SEC,
        }
    }

    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok(), Self::default_for_stdout())
    }

    /// Parses `LOG_FORMAT`, `LOG_LEVEL` and `LOG_MAX_BYTES_PER_SEC` over `default`. An empty value counts as unset; an
    /// error names the variable without echoing its value.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>, default: Self) -> Result<Self, String> {
        let var = |name: &str| get(name).filter(|v| !v.is_empty());
        let format = match var("LOG_FORMAT").as_deref() {
            None => default.format,
            Some("json") => Format::Json,
            Some("text") => Format::Text,
            Some(_) => return Err("LOG_FORMAT must be json or text".into()),
        };
        let level = match var("LOG_LEVEL").as_deref() {
            None => default.level,
            Some("debug") => Level::Debug,
            Some("info") => Level::Info,
            Some("warn") => Level::Warn,
            Some("error") => Level::Error,
            Some(_) => return Err("LOG_LEVEL must be debug, info, warn or error".into()),
        };
        let max_bytes_per_sec = match var("LOG_MAX_BYTES_PER_SEC") {
            None => default.max_bytes_per_sec,
            Some(v) => match v.parse::<u64>() {
                Ok(n) if n >= 1024 => n,
                _ => return Err("LOG_MAX_BYTES_PER_SEC must be a whole number, 1024 or more".into()),
            },
        };
        Ok(Settings {
            format,
            level,
            max_bytes_per_sec,
        })
    }
}

struct Logger {
    settings: Settings,
    lines: SyncSender<Vec<u8>>,
    /// When the logger started; the ceiling counts time from it.
    started: Instant,
    budget: Budget,
    /// Events dropped since the last `log_events_dropped`, by cause and kind.
    drops: Mutex<Drops>,
    /// How many of them are not yet written and flushed, for [`flush`].
    unreported: AtomicU64,
    /// Lines queued and not yet written and flushed, for [`flush`].
    pending: AtomicU64,
}

impl Logger {
    fn new(settings: Settings) -> (Self, Receiver<Vec<u8>>) {
        let (lines, rx) = sync_channel(QUEUE_LINES);
        let logger = Logger {
            settings,
            lines,
            started: Instant::now(),
            budget: Budget::new(settings.max_bytes_per_sec),
            drops: Mutex::new(Drops::default()),
            unreported: AtomicU64::new(0),
            pending: AtomicU64::new(0),
        };
        (logger, rx)
    }

    /// [`emit`], with the time since `started` given, so tests can run it on their own clock.
    fn emit_at<E: Event>(&self, event: &E, now: Duration) {
        if E::LEVEL < self.settings.level {
            return;
        }
        // Checked before formatting, so an event over the ceiling costs a load and a count.
        if !E::KEEP && self.budget.over(now) {
            self.dropped(E::NAME, Cause::OverBudget);
            return;
        }
        let line = format::line(event, self.settings.format, SystemTime::now());
        if !E::KEEP {
            self.budget.charge(now, line.len());
        }
        self.pending.fetch_add(1, Ordering::Relaxed);
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = self.lines.try_send(line)
        {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            self.dropped(E::NAME, Cause::QueueFull);
        }
    }

    fn dropped(&self, name: &'static str, cause: Cause) {
        let mut d = self.drops.lock().unwrap_or_else(PoisonError::into_inner);
        match cause {
            Cause::QueueFull => d.queue_full += 1,
            Cause::OverBudget => d.over_budget += 1,
        }
        *d.events.entry(name).or_default() += 1;
        self.unreported.fetch_add(1, Ordering::Relaxed);
    }

    fn any_dropped(&self) -> bool {
        self.unreported.load(Ordering::Relaxed) > 0
    }
}

enum Cause {
    QueueFull,
    OverBudget,
}

/// Dropped events since the last report. `events` has one entry per event type at most.
#[derive(Debug, Default)]
struct Drops {
    queue_full: u64,
    over_budget: u64,
    events: BTreeMap<&'static str, u64>,
}

/// The ceiling, as a generic cell rate algorithm over bytes: `tat` is when the bytes charged so
/// far will have been paid for at the rate. Past `now` plus the burst, the budget is spent. One
/// atomic, so it costs every thread the same and never takes a lock.
#[derive(Debug)]
struct Budget {
    bytes_per_sec: u64,
    /// Nanoseconds since the logger started.
    tat: AtomicU64,
}

impl Budget {
    fn new(bytes_per_sec: u64) -> Self {
        Self {
            bytes_per_sec: bytes_per_sec.max(1),
            tat: AtomicU64::new(0),
        }
    }

    fn over(&self, now: Duration) -> bool {
        let limit = nanos(now).saturating_add(BURST_SECS * 1_000_000_000);
        self.tat.load(Ordering::Relaxed) > limit
    }

    /// Several threads may pass [`Budget::over`] together, so the ceiling can be overshot by
    /// one line per thread: bounded, and cheaper than a lock.
    fn charge(&self, now: Duration, bytes: usize) {
        let cost = u64::try_from(bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(1_000_000_000)
            / self.bytes_per_sec;
        let now = nanos(now);
        let _ = self
            .tat
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |tat| {
                Some(tat.max(now).saturating_add(cost))
            });
    }
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Starts the writer thread on `out`. Only the first call in a process takes effect; it returns
/// false otherwise, or if the thread could not be started. Events emitted before are discarded.
pub fn init(settings: Settings, out: impl Write + Send + 'static) -> bool {
    let mut rx = None;
    LOGGER.get_or_init(|| {
        let (logger, r) = Logger::new(settings);
        rx = Some(r);
        logger
    });
    let (Some(rx), Some(logger)) = (rx, LOGGER.get()) else {
        return false;
    };
    std::thread::Builder::new()
        .name("log-writer".into())
        .spawn(move || write_lines(logger, rx, out))
        .is_ok()
}

/// Records `event` if its level is enabled and the ceiling allows it. Never blocks and never
/// panics.
pub fn emit<E: Event>(event: &E) {
    if let Some(logger) = LOGGER.get() {
        logger.emit_at(event, logger.started.elapsed());
    }
}

/// Waits up to `limit` for every queued line, and the count of any dropped, to be written. For
/// process exit; gives up rather than hang on a blocked stdout.
pub fn flush(limit: Duration) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let start = Instant::now();
    let busy = || logger.pending.load(Ordering::Relaxed) > 0 || logger.any_dropped();
    while busy() && start.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Replaces the default panic message on stderr with a [`events::Panic`] event.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let thread = std::thread::current();
        emit(&events::Panic {
            message,
            location: &location,
            thread: thread.name().unwrap_or("unnamed"),
        });
    }));
}

fn write_lines(logger: &Logger, rx: Receiver<Vec<u8>>, out: impl Write) {
    let mut out = io::BufWriter::with_capacity(64 << 10, out);
    let mut reported: Option<Instant> = None;
    loop {
        // A producer can count a drop just after the last check below and then go quiet, so
        // wake periodically to report it rather than only when the next line arrives.
        let mut written = 0;
        match rx.recv_timeout(WAKE_EVERY) {
            Ok(first) => {
                let _ = out.write_all(&first);
                written += 1;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Batch whatever else is queued into one flush.
        while let Ok(line) = rx.try_recv() {
            let _ = out.write_all(&line);
            written += 1;
        }
        let due = reported.is_none_or(|at| at.elapsed() >= REPORT_DROPS_EVERY);
        let drops = if due && logger.any_dropped() {
            reported = Some(Instant::now());
            let mut d = logger.drops.lock().unwrap_or_else(PoisonError::into_inner);
            Some(std::mem::take(&mut *d))
        } else {
            None
        };
        if let Some(d) = &drops {
            let event = events::LogEventsDropped {
                count: d.queue_full + d.over_budget,
                queue_full: d.queue_full,
                over_budget: d.over_budget,
                events: &d.events,
            };
            let _ = out.write_all(&format::line(&event, logger.settings.format, SystemTime::now()));
        }
        let _ = out.flush();
        // Only now, so `flush` waits for the line, and drops counted meanwhile stay outstanding.
        if let Some(d) = drops {
            logger
                .unreported
                .fetch_sub(d.queue_full + d.over_budget, Ordering::Relaxed);
        }
        logger.pending.fetch_sub(written, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_and_name_the_variable() {
        let d = Settings {
            format: Format::Json,
            level: Level::Info,
            max_bytes_per_sec: DEFAULT_MAX_BYTES_PER_SEC,
        };
        let parse = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            Settings::from_vars(
                |k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()),
                d,
            )
        };
        assert_eq!(parse(&[]).unwrap(), d);
        assert_eq!(parse(&[("LOG_FORMAT", "")]).unwrap(), d);
        let s = parse(&[
            ("LOG_FORMAT", "text"),
            ("LOG_LEVEL", "warn"),
            ("LOG_MAX_BYTES_PER_SEC", "65536"),
        ])
        .unwrap();
        assert_eq!(
            (s.format, s.level, s.max_bytes_per_sec),
            (Format::Text, Level::Warn, 65536)
        );
        for bad in ["0", "1023", "-1", "1e6"] {
            let e = parse(&[("LOG_MAX_BYTES_PER_SEC", bad)]).unwrap_err();
            assert!(e.contains("LOG_MAX_BYTES_PER_SEC"), "{e}");
        }
        let e = parse(&[("LOG_LEVEL", "verbose-secret")]).unwrap_err();
        assert!(
            e.contains("LOG_LEVEL") && !e.contains("verbose-secret"),
            "{e}"
        );
        assert!(
            parse(&[("LOG_FORMAT", "xml")])
                .unwrap_err()
                .contains("LOG_FORMAT")
        );
    }

    #[derive(Serialize)]
    struct Chatty {
        n: u64,
    }
    impl Event for Chatty {
        const NAME: &'static str = "chatty";
        const LEVEL: Level = Level::Info;
    }

    #[derive(Serialize)]
    struct Vital {}
    impl Event for Vital {
        const NAME: &'static str = "vital";
        const LEVEL: Level = Level::Info;
        const KEEP: bool = true;
    }

    #[derive(Serialize)]
    struct Alarm {}
    impl Event for Alarm {
        const NAME: &'static str = "alarm";
        const LEVEL: Level = Level::Warn;
    }

    /// Emits `chatty` at `now` until the ceiling drops one; the bytes it let through.
    fn fill(l: &Logger, rx: &Receiver<Vec<u8>>, now: Duration) -> usize {
        let before = l.drops.lock().unwrap().over_budget;
        let mut bytes = 0;
        for n in 0.. {
            l.emit_at(&Chatty { n }, now);
            bytes += rx.try_iter().map(|line| line.len()).sum::<usize>();
            if l.drops.lock().unwrap().over_budget > before {
                return bytes;
            }
        }
        unreachable!()
    }

    #[test]
    fn the_ceiling_drops_only_what_may_be_dropped_and_counts_each_kind() {
        let (l, rx) = Logger::new(Settings {
            format: Format::Json,
            level: Level::Debug,
            max_bytes_per_sec: 1000,
        });
        let line = format::line(&Chatty { n: 0 }, Format::Json, SystemTime::now()).len() + 8;
        // The burst: ten seconds' worth at once, overshot by one line at most.
        let t0 = Duration::from_secs(5);
        let burst = fill(&l, &rx, t0);
        assert!((10_000..10_000 + line).contains(&burst), "{burst}");
        // Spent: kept events still pass, every one, and are not charged.
        for _ in 0..100 {
            l.emit_at(&Vital {}, t0);
            l.emit_at(&Alarm {}, t0);
        }
        assert_eq!(rx.try_iter().count(), 200);
        l.emit_at(&Chatty { n: 0 }, t0);
        assert_eq!(rx.try_iter().count(), 0, "still over");
        // The rate refills: one second later, a second's worth.
        let refill = fill(&l, &rx, t0 + Duration::from_secs(1));
        assert!((1000 - line..1000 + line).contains(&refill), "{refill}");
        // Counted by kind and cause; the kept kinds never appear.
        let d = l.drops.lock().unwrap();
        assert_eq!((d.queue_full, d.over_budget), (0, 3));
        assert_eq!(d.events, BTreeMap::from([("chatty", 3)]));
        assert_eq!(l.unreported.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_full_queue_drops_even_kept_events_and_counts_them() {
        let (l, rx) = Logger::new(Settings {
            format: Format::Json,
            level: Level::Info,
            max_bytes_per_sec: u64::MAX,
        });
        for _ in 0..QUEUE_LINES + 5 {
            l.emit_at(&Alarm {}, Duration::ZERO);
        }
        assert_eq!(rx.try_iter().count(), QUEUE_LINES);
        let d = l.drops.lock().unwrap();
        assert_eq!((d.queue_full, d.over_budget), (5, 0));
        assert_eq!(d.events, BTreeMap::from([("alarm", 5)]));
    }

    #[test]
    fn warn_and_error_are_kept_and_info_only_when_marked() {
        const {
            assert!(Alarm::KEEP && Vital::KEEP && !Chatty::KEEP);
            assert!(events::Panic::KEEP && events::ServerStarting::KEEP);
            assert!(!events::HttpRequest::KEEP && !events::WsClosed::KEEP);
        }
    }

    #[test]
    fn levels_are_ordered() {
        assert!(Level::Debug < Level::Info && Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
    }
}
