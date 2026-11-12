//! LDK's logger, turned into [`Event::Ldk`] values: filtered by level, cut to a bounded length and
//! rate-limited, with the number held back reported in [`Event::LdkSuppressed`].

use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::Instant;

use lightning::bitcoin::secp256k1::PublicKey;
use lightning::util::logger::{Level, Logger, Record};
use tokio::sync::broadcast;

use crate::Event;

/// Longest message kept, in bytes. LDK messages can quote peer-supplied text.
const MAX_MESSAGE_BYTES: usize = 512;

/// One LDK log record. `message` is LDK's free text and may quote what a peer sent: treat it as
/// untrusted.
#[derive(Clone, Debug)]
pub struct LdkRecord {
    pub level: Level,
    pub module: &'static str,
    pub line: u32,
    pub peer: Option<PublicKey>,
    pub message: String,
}

pub(crate) struct LogSink {
    min_level: Level,
    per_second: u32,
    window: Mutex<Window>,
    events: broadcast::Sender<Event>,
}

struct Window {
    start: Instant,
    sent: u32,
    suppressed: u64,
}

impl LogSink {
    pub(crate) fn new(min_level: Level, per_second: u32, events: broadcast::Sender<Event>) -> Self {
        Self {
            min_level,
            per_second,
            window: Mutex::new(Window {
                start: Instant::now(),
                sent: 0,
                suppressed: 0,
            }),
            events,
        }
    }

    /// Reports records held back in a window that has since ended. Called on the supervisor's tick
    /// so a burst followed by silence is still reported.
    pub(crate) fn flush(&self) {
        let Ok(mut w) = self.window.lock() else {
            return;
        };
        if w.start.elapsed().as_secs() >= 1 && w.suppressed > 0 {
            let _ = self.events.send(Event::LdkSuppressed {
                count: w.suppressed,
            });
            w.suppressed = 0;
        }
    }
}

impl Logger for LogSink {
    fn log(&self, record: Record) {
        if record.level < self.min_level {
            return;
        }
        {
            let Ok(mut w) = self.window.lock() else {
                return;
            };
            if w.start.elapsed().as_secs() >= 1 {
                if w.suppressed > 0 {
                    let _ = self.events.send(Event::LdkSuppressed {
                        count: w.suppressed,
                    });
                }
                *w = Window {
                    start: Instant::now(),
                    sent: 0,
                    suppressed: 0,
                };
            }
            if w.sent >= self.per_second {
                w.suppressed += 1;
                return;
            }
            w.sent += 1;
        }
        let mut message = Bounded(String::new());
        let _ = write!(message, "{}", record.args);
        let _ = self.events.send(Event::Ldk(LdkRecord {
            level: record.level,
            module: record.module_path,
            line: record.line,
            peer: record.peer_id,
            message: message.0,
        }));
    }
}

/// A string that stops growing at [`MAX_MESSAGE_BYTES`], on a character boundary.
struct Bounded(String);

impl std::fmt::Write for Bounded {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for c in s.chars() {
            if self.0.len() + c.len_utf8() > MAX_MESSAGE_BYTES {
                return Err(std::fmt::Error);
            }
            self.0.push(c);
        }
        Ok(())
    }
}
