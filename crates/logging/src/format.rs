//! Turning an event into exactly one line of output.
//!
//! Field values may hold text an attacker chose. Both formats keep every value inside its field
//! and every event on one line: serde_json escapes quotes, backslashes and C0 controls, and
//! [`escape_unsafe`] escapes the remaining characters a log viewer or terminal could treat as a
//! line break or a control sequence.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::{Event, Format, Level};

/// The full output line for `event`, newline included.
pub(crate) fn line<E: Event>(event: &E, format: Format, at: SystemTime) -> Vec<u8> {
    let ts = timestamp(at);
    let out = match format {
        Format::Json => json(event, &ts),
        Format::Text => text(event, &ts),
    };
    let mut out = escape_unsafe(out).into_bytes();
    out.push(b'\n');
    out
}

fn json<E: Event>(event: &E, ts: &str) -> String {
    let fields = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_owned());
    let mut out = format!(
        r#"{{"ts":"{ts}","level":"{}","event":"{}""#,
        E::LEVEL.as_str(),
        E::NAME
    );
    match fields.strip_prefix('{') {
        Some("}") => out.push('}'),
        Some(rest) => {
            out.push(',');
            out.push_str(rest);
        }
        // Not a struct with named fields: keep it whole under one key.
        None => {
            out.push_str(",\"value\":");
            out.push_str(&fields);
            out.push('}');
        }
    }
    out
}

fn text<E: Event>(event: &E, ts: &str) -> String {
    let mut out = format!("{ts} {:<5} {}", E::LEVEL.as_str().to_uppercase(), E::NAME);
    match serde_json::to_value(event) {
        Ok(Value::Object(fields)) => {
            for (key, value) in &fields {
                let _ = write!(out, " {key}={}", text_value(value));
            }
        }
        Ok(value) => {
            let _ = write!(out, " value={}", text_value(&value));
        }
        Err(_) => {}
    }
    out
}

/// Plain words stay bare; anything with spaces, quotes, `=` or non-ASCII is quoted and escaped.
fn text_value(value: &Value) -> String {
    match value {
        Value::String(s)
            if !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'=' | b'\\')) =>
        {
            s.clone()
        }
        other => other.to_string(),
    }
}

/// Escapes, as `\uXXXX`, characters serde_json leaves raw that could still break a line or
/// fool a reader: DEL, C1 controls (NEL and the 8-bit terminal CSI among them), the Unicode line
/// and paragraph separators, and bidirectional overrides. They can only appear inside JSON
/// strings here, where a `\uXXXX` escape is valid.
pub(crate) fn escape_unsafe(s: String) -> String {
    if !s.chars().any(unsafe_char) {
        return s;
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        if unsafe_char(c) {
            let _ = write!(out, "\\u{:04x}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

fn unsafe_char(c: char) -> bool {
    matches!(c,
        '\u{7f}'..='\u{9f}'
        | '\u{200e}' | '\u{200f}'
        | '\u{2028}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}')
}

/// RFC 3339 in UTC with milliseconds, e.g. `2024-02-29T23:59:59.999Z`. A clock set before 1970
/// reads as the epoch.
pub fn timestamp(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (y, m, d) = civil_from_days(secs / 86_400);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date, after Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::time::Duration;

    use serde::Serialize;

    use super::*;

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn timestamps_at_known_instants() {
        assert_eq!(timestamp(at(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(at(951_782_400_000)), "2000-02-29T00:00:00.000Z");
        assert_eq!(timestamp(at(951_868_800_000)), "2000-03-01T00:00:00.000Z");
        assert_eq!(timestamp(at(1_709_251_199_999)), "2024-02-29T23:59:59.999Z");
        assert_eq!(timestamp(at(1_709_251_200_000)), "2024-03-01T00:00:00.000Z");
        assert_eq!(timestamp(at(4_102_444_799_123)), "2099-12-31T23:59:59.123Z");
        assert_eq!(
            timestamp(UNIX_EPOCH - Duration::from_secs(5)),
            "1970-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn every_day_for_a_thousand_years_round_trips() {
        // Count days forward and check the date advances by exactly one calendar day.
        let (mut y, mut m, mut d) = (1970, 1, 1);
        for days in 0..366 * 1000 {
            assert_eq!(civil_from_days(days), (y, m, d), "day {days}");
            let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
            let len = match m {
                2 if leap => 29,
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            d += 1;
            if d > len {
                (d, m) = (1, m + 1);
            }
            if m > 12 {
                (m, y) = (1, y + 1);
            }
        }
    }

    #[derive(Serialize)]
    struct Probe<'a> {
        text: &'a str,
        n: u32,
    }

    impl Event for Probe<'_> {
        const NAME: &'static str = "probe";
        const LEVEL: Level = Level::Info;
    }

    #[derive(Serialize)]
    struct Empty {}

    impl Event for Empty {
        const NAME: &'static str = "empty";
        const LEVEL: Level = Level::Warn;
    }

    const HOSTILE: &[&str] = &[
        "line\nbreak",
        "carriage\rreturn",
        "quote\" , \"injected\":\"yes",
        "back\\slash\\",
        "nul\0bell\x07esc\x1b[31mred",
        "del\x7f nel\u{85} csi\u{9b}31m",
        "sep\u{2028}para\u{2029}",
        "bidi\u{202e}desrever\u{2066}x\u{2069}",
        "key=value other=thing",
        "",
    ];

    fn invalid_utf8() -> String {
        String::from_utf8_lossy(b"bad\xff\xfe\xc0utf8\n").into_owned()
    }

    #[test]
    fn json_lines_hold_hostile_text_in_its_field() {
        let mut cases: Vec<String> = HOSTILE.iter().map(|s| (*s).to_owned()).collect();
        cases.push(invalid_utf8());
        for text in &cases {
            let out = line(&Probe { text, n: 7 }, Format::Json, at(0));
            let s = String::from_utf8(out).unwrap();
            assert_eq!(s.matches('\n').count(), 1, "{s}");
            assert!(s.ends_with('\n'));
            assert!(!s.chars().any(|c| c.is_control() && c != '\n'), "{s:?}");
            assert!(!s.chars().any(unsafe_char), "{s:?}");
            let v: Value = serde_json::from_str(&s).unwrap();
            assert!(s.starts_with(r#"{"ts":"#), "{s}");
            let mut keys: Vec<_> = v.as_object().unwrap().keys().collect();
            keys.sort_unstable();
            assert_eq!(keys, ["event", "level", "n", "text", "ts"]);
            assert_eq!(v["text"], text.as_str(), "value must round-trip");
            assert_eq!(v["event"], "probe");
            assert_eq!(v["level"], "info");
            assert_eq!(v["ts"], "1970-01-01T00:00:00.000Z");
        }
    }

    #[test]
    fn text_lines_hold_hostile_text_in_its_field() {
        let mut cases: Vec<String> = HOSTILE.iter().map(|s| (*s).to_owned()).collect();
        cases.push(invalid_utf8());
        for text in &cases {
            let out = line(&Probe { text, n: 7 }, Format::Text, at(0));
            let s = String::from_utf8(out).unwrap();
            assert_eq!(s.matches('\n').count(), 1, "{s}");
            assert!(!s.chars().any(|c| c.is_control() && c != '\n'), "{s:?}");
            assert!(!s.chars().any(unsafe_char), "{s:?}");
            let rest = s
                .strip_prefix("1970-01-01T00:00:00.000Z INFO  probe n=7 text=")
                .unwrap();
            // The value is either one bare word or one JSON string that decodes back to it.
            let value = rest.trim_end_matches('\n');
            if value.starts_with('"') {
                let back: String = serde_json::from_str(value).unwrap();
                assert_eq!(&back, text);
            } else {
                assert_eq!(value, text);
                assert!(!value.contains(' '));
            }
        }
    }

    #[test]
    fn plain_values_stay_bare_in_text() {
        let out = line(
            &Probe {
                text: "/api/tx/{txid}",
                n: 1,
            },
            Format::Text,
            at(1_709_251_199_999),
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "2024-02-29T23:59:59.999Z INFO  probe n=1 text=/api/tx/{txid}\n"
        );
    }

    #[test]
    fn events_without_fields() {
        let json = String::from_utf8(line(&Empty {}, Format::Json, at(0))).unwrap();
        assert_eq!(
            json,
            "{\"ts\":\"1970-01-01T00:00:00.000Z\",\"level\":\"warn\",\"event\":\"empty\"}\n"
        );
        let text = String::from_utf8(line(&Empty {}, Format::Text, at(0))).unwrap();
        assert_eq!(text, "1970-01-01T00:00:00.000Z WARN  empty\n");
    }
}
