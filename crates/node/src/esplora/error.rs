use std::fmt;

use hyper::StatusCode;

/// Longest upstream error text we keep. The node's messages are one line.
const MAX_MESSAGE: usize = 512;

/// What went wrong with an Esplora call, in terms a caller can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The URL given to [`Esplora::new`](super::Esplora::new) is not usable.
    Config(&'static str),
    /// 404: no such object, or a bad `?asof=` block.
    NotFound,
    /// 503 `scripthash index disabled`: the node runs without `--sh-index`.
    AddressIndexDisabled,
    /// 503 from `--max-sh-creates`: the unpaged answer is too large. Use a paged route.
    TooLarge,
    /// 503 `chain view moved`, or two stamps of one read disagreed. Retry the read.
    ChainViewMoved,
    /// Another 4xx: the node refused the request. `message` is the node's text,
    /// for example a broadcast reject reason. It is chain-adjacent, untrusted text.
    Rejected { status: StatusCode, message: String },
    /// Connection failure, or a 5xx not covered above.
    Unavailable(String),
    /// The request did not complete within the configured timeout.
    Timeout,
    /// The response body exceeded the configured size cap.
    ResponseTooBig,
    /// The response did not decode into the expected type.
    Decode(String),
    /// An `/internal/*` route was called on a TCP endpoint; they exist only on a unix socket.
    InternalUnavailable,
}

impl Error {
    /// A fixed label for the log: never the node's own text, which can carry chain data.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Unavailable(_) => "unavailable",
            Self::Rejected { .. } => "refused",
            Self::NotFound => "not_found",
            Self::Decode(_) => "malformed",
            Self::TooLarge | Self::ResponseTooBig => "too_big",
            Self::ChainViewMoved => "chain_moved",
            Self::Config(_) | Self::AddressIndexDisabled | Self::InternalUnavailable => "config",
        }
    }

    /// Maps a non-200 upstream answer to an error.
    pub fn from_status(status: StatusCode, body: &[u8]) -> Self {
        let message = message(body);
        match status {
            StatusCode::NOT_FOUND => Self::NotFound,
            StatusCode::SERVICE_UNAVAILABLE => {
                if message.starts_with("scripthash index disabled") {
                    Self::AddressIndexDisabled
                } else if message.contains("--max-sh-creates") {
                    Self::TooLarge
                } else if message == "chain view moved" {
                    Self::ChainViewMoved
                } else {
                    Self::Unavailable(format!("{status}: {message}"))
                }
            }
            s if s.is_client_error() => Self::Rejected { status, message },
            _ => Self::Unavailable(format!("{status}: {message}")),
        }
    }
}

fn message(body: &[u8]) -> String {
    let body = body.get(..MAX_MESSAGE).unwrap_or(body);
    String::from_utf8_lossy(body).trim().to_owned()
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) => write!(f, "invalid esplora endpoint: {m}"),
            Self::NotFound => f.write_str("not found"),
            Self::AddressIndexDisabled => f.write_str("address index disabled on the node"),
            Self::TooLarge => f.write_str("history too large for an unpaged answer"),
            Self::ChainViewMoved => f.write_str("chain view moved"),
            Self::Rejected { status, message } => write!(f, "rejected ({status}): {message}"),
            Self::Unavailable(m) => write!(f, "esplora unavailable: {m}"),
            Self::Timeout => f.write_str("esplora request timed out"),
            Self::ResponseTooBig => f.write_str("esplora response too big"),
            Self::Decode(m) => write!(f, "esplora response did not decode: {m}"),
            Self::InternalUnavailable => f.write_str("internal routes need a unix-socket endpoint"),
        }
    }
}

impl std::error::Error for Error {}

impl From<crate::http::Error> for Error {
    fn from(e: crate::http::Error) -> Self {
        use crate::http::Error as H;
        match e {
            H::Config(m) => Self::Config(m),
            H::Unavailable(m) => Self::Unavailable(m),
            H::Timeout => Self::Timeout,
            H::ResponseTooBig => Self::ResponseTooBig,
        }
    }
}
