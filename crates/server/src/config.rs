//! Process configuration, read once from the environment at startup. `docs/CONFIG.md` lists every variable.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use axum::http::HeaderName;

/// A chain the server can serve. Mainnet is at the root, the others under their own path prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Network {
    Mainnet,
    Signet,
    Testnet4,
    Regtest,
}

impl Network {
    pub const ALL: [Self; 4] = [Self::Mainnet, Self::Signet, Self::Testnet4, Self::Regtest];

    pub fn name(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Signet => "signet",
            Self::Testnet4 => "testnet4",
            Self::Regtest => "regtest",
        }
    }

    /// Where this network's pages and APIs are mounted: `""` for mainnet, `"/signet"` and so on.
    pub fn path_prefix(self) -> &'static str {
        match self {
            Self::Mainnet => "",
            Self::Signet => "/signet",
            Self::Testnet4 => "/testnet4",
            Self::Regtest => "/regtest",
        }
    }

    fn env_prefix(self) -> &'static str {
        match self {
            Self::Mainnet => "MAINNET",
            Self::Signet => "SIGNET",
            Self::Testnet4 => "TESTNET4",
            Self::Regtest => "REGTEST",
        }
    }
}

/// A value that must never reach a log line, error or response. `Debug` prints a placeholder and
/// there is no `Display`, so the only way to read it is to call [`Secret::expose`] on purpose.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Where the node's Esplora REST server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EsploraEndpoint {
    /// `http://` base URL, without a trailing slash.
    Http(String),
    /// Absolute path of a unix socket.
    Unix(PathBuf),
}

/// JSON-RPC endpoint with HTTP Basic credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcEndpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Secret,
}

/// Everything needed to reach one network's node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    pub network: Network,
    pub rpc: RpcEndpoint,
    pub esplora: EsploraEndpoint,
    /// Electrum TCP address as `host:port`.
    pub electrum: String,
}

/// Bounds on what clients can make the server do (SECURITY.md rules 6 and 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_body_bytes: usize,
    /// Also how long a connection may take to send request headers, or sit idle between requests.
    pub request_timeout: Duration,
    pub max_connections: usize,
    pub max_requests: usize,
    pub client_connections: u32,
    pub client_requests: u32,
    pub client_rate_per_sec: u32,
    pub client_burst: u32,
    pub max_tracked_clients: usize,
    /// Header a trusted reverse proxy sets to the client address. `None`: use the socket peer.
    pub trusted_proxy_header: Option<HeaderName>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_body_bytes: 16 * 1024,
            request_timeout: Duration::from_secs(10),
            max_connections: 1024,
            max_requests: 256,
            client_connections: 32,
            client_requests: 16,
            client_rate_per_sec: 20,
            client_burst: 60,
            max_tracked_clients: 65_536,
            trusted_proxy_header: None,
        }
    }
}

/// Which requests get an `http_request` event (`LOG_REQUESTS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestLog {
    All,
    /// Status 400 and above, which includes every refusal.
    Failures,
    Off,
}

impl RequestLog {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Failures => "failures",
            Self::Off => "off",
        }
    }
}

/// Bounds on WebSocket connections (SECURITY.md rule 9). `docs/CONFIG.md` explains each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsLimits {
    pub max_sockets: usize,
    pub client_max_sockets: u32,
    pub max_message_bytes: usize,
    pub messages_per_sec: u32,
    pub max_subscriptions: usize,
    pub send_queue: usize,
    pub idle_timeout: Duration,
}

impl Default for WsLimits {
    fn default() -> Self {
        Self {
            max_sockets: 1024,
            client_max_sockets: 8,
            max_message_bytes: 1024,
            messages_per_sec: 10,
            max_subscriptions: 8,
            send_queue: 64,
            idle_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub web_dir: PathBuf,
    /// Enabled networks, in [`Network::ALL`] order.
    pub networks: Vec<NetworkConfig>,
    pub limits: Limits,
    pub ws: WsLimits,
    /// Whether `POST /api/tx` is served (`BROADCAST_ENABLED`).
    pub broadcast: bool,
    pub log_requests: RequestLog,
    /// Whether request events carry the client address (`LOG_CLIENT_ADDR`).
    pub log_client_addr: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// Parses configuration from a variable lookup. An empty value counts as unset.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |name: &str| get(name).filter(|v| !v.is_empty());

        let listen = match var("LISTEN_ADDR") {
            None => SocketAddr::from(([127, 0, 0, 1], 8080)),
            Some(v) => v
                .parse()
                .map_err(|_| "LISTEN_ADDR is not a socket address".to_owned())?,
        };
        let web_dir = var("WEB_DIR").map_or_else(|| "web/dist".into(), PathBuf::from);

        let mut networks = Vec::new();
        for network in Network::ALL {
            if let Some(n) = network_config(network, &var)? {
                networks.push(n);
            }
        }

        let d = Limits::default();
        let trusted_proxy_header = match var("TRUSTED_PROXY_HEADER") {
            None => None,
            Some(v) => Some(
                HeaderName::try_from(v.as_str())
                    .map_err(|_| "TRUSTED_PROXY_HEADER is not a valid header name".to_owned())?,
            ),
        };
        let limits = Limits {
            max_body_bytes: positive(&var, "MAX_BODY_BYTES", d.max_body_bytes)?,
            request_timeout: Duration::from_secs(positive(
                &var,
                "REQUEST_TIMEOUT_SECS",
                d.request_timeout.as_secs(),
            )?),
            max_connections: positive(&var, "MAX_CONNECTIONS", d.max_connections)?,
            max_requests: positive(&var, "MAX_CONCURRENT_REQUESTS", d.max_requests)?,
            client_connections: positive(&var, "CLIENT_MAX_CONNECTIONS", d.client_connections)?,
            client_requests: positive(&var, "CLIENT_MAX_CONCURRENT_REQUESTS", d.client_requests)?,
            client_rate_per_sec: positive(&var, "CLIENT_RATE_PER_SEC", d.client_rate_per_sec)?,
            client_burst: positive(&var, "CLIENT_BURST", d.client_burst)?,
            max_tracked_clients: positive(&var, "MAX_TRACKED_CLIENTS", d.max_tracked_clients)?,
            trusted_proxy_header,
        };
        // Semaphore permits are u32 and tokio caps them lower still; keep well inside both.
        if limits.max_connections > 1 << 20 || limits.max_requests > 1 << 20 {
            return Err(
                "MAX_CONNECTIONS and MAX_CONCURRENT_REQUESTS must be at most 1048576".into(),
            );
        }

        let w = WsLimits::default();
        let ws = WsLimits {
            max_sockets: positive(&var, "WS_MAX_SOCKETS", w.max_sockets)?,
            client_max_sockets: positive(&var, "WS_CLIENT_MAX_SOCKETS", w.client_max_sockets)?,
            max_message_bytes: positive(&var, "WS_MAX_MESSAGE_BYTES", w.max_message_bytes)?,
            messages_per_sec: positive(&var, "WS_MESSAGES_PER_SEC", w.messages_per_sec)?,
            max_subscriptions: positive(&var, "WS_MAX_SUBSCRIPTIONS", w.max_subscriptions)?,
            send_queue: positive(&var, "WS_SEND_QUEUE", w.send_queue)?,
            idle_timeout: Duration::from_secs(positive(
                &var,
                "WS_IDLE_TIMEOUT_SECS",
                w.idle_timeout.as_secs(),
            )?),
        };
        if ws.max_sockets > 1 << 20 || ws.send_queue > 1 << 16 || ws.max_message_bytes > 1 << 20 {
            return Err(
                "WS_MAX_SOCKETS must be at most 1048576, WS_SEND_QUEUE at most 65536 \
                 and WS_MAX_MESSAGE_BYTES at most 1048576"
                    .into(),
            );
        }

        let broadcast = match var("BROADCAST_ENABLED").as_deref() {
            None | Some("true") => true,
            Some("false") => false,
            Some(_) => return Err("BROADCAST_ENABLED must be true or false".into()),
        };

        let log_requests = match var("LOG_REQUESTS").as_deref() {
            None | Some("all") => RequestLog::All,
            Some("failures") => RequestLog::Failures,
            Some("off") => RequestLog::Off,
            Some(_) => return Err("LOG_REQUESTS must be all, failures or off".into()),
        };
        let log_client_addr = match var("LOG_CLIENT_ADDR").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => return Err("LOG_CLIENT_ADDR must be true or false".into()),
        };

        Ok(Self {
            listen,
            web_dir,
            networks,
            limits,
            ws,
            broadcast,
            log_requests,
            log_client_addr,
        })
    }
}

fn positive<T>(var: &dyn Fn(&str) -> Option<String>, name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + Default,
{
    let Some(v) = var(name) else {
        return Ok(default);
    };
    match v.parse::<T>() {
        Ok(n) if n > T::default() => Ok(n),
        _ => Err(format!("{name} must be a positive integer")),
    }
}

/// `Ok(None)` when none of the network's variables are set; an error when only some are.
fn network_config(
    network: Network,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<NetworkConfig>, String> {
    let p = network.env_prefix();
    let names = [
        "RPC_HOST",
        "RPC_PORT",
        "RPC_USER",
        "RPC_PASSWORD",
        "ESPLORA_URL",
        "ELECTRUM_ADDR",
    ]
    .map(|suffix| format!("{p}_{suffix}"));
    let values = names.clone().map(|n| var(&n));
    let missing: Vec<&str> = names
        .iter()
        .zip(&values)
        .filter(|(_, v)| v.is_none())
        .map(|(n, _)| n.as_str())
        .collect();
    if missing.len() == names.len() {
        return Ok(None);
    }
    if !missing.is_empty() {
        return Err(format!(
            "{} is partly configured; also set {}",
            network.name(),
            missing.join(", ")
        ));
    }
    let [
        Some(host),
        Some(port),
        Some(user),
        Some(password),
        Some(esplora),
        Some(electrum),
    ] = values
    else {
        return Err(format!("{} is partly configured", network.name()));
    };

    if !is_host(&host) {
        return Err(format!("{p}_RPC_HOST is not a host name or IP address"));
    }
    let port = parse_port(&port).ok_or_else(|| format!("{p}_RPC_PORT is not a port number"))?;
    let esplora = parse_esplora(&esplora).ok_or_else(|| {
        format!(
            "{p}_ESPLORA_URL must be a plain http URL without credentials, or an absolute unix socket path"
        )
    })?;
    if !matches!(host_port(&electrum), Some((_, Some(_)))) {
        return Err(format!("{p}_ELECTRUM_ADDR must be host:port"));
    }

    Ok(Some(NetworkConfig {
        network,
        rpc: RpcEndpoint {
            host,
            port,
            user,
            password: Secret(password),
        },
        esplora,
        electrum,
    }))
}

fn parse_port(s: &str) -> Option<u16> {
    s.parse().ok().filter(|&p| p != 0)
}

/// A DNS name, an IP address, or a bracketed IPv6 address; nothing that could carry a path or userinfo.
fn is_host(s: &str) -> bool {
    if let Some(v6) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return v6.parse::<Ipv6Addr>().is_ok();
    }
    let dns_name = s.len() <= 253
        && s.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
    s.parse::<IpAddr>().is_ok() || dns_name
}

/// Splits `host[:port]`. An IPv6 host must be bracketed here, or its colons would read as a port.
fn host_port(s: &str) -> Option<(&str, Option<u16>)> {
    let (host, port) = match s.rfind(']') {
        Some(end) => {
            let (host, rest) = s.split_at(end + 1);
            match rest.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None if rest.is_empty() => (host, None),
                None => return None,
            }
        }
        None => match s.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (s, None),
        },
    };
    let port = match port {
        Some(p) => Some(parse_port(p)?),
        None => None,
    };
    is_host(host).then_some((host, port))
}

fn parse_esplora(s: &str) -> Option<EsploraEndpoint> {
    if s.starts_with('/') {
        return Some(EsploraEndpoint::Unix(PathBuf::from(s)));
    }
    // The node speaks plain HTTP; the client has no TLS.
    let rest = s.strip_prefix("http://")?;
    let authority = rest.split('/').next().unwrap_or_default();
    // Credentials in the URL would end up in logs; whitespace and control bytes have no place in a URL.
    let ok = host_port(authority).is_some()
        && node::esplora::Endpoint::parse(s).is_ok()
        && !s
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control());
    ok.then(|| EsploraEndpoint::Http(s.trim_end_matches('/').to_owned()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn parse(vars: &[(&str, &str)]) -> Result<Config, String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_vars(|k| map.get(k).cloned())
    }

    fn signet() -> Vec<(&'static str, &'static str)> {
        vec![
            ("SIGNET_RPC_HOST", "node.example"),
            ("SIGNET_RPC_PORT", "38332"),
            ("SIGNET_RPC_USER", "u"),
            ("SIGNET_RPC_PASSWORD", "hunter2-very-secret"),
            ("SIGNET_ESPLORA_URL", "http://node.example:3002/"),
            ("SIGNET_ELECTRUM_ADDR", "node.example:60601"),
        ]
    }

    #[test]
    fn defaults_with_nothing_set() {
        let c = parse(&[]).unwrap();
        assert_eq!(c.listen.to_string(), "127.0.0.1:8080");
        assert!(c.networks.is_empty());
        assert_eq!(c.limits, Limits::default());
    }

    #[test]
    fn a_fully_set_network_is_enabled() {
        let c = parse(&signet()).unwrap();
        assert_eq!(c.networks.len(), 1);
        let n = &c.networks[0];
        assert_eq!(n.network, Network::Signet);
        assert_eq!(n.rpc.port, 38332);
        assert_eq!(n.rpc.password.expose(), "hunter2-very-secret");
        assert_eq!(
            n.esplora,
            EsploraEndpoint::Http("http://node.example:3002".into())
        );
    }

    #[test]
    fn unix_socket_esplora() {
        let mut v = signet();
        v[4].1 = "/run/rbitcoin/esplora.sock";
        let c = parse(&v).unwrap();
        assert_eq!(
            c.networks[0].esplora,
            EsploraEndpoint::Unix("/run/rbitcoin/esplora.sock".into())
        );
    }

    #[test]
    fn ip_literals_and_ports_are_accepted() {
        let mut v = signet();
        v[0].1 = "::1";
        v[4].1 = "http://[::1]:3002/api";
        v[5].1 = "[::1]:60601";
        assert!(parse(&v).is_ok());
        v[0].1 = "[2001:db8::1]";
        v[4].1 = "http://192.0.2.1";
        v[5].1 = "192.0.2.1:50001";
        assert!(parse(&v).is_ok());
    }

    #[test]
    fn half_set_network_names_what_is_missing() {
        let mut v = signet();
        v.remove(3);
        let e = parse(&v).unwrap_err();
        assert!(e.contains("signet is partly configured"), "{e}");
        assert!(e.contains("SIGNET_RPC_PASSWORD"), "{e}");
        // Empty counts as unset.
        let mut v = signet();
        v[2].1 = "";
        assert!(parse(&v).unwrap_err().contains("SIGNET_RPC_USER"));
    }

    #[test]
    fn invalid_values_are_refused_without_echoing_them() {
        let cases = [
            (1, "0"),
            (1, "99999"),
            (0, "host/path"),
            (0, "user@host"),
            (4, "ftp://x"),
            (4, "https://node.example"),
            (4, "http://node.example:3000/?x=1"),
            (4, "http://user:pw@host"),
            (4, "relative/socket"),
            (4, "http://"),
            (5, "no-port"),
            (5, "host:0"),
            (5, "[[bad:42"),
            (5, "::1:50001"),
            (0, ":::["),
            (0, "a..b"),
            (4, "http://node.example:99999"),
            (4, "http://[::1"),
        ];
        for (i, bad) in cases {
            let mut v = signet();
            v[i].1 = bad;
            let e = parse(&v).unwrap_err();
            assert!(e.contains(v[i].0), "{bad}: {e}");
            assert!(!e.contains(bad), "{bad} echoed in {e}");
        }
    }

    #[test]
    fn limits_parse_and_must_be_positive() {
        let c = parse(&[
            ("MAX_BODY_BYTES", "1024"),
            ("REQUEST_TIMEOUT_SECS", "3"),
            ("CLIENT_RATE_PER_SEC", "5"),
            ("TRUSTED_PROXY_HEADER", "X-Real-IP"),
        ])
        .unwrap();
        assert_eq!(c.limits.max_body_bytes, 1024);
        assert_eq!(c.limits.request_timeout, Duration::from_secs(3));
        assert_eq!(c.limits.client_rate_per_sec, 5);
        assert_eq!(c.limits.trusted_proxy_header.unwrap(), "x-real-ip");
        for bad in ["0", "-1", "abc", "1.5"] {
            let e = parse(&[("CLIENT_BURST", bad)]).unwrap_err();
            assert!(e.contains("CLIENT_BURST"), "{e}");
        }
        assert!(parse(&[("TRUSTED_PROXY_HEADER", "bad header")]).is_err());
        assert!(parse(&[]).unwrap().broadcast);
        assert!(!parse(&[("BROADCAST_ENABLED", "false")]).unwrap().broadcast);
        assert!(parse(&[("BROADCAST_ENABLED", "no")]).is_err());
        let c = parse(&[]).unwrap();
        assert_eq!(
            (c.log_requests, c.log_client_addr),
            (RequestLog::All, false)
        );
        let c = parse(&[("LOG_REQUESTS", "failures"), ("LOG_CLIENT_ADDR", "true")]).unwrap();
        assert_eq!(
            (c.log_requests, c.log_client_addr),
            (RequestLog::Failures, true)
        );
        assert!(parse(&[("LOG_REQUESTS", "some")]).is_err());
        assert!(parse(&[("LOG_CLIENT_ADDR", "yes")]).is_err());
        assert!(parse(&[("MAX_CONNECTIONS", "99999999")]).is_err());
        assert_eq!(parse(&[]).unwrap().ws, WsLimits::default());
        let c = parse(&[("WS_SEND_QUEUE", "3"), ("WS_IDLE_TIMEOUT_SECS", "2")]).unwrap();
        assert_eq!(
            (c.ws.send_queue, c.ws.idle_timeout),
            (3, Duration::from_secs(2))
        );
        assert!(parse(&[("WS_MAX_SUBSCRIPTIONS", "0")]).is_err());
        assert!(parse(&[("WS_SEND_QUEUE", "99999999")]).is_err());
        let e = parse(&[("LISTEN_ADDR", "pasted-secret")]).unwrap_err();
        assert!(!e.contains("pasted-secret"), "{e}");
    }

    #[test]
    fn debug_output_never_contains_the_password() {
        let c = parse(&signet()).unwrap();
        let s = format!("{c:?} {:#?}", c.networks);
        assert!(!s.contains("hunter2"), "{s}");
        assert!(s.contains("redacted"));
    }
}
