//! Structured logging. Every line is a declared event type; `docs/LOGGING.md` has the rules and
//! the catalogue.
//!
//! [`emit`] formats an event on the caller's thread and hands the line to a bounded queue. One
//! writer thread drains it to the output. When the queue is full the event is dropped and
//! counted, and the count is written as its own event once the queue drains, so a slow or
//! blocked stdout never blocks a caller. A failed write is ignored, never a panic.

pub mod events;
mod format;

use std::io::{self, IsTerminal, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

pub use format::timestamp;

/// Lines waiting for the writer. Past this, events are dropped and counted.
pub const QUEUE_LINES: usize = 4096;
/// Longest a dropped-events count waits to be written when nothing else is logged.
const REPORT_DROPS_EVERY: Duration = Duration::from_millis(100);

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
        }
    }

    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok(), Self::default_for_stdout())
    }

    /// Parses `LOG_FORMAT` and `LOG_LEVEL` over `default`. An empty value counts as unset; an
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
        Ok(Settings { format, level })
    }
}

struct Logger {
    settings: Settings,
    lines: SyncSender<Vec<u8>>,
    /// Events dropped since the last `log_events_dropped`.
    dropped: AtomicU64,
    /// Lines queued and not yet written and flushed, for [`flush`].
    pending: AtomicU64,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Starts the writer thread on `out`. Only the first call in a process takes effect; it returns
/// false otherwise, or if the thread could not be started. Events emitted before are discarded.
pub fn init(settings: Settings, out: impl Write + Send + 'static) -> bool {
    let (lines, rx) = sync_channel(QUEUE_LINES);
    let mut fresh = false;
    LOGGER.get_or_init(|| {
        fresh = true;
        Logger {
            settings,
            lines,
            dropped: AtomicU64::new(0),
            pending: AtomicU64::new(0),
        }
    });
    if !fresh {
        return false;
    }
    std::thread::Builder::new()
        .name("log-writer".into())
        .spawn(move || write_lines(rx, out))
        .is_ok()
}

/// Records `event` if its level is enabled. Never blocks and never panics.
pub fn emit<E: Event>(event: &E) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    if E::LEVEL < logger.settings.level {
        return;
    }
    let line = format::line(event, logger.settings.format, SystemTime::now());
    logger.pending.fetch_add(1, Ordering::Relaxed);
    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = logger.lines.try_send(line)
    {
        logger.pending.fetch_sub(1, Ordering::Relaxed);
        logger.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Waits up to `limit` for every queued line, and the count of any dropped, to be written. For
/// process exit; gives up rather than hang on a blocked stdout.
pub fn flush(limit: Duration) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let start = Instant::now();
    let busy =
        || logger.pending.load(Ordering::Relaxed) > 0 || logger.dropped.load(Ordering::Relaxed) > 0;
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

fn write_lines(rx: Receiver<Vec<u8>>, out: impl Write) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let mut out = io::BufWriter::with_capacity(64 << 10, out);
    loop {
        // A producer can count a drop just after the last check below and then go quiet, so
        // wake periodically to report it rather than only when the next line arrives.
        let mut written = 0;
        match rx.recv_timeout(REPORT_DROPS_EVERY) {
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
        // Read, not cleared: the count stays outstanding for `flush` until its line is out, and
        // drops counted meanwhile survive the subtraction below.
        let dropped = logger.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            let event = events::LogEventsDropped { count: dropped };
            let line = format::line(&event, logger.settings.format, SystemTime::now());
            let _ = out.write_all(&line);
        }
        let _ = out.flush();
        logger.dropped.fetch_sub(dropped, Ordering::Relaxed);
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
        let s = parse(&[("LOG_FORMAT", "text"), ("LOG_LEVEL", "warn")]).unwrap();
        assert_eq!((s.format, s.level), (Format::Text, Level::Warn));
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

    #[test]
    fn levels_are_ordered() {
        assert!(Level::Debug < Level::Info && Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
    }
}
