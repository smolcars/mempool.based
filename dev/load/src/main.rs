//! Load generator for the server: HTTP latency and throughput at a number of concurrent clients,
//! and WebSocket fan-out to many subscribers. Method and results: docs/PERFORMANCE.md.
//!
//! ```text
//! load http <http://host:port> <path>... [--clients N] [--seconds S] [--ips N] [--rate R] [--pid PID]
//! load ws <ws://host:port/path> [--sockets N] [--seconds S] [--topics a,b] [--block-every K]
//!         [--slow N] [--ips N] [--pid PID] [--server-log FILE] [--drain S]
//! ```
//!
//! `--ips N` names each client as one of N addresses in `X-Forwarded-For` (the server must trust
//! 127.0.0.1 for it); 0, the default, sends no header, so every client is 127.0.0.1. `--pid`
//! samples that process's CPU time and resident memory. A JSON summary goes to stdout. An HTTP
//! request still unanswered [`GRACE`] after `--seconds` is counted as a timeout and the run ends.
//! A WebSocket run counts messages first seen during `--seconds`, then keeps reading for
//! `--drain` seconds (default 15) so a late reader is still counted; a message's last-reader
//! time is reported only when every reader subscribed to it got it.
#![allow(clippy::print_stdout, clippy::print_stderr)] // a command-line tool: printing is its output

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hasher};
use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Semaphore;

type Sender = hyper::client::conn::http1::SendRequest<Empty<Bytes>>;

/// How long past `--seconds` an HTTP request may still be in flight before it counts as a timeout.
const GRACE: Duration = Duration::from_secs(5);
/// A WebSocket that has not finished its upgrade and subscription by then counts as refused.
const WS_CONNECT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("http") => http(Opts::parse(args.get(1..).unwrap_or_default())).await,
        Some("ws") => ws(Opts::parse(args.get(1..).unwrap_or_default())).await,
        _ => Err("usage: load http|ws <url> [options]; see the file's header".into()),
    };
    match result {
        Ok(summary) => println!("{summary}"),
        Err(e) => {
            eprintln!("load: {e}");
            std::process::exit(2);
        }
    }
}

/// Positional arguments and `--name value` options.
struct Opts {
    positional: Vec<String>,
    named: HashMap<String, String>,
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut positional = Vec::new();
        let mut named = HashMap::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.strip_prefix("--") {
                Some(name) => {
                    named.insert(name.to_owned(), it.next().cloned().unwrap_or_default());
                }
                None => positional.push(a.clone()),
            }
        }
        Self { positional, named }
    }

    fn num(&self, name: &str, default: u64) -> Result<u64, String> {
        self.named.get(name).map_or(Ok(default), |v| {
            v.parse()
                .map_err(|_| format!("--{name}: not a number: {v}"))
        })
    }
}

/// `host:port` and the path of an `http://` or `ws://` URL.
fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("ws://"))
        .ok_or_else(|| format!("not an http:// or ws:// URL: {url}"))?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    Ok((
        host.to_owned(),
        if path.is_empty() {
            "/".into()
        } else {
            path.into()
        },
    ))
}

/// The `n`th client's address, when clients are named in `X-Forwarded-For`.
fn client_ip(i: u64, ips: u64) -> Option<String> {
    let n = i.checked_rem(ips)?;
    Some(format!(
        "10.{}.{}.{}",
        (n >> 16) & 255,
        (n >> 8) & 255,
        n & 255
    ))
}

fn now_us() -> u64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
}

/// The nearest-rank percentile of sorted values.
fn percentile(sorted: &[u32], p: f64) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted
        .get(rank.saturating_sub(1).min(sorted.len() - 1))
        .copied()
        .unwrap_or(0)
}

/// Median, p90, p99 and max of latencies in microseconds, as milliseconds.
fn spread(mut us: Vec<u32>) -> Value {
    us.sort_unstable();
    let ms = |v: u32| (f64::from(v) / 10.0).round() / 100.0;
    json!({
        "n": us.len(),
        "p50": ms(percentile(&us, 50.0)),
        "p90": ms(percentile(&us, 90.0)),
        "p99": ms(percentile(&us, 99.0)),
        "max": ms(us.last().copied().unwrap_or(0)),
    })
}

// --- process sampling ------------------------------------------------------------------------

/// CPU seconds from `ps -o time=`: `[[dd-]hh:]mm:ss[.cc]`.
fn parse_cpu_time(s: &str) -> Option<f64> {
    let (days, clock) = match s.trim().split_once('-') {
        Some((d, c)) => (d.parse::<f64>().ok()?, c),
        None => (0.0, s.trim()),
    };
    let mut secs = 0.0;
    for part in clock.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + secs)
}

/// Samples a process's CPU time and resident memory every half second until stopped.
struct Sampler {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<(Instant, f64, u64)>>>,
}

impl Sampler {
    fn start(pid: Option<u64>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = pid.map(|pid| {
            std::thread::spawn(move || {
                let mut samples = Vec::new();
                loop {
                    let out = Command::new("ps")
                        .args(["-o", "time=,rss=", "-p", &pid.to_string()])
                        .output();
                    if let Ok(out) = out {
                        let text = String::from_utf8_lossy(&out.stdout).to_string();
                        let mut f = text.split_whitespace();
                        if let (Some(cpu), Some(rss)) = (
                            f.next().and_then(parse_cpu_time),
                            f.next().and_then(|r| r.parse().ok()),
                        ) {
                            samples.push((Instant::now(), cpu, rss));
                        }
                    }
                    if flag.load(Ordering::Relaxed) {
                        return samples;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            })
        });
        Self { stop, thread }
    }

    /// Mean and busiest-second CPU (percent of one core) and peak resident memory.
    fn finish(self) -> Value {
        self.stop.store(true, Ordering::Relaxed);
        let Some(samples) = self.thread.and_then(|t| t.join().ok()) else {
            return Value::Null;
        };
        let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
            return Value::Null;
        };
        let pct = |a: &(Instant, f64, u64), b: &(Instant, f64, u64)| {
            let wall = b.0.duration_since(a.0).as_secs_f64();
            if wall > 0.0 {
                (b.1 - a.1) / wall * 100.0
            } else {
                0.0
            }
        };
        // Pairs a second apart, so a coarse `ps` clock still reads sensibly.
        let busiest = samples
            .iter()
            .zip(samples.iter().skip(2))
            .map(|(a, b)| pct(a, b))
            .fold(0.0_f64, f64::max);
        let rss = samples.iter().map(|s| s.2).max().unwrap_or(0);
        json!({
            "cpu_pct_mean": (pct(first, last) * 10.0).round() / 10.0,
            "cpu_pct_busiest_second": (busiest * 10.0).round() / 10.0,
            "rss_mb_start": first.2 / 1024,
            "rss_mb_max": rss / 1024,
        })
    }
}

fn own_cpu(started: f64, wall: Duration) -> Value {
    let now = own_cpu_secs().unwrap_or(started);
    json!((((now - started) / wall.as_secs_f64()) * 1000.0).round() / 10.0)
}

fn own_cpu_secs() -> Option<f64> {
    let out = Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    parse_cpu_time(&String::from_utf8_lossy(&out.stdout))
}

// --- HTTP ------------------------------------------------------------------------------------

#[derive(Default)]
struct PathStats {
    ok_us: Vec<u32>,
    refused_us: Vec<u32>,
    status: BTreeMap<u16, u64>,
    bytes: u64,
}

#[derive(Default)]
struct HttpResults {
    paths: HashMap<String, PathStats>,
    connect_errors: u64,
    request_errors: u64,
    timeouts: u64,
}

async fn connect(addr: &str) -> io::Result<Sender> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    tokio::spawn(async move { conn.await.ok() });
    Ok(sender)
}

struct HttpRun {
    addr: String,
    paths: Vec<String>,
    ips: u64,
    /// Requests a second per client; 0 sends the next as soon as the last is answered.
    rate: f64,
    deadline: Instant,
}

async fn http_client(id: u64, run: Arc<HttpRun>) -> HttpResults {
    let mut out = HttpResults::default();
    let ip = client_ip(id, run.ips);
    let mut sender: Option<Sender> = None;
    let mut next = Instant::now();
    let mut i = usize::try_from(id).unwrap_or(0);
    let stop = tokio::time::Instant::from_std(run.deadline + GRACE);
    while Instant::now() < run.deadline {
        if run.rate > 0.0 {
            if next >= run.deadline {
                break;
            }
            tokio::time::sleep_until(next.into()).await;
            next += Duration::from_secs_f64(1.0 / run.rate);
        }
        let mut s = match sender.take() {
            Some(s) if s.is_ready() => s,
            _ => match tokio::time::timeout_at(stop, connect(&run.addr)).await {
                Ok(Ok(s)) => s,
                Ok(Err(_)) => {
                    out.connect_errors += 1;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                Err(_) => {
                    out.timeouts += 1;
                    break;
                }
            },
        };
        let Some(path) = run.paths.get(i % run.paths.len().max(1)) else {
            break;
        };
        i = i.wrapping_add(1);
        let mut req = hyper::Request::get(path.as_str()).header(hyper::header::HOST, &run.addr);
        if let Some(ip) = &ip {
            req = req.header("x-forwarded-for", ip);
        }
        let Ok(req) = req.body(Empty::new()) else {
            break;
        };
        let started = Instant::now();
        let answer = async {
            let res = s.send_request(req).await.ok()?;
            let status = res.status().as_u16();
            let body = res.into_body().collect().await.ok()?.to_bytes();
            Some((status, body.len()))
        };
        match tokio::time::timeout_at(stop, answer).await {
            Err(_) => {
                out.timeouts += 1;
                break;
            }
            Ok(None) => out.request_errors += 1,
            Ok(Some((status, len))) => {
                let us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
                let stats = out.paths.entry(path.clone()).or_default();
                *stats.status.entry(status).or_default() += 1;
                stats.bytes += len as u64;
                if (200..300).contains(&status) {
                    stats.ok_us.push(us);
                } else {
                    stats.refused_us.push(us);
                }
                sender = Some(s);
            }
        }
    }
    out
}

async fn http(o: Opts) -> Result<Value, String> {
    let (base, paths) = o.positional.split_first().ok_or("load http: no base URL")?;
    if paths.is_empty() {
        return Err("load http: no paths".into());
    }
    let (addr, _) = split_url(base)?;
    let clients = o.num("clients", 1)?;
    let seconds = o.num("seconds", 10)?;
    let run = Arc::new(HttpRun {
        addr,
        paths: paths.to_vec(),
        ips: o.num("ips", 0)?,
        rate: o
            .named
            .get("rate")
            .and_then(|r| r.parse().ok())
            .unwrap_or(0.0),
        deadline: Instant::now() + Duration::from_secs(seconds),
    });
    let sampler = Sampler::start(o.named.get("pid").and_then(|p| p.parse().ok()));
    let own = own_cpu_secs().unwrap_or(0.0);
    let started = Instant::now();
    let tasks: Vec<_> = (0..clients)
        .map(|id| tokio::spawn(http_client(id, Arc::clone(&run))))
        .collect();
    let mut all = HttpResults::default();
    for t in tasks {
        let r = t.await.map_err(|e| e.to_string())?;
        all.connect_errors += r.connect_errors;
        all.request_errors += r.request_errors;
        all.timeouts += r.timeouts;
        for (path, s) in r.paths {
            let into = all.paths.entry(path).or_default();
            into.ok_us.extend(s.ok_us);
            into.refused_us.extend(s.refused_us);
            into.bytes += s.bytes;
            for (k, v) in s.status {
                *into.status.entry(k).or_default() += v;
            }
        }
    }
    let wall = started.elapsed();
    let server = sampler.finish();
    let mut ok = Vec::new();
    let mut refused = Vec::new();
    let mut status: BTreeMap<u16, u64> = BTreeMap::new();
    let mut per_path = serde_json::Map::new();
    for path in paths {
        let Some(s) = all.paths.remove(path) else {
            continue;
        };
        let n = s.ok_us.len() + s.refused_us.len();
        ok.extend_from_slice(&s.ok_us);
        refused.extend_from_slice(&s.refused_us);
        for (k, v) in &s.status {
            *status.entry(*k).or_default() += v;
        }
        per_path.insert(
            path.clone(),
            json!({
                "requests": n,
                "status": s.status,
                "bytes_mean": s.bytes.checked_div(n as u64).unwrap_or(0),
                "ok_ms": spread(s.ok_us),
            }),
        );
    }
    let requests = ok.len() + refused.len();
    Ok(json!({
        "mode": "http",
        "clients": clients,
        "ips": run.ips,
        "rate_per_client": run.rate,
        "seconds": (wall.as_secs_f64() * 10.0).round() / 10.0,
        "requests": requests,
        "per_sec": (requests as f64 / wall.as_secs_f64()).round(),
        "ok_per_sec": (ok.len() as f64 / wall.as_secs_f64()).round(),
        "status": status,
        "connect_errors": all.connect_errors,
        "request_errors": all.request_errors,
        "timeouts": all.timeouts,
        "ok_ms": spread(ok),
        "refused_ms": spread(refused),
        "server": server,
        "load_tool_cpu_pct": own_cpu(own, wall),
        "paths": per_path,
    }))
}

// --- WebSocket -------------------------------------------------------------------------------

/// A client frame: always masked (RFC 6455 5.3), with a fixed key, which the server only removes.
fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    const MASK: [u8; 4] = [0x6d, 0x62, 0x61, 0x73];
    let mut f = vec![0x80 | opcode];
    match payload.len() {
        n @ 0..=125 => f.push(0x80 | n as u8),
        n => {
            f.push(0x80 | 126);
            f.extend_from_slice(&u16::try_from(n).unwrap_or(u16::MAX).to_be_bytes());
        }
    }
    f.extend_from_slice(&MASK);
    f.extend(payload.iter().zip(MASK.iter().cycle()).map(|(b, m)| b ^ m));
    f
}

/// Reads one frame into `buf`; returns its opcode.
async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> io::Result<u8> {
    let mut head = [0u8; 2];
    r.read_exact(&mut head).await?;
    let [b0, b1] = head;
    let len = match b1 & 0x7f {
        126 => u64::from(r.read_u16().await?),
        127 => r.read_u64().await?,
        n => u64::from(n),
    };
    // The server's largest message is a projected block's snapshot, well under this.
    let len = usize::try_from(len)
        .ok()
        .filter(|&n| n <= 64 << 20)
        .ok_or_else(|| io::Error::other("frame too large"))?;
    let mask = if b1 & 0x80 != 0 {
        Some(r.read_u32().await?.to_be_bytes())
    } else {
        None
    };
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    if let Some(mask) = mask {
        buf.iter_mut()
            .zip(mask.iter().cycle())
            .for_each(|(b, m)| *b ^= m);
    }
    Ok(b0 & 0x0f)
}

/// Opens a socket and returns its halves after the `101`, or the refusal's status code.
async fn ws_open(
    addr: &str,
    path: &str,
    ip: Option<&str>,
) -> Result<(BufReader<OwnedReadHalf>, OwnedWriteHalf), String> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| e.kind().to_string())?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let (r, mut w) = stream.into_split();
    let xff = ip
        .map(|ip| format!("X-Forwarded-For: {ip}\r\n"))
        .unwrap_or_default();
    // The key is fixed: this client does not check the server's `Sec-WebSocket-Accept`.
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{xff}\r\n"
    );
    w.write_all(req.as_bytes())
        .await
        .map_err(|e| e.kind().to_string())?;
    let mut r = BufReader::with_capacity(16 << 10, r);
    let mut status = String::new();
    r.read_line(&mut status)
        .await
        .map_err(|e| e.kind().to_string())?;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)
            .await
            .map_err(|e| e.kind().to_string())?
            == 0
            || line == "\r\n"
        {
            break;
        }
    }
    match status.split_whitespace().nth(1) {
        Some("101") => Ok((r, w)),
        Some(code) => Err(code.to_owned()),
        None => Err("no status".into()),
    }
}

/// One distinct message, identified by its bytes: every subscriber gets the same ones.
struct Seen {
    label: String,
    /// The topic it belongs to, which says how many readers should get it.
    topic: String,
    bytes: usize,
    first_us: u64,
    last_us: u64,
    /// Readers that got it; the same bytes read again by one reader do not move `last_us`.
    readers: HashSet<u32>,
}

type Messages = Arc<Mutex<HashMap<u64, Seen>>>;

/// A message's label (topic and type, or binary kind and block) and its topic.
fn label(opcode: u8, payload: &[u8]) -> (String, String) {
    if opcode == 2 {
        // Byte 0 is the kind, byte 2 the projected block (docs/WS.md, binary frames).
        let kind = payload.first().copied().unwrap_or(0);
        let block = payload.get(2).copied().unwrap_or(0);
        return (
            format!("binary:{kind}:{block}"),
            format!("projected-txs:{block}"),
        );
    }
    let v: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("-").to_owned();
    (format!("{}:{}", s("topic"), s("type")), s("topic"))
}

async fn ws_reader(
    id: u32,
    mut r: BufReader<OwnedReadHalf>,
    mut w: OwnedWriteHalf,
    seen: Messages,
) {
    let mut buf = Vec::new();
    loop {
        let Ok(opcode) = read_frame(&mut r, &mut buf).await else {
            return;
        };
        let at = now_us();
        match opcode {
            1 | 2 => {
                let mut h = DefaultHasher::new();
                h.write(&buf);
                let key = h.finish();
                let mut seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
                let e = seen.entry(key).or_insert_with(|| {
                    let (label, topic) = label(opcode, &buf);
                    Seen {
                        label,
                        topic,
                        bytes: buf.len(),
                        first_us: at,
                        last_us: at,
                        readers: HashSet::new(),
                    }
                });
                if e.readers.insert(id) {
                    e.first_us = e.first_us.min(at);
                    e.last_us = e.last_us.max(at);
                }
            }
            9 => {
                if w.write_all(&client_frame(10, &buf)).await.is_err() {
                    return;
                }
            }
            8 => return,
            _ => {}
        }
    }
}

/// `2026-11-12T10:00:00.123Z` as milliseconds since the epoch.
fn parse_ts_ms(ts: &str) -> Option<u64> {
    let num = |r: std::ops::Range<usize>| ts.get(r).and_then(|s| s.parse::<u64>().ok());
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss, ms) = (num(11..13)?, num(14..16)?, num(17..19)?, num(20..23)?);
    // Days from the civil date (Howard Hinnant's algorithm), for years after 1970.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era * 146_097 + doe).checked_sub(719_468)?;
    Some(((days * 86_400 + hh * 3600 + mm * 60 + ss) * 1000) + ms)
}

/// The server's `ws_fanout` events for blocks, as (published at in ms, queueing ms, node to
/// queued ms), and the largest mempool fan-out it reported, from its JSON log.
fn server_fanouts(log: &str, since_ms: u64) -> (Vec<(f64, f64, f64)>, f64) {
    let mut blocks = Vec::new();
    let mut mempool_max = 0.0_f64;
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(ts) = v.get("ts").and_then(Value::as_str).and_then(parse_ts_ms) else {
            continue;
        };
        if ts < since_ms {
            continue;
        }
        let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        match v.get("event").and_then(Value::as_str) {
            Some("ws_fanout") if v.get("kind").and_then(Value::as_str) == Some("block") => {
                blocks.push((ts as f64 - f("fanout_ms"), f("fanout_ms"), f("duration_ms")));
            }
            Some("mempool_summary") => mempool_max = mempool_max.max(f("fanout_ms_max")),
            _ => {}
        }
    }
    (blocks, mempool_max)
}

async fn ws(o: Opts) -> Result<Value, String> {
    let url = o.positional.first().ok_or("load ws: no URL")?;
    let (addr, path) = split_url(url)?;
    let sockets = o.num("sockets", 100)?;
    let slow = o.num("slow", 0)?;
    let seconds = o.num("seconds", 30)?;
    let every = o.num("block-every", 0)?;
    let ips = o.num("ips", 0)?;
    let drain = o.num("drain", 15)?;
    let topics: Vec<String> = o
        .named
        .get("topics")
        .map_or("blocks,mempool,projected,transactions", String::as_str)
        .split(',')
        .map(str::to_owned)
        .collect();
    let seen: Messages = Messages::default();
    let sampler = Sampler::start(o.named.get("pid").and_then(|p| p.parse().ok()));
    let own = own_cpu_secs().unwrap_or(0.0);
    let started = Instant::now();

    // Connect in waves the listener's backlog can take.
    let gate = Arc::new(Semaphore::new(200));
    let open = Arc::new(AtomicUsize::new(0));
    let refused: Arc<Mutex<BTreeMap<String, u64>>> = Arc::default();
    let connect_us: Arc<Mutex<Vec<u32>>> = Arc::default();
    let mut held = Vec::new();
    let mut tasks = Vec::new();
    for id in 0..sockets + slow {
        let is_slow = id >= sockets;
        let mut want = topics.clone();
        if is_slow {
            // The largest topics, so a client that does not read fills its queue soonest.
            want.extend(["projected-txs:0".to_owned(), "projected-txs:1".to_owned()]);
        }
        if !is_slow && every > 0 && id % every == 0 {
            want.push("projected-txs:0".to_owned());
        }
        let sub = json!({ "op": "subscribe", "topics": &want }).to_string();
        let ip = client_ip(id, ips);
        let (addr, path, gate) = (addr.clone(), path.clone(), Arc::clone(&gate));
        let (open, refused, connect_us) = (
            Arc::clone(&open),
            Arc::clone(&refused),
            Arc::clone(&connect_us),
        );
        tasks.push(tokio::spawn(async move {
            let _permit = gate.acquire().await.ok()?;
            let t = Instant::now();
            let open_and_subscribe = async {
                let (r, mut w) = ws_open(&addr, &path, ip.as_deref()).await?;
                w.write_all(&client_frame(1, sub.as_bytes()))
                    .await
                    .map_err(|e| e.kind().to_string())?;
                Ok::<_, String>((r, w))
            };
            let opened = tokio::time::timeout(WS_CONNECT, open_and_subscribe)
                .await
                .unwrap_or_else(|_| Err("timeout".into()));
            match opened {
                Ok((r, w)) => {
                    connect_us
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(u32::try_from(t.elapsed().as_micros()).unwrap_or(u32::MAX));
                    open.fetch_add(1, Ordering::Relaxed);
                    Some((id, is_slow, want, r, w))
                }
                Err(code) => {
                    *refused
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .entry(code)
                        .or_default() += 1;
                    None
                }
            }
        }));
    }
    let mut readers = Vec::new();
    // Readers subscribed to each topic: how many should get each of its messages.
    let mut topic_readers: HashMap<String, usize> = HashMap::new();
    for t in tasks {
        if let Ok(Some((id, is_slow, want, r, w))) = t.await {
            if is_slow {
                held.push((r, w)); // never read
            } else {
                for topic in want {
                    *topic_readers.entry(topic).or_default() += 1;
                }
                let id = u32::try_from(id).unwrap_or(u32::MAX);
                readers.push(tokio::spawn(ws_reader(id, r, w, Arc::clone(&seen))));
            }
        }
    }
    let connected_s = started.elapsed().as_secs_f64();
    // Snapshots sent on subscribe are not fan-outs: count only what starts after everyone is in.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let from_us = now_us();
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    let until_us = now_us();
    let wall = started.elapsed();
    let server = sampler.finish();
    let load_cpu = own_cpu(own, wall);
    // Late readers of messages first seen in the window still count.
    tokio::time::sleep(Duration::from_secs(drain)).await;
    for r in &readers {
        r.abort();
    }
    let still_open_slow = held.len();
    drop(held);

    let log = o
        .named
        .get("server-log")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let (fanouts, mempool_fanout_max) = server_fanouts(&log, from_us / 1000);
    let seen = std::mem::take(&mut *seen.lock().unwrap_or_else(PoisonError::into_inner));
    let mut by_label: BTreeMap<String, Vec<&Seen>> = BTreeMap::new();
    for s in seen
        .values()
        .filter(|s| s.first_us >= from_us && s.first_us < until_us)
    {
        by_label.entry(s.label.clone()).or_default().push(s);
    }
    let mut messages = serde_json::Map::new();
    let expected = |s: &Seen| {
        topic_readers
            .get(&s.topic)
            .copied()
            .unwrap_or(readers.len())
    };
    for (label, all) in &by_label {
        let (list, incomplete): (Vec<&Seen>, Vec<&Seen>) =
            all.iter().partition(|s| s.readers.len() >= expected(s));
        let spread_us: Vec<u32> = list
            .iter()
            .map(|s| u32::try_from(s.last_us - s.first_us).unwrap_or(u32::MAX))
            .collect();
        let mut entry = json!({
            "messages": all.len(),
            "expected_readers": all.first().map(|s| expected(s)),
            "complete": list.len(),
            "incomplete": incomplete.len(),
            "incomplete_received_min": incomplete.iter().map(|s| s.readers.len()).min(),
            "bytes_max": all.iter().map(|s| s.bytes).max(),
            "first_to_last_ms": spread(spread_us),
        });
        if label == "blocks:block" {
            // Join each complete block with the server's fan-out event: published, to the last reader.
            let mut ready_to_last = Vec::new();
            let mut node_to_last = Vec::new();
            let mut node_to_queued = Vec::new();
            for s in &list {
                let first_ms = s.first_us as f64 / 1000.0;
                let best = fanouts
                    .iter()
                    .filter(|f| f.0 <= first_ms + 2.0 && f.0 > first_ms - 1000.0)
                    .max_by(|a, b| a.0.total_cmp(&b.0));
                if let Some(&(published, queueing, node_to_queued_ms)) = best {
                    let last_ms = s.last_us as f64 / 1000.0;
                    let to_last = ((last_ms - published).max(0.0) * 1000.0) as u32;
                    ready_to_last.push(to_last);
                    node_to_queued.push((node_to_queued_ms * 1000.0) as u32);
                    let node = node_to_queued_ms - queueing;
                    node_to_last.push(to_last.saturating_add((node.max(0.0) * 1000.0) as u32));
                }
            }
            if let Some(map) = entry.as_object_mut() {
                map.insert("published_to_last_ms".into(), spread(ready_to_last));
                map.insert("server_node_to_queued_ms".into(), spread(node_to_queued));
                map.insert("node_to_last_ms".into(), spread(node_to_last));
            }
        }
        messages.insert(label.clone(), entry);
    }
    let connect = std::mem::take(&mut *connect_us.lock().unwrap_or_else(PoisonError::into_inner));
    let refused = std::mem::take(&mut *refused.lock().unwrap_or_else(PoisonError::into_inner));
    Ok(json!({
        "mode": "ws",
        "sockets": sockets,
        "slow": slow,
        "open": open.load(Ordering::Relaxed),
        "refused": refused,
        "connect_ms": spread(connect),
        "connected_after_s": (connected_s * 10.0).round() / 10.0,
        "seconds": seconds,
        "drain_seconds": drain,
        "topics": topics,
        "block_topic_every": every,
        "slow_still_open_at_end": still_open_slow,
        "messages": messages,
        "server_mempool_fanout_ms_max": mempool_fanout_max,
        "server": server,
        "load_tool_cpu_pct": load_cpu,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_by_nearest_rank() {
        let v: Vec<u32> = (1..=100).collect();
        assert_eq!(percentile(&v, 50.0), 50);
        assert_eq!(percentile(&v, 99.0), 99);
        assert_eq!(percentile(&v, 100.0), 100);
        assert_eq!(percentile(&[7], 99.0), 7);
        assert_eq!(percentile(&[], 50.0), 0);
    }

    #[test]
    fn cpu_times_in_both_ps_formats() {
        assert_eq!(parse_cpu_time("0:01.50"), Some(1.5));
        assert_eq!(parse_cpu_time(" 01:02:03 "), Some(3723.0));
        assert_eq!(parse_cpu_time("1-00:00:01"), Some(86_401.0));
        assert_eq!(parse_cpu_time("x"), None);
    }

    #[test]
    fn log_timestamps() {
        assert_eq!(parse_ts_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_ts_ms("2024-03-01T00:00:00.000Z"),
            Some(1_709_251_200_000)
        );
        assert_eq!(
            parse_ts_ms("2099-12-31T23:59:59.123Z"),
            Some(4_102_444_799_123)
        );
    }

    #[tokio::test]
    async fn a_client_frame_reads_back() {
        let payload = vec![b'x'; 300];
        let frame = client_frame(1, &payload);
        let mut buf = Vec::new();
        assert_eq!(
            read_frame(&mut frame.as_slice(), &mut buf).await.unwrap(),
            1
        );
        assert_eq!(buf, payload);
    }

    #[test]
    fn messages_are_labelled_by_their_topic() {
        assert_eq!(
            label(2, &[2, 1, 1, 0]),
            ("binary:2:1".into(), "projected-txs:1".into())
        );
        assert_eq!(
            label(1, br#"{"topic":"blocks","type":"block"}"#),
            ("blocks:block".into(), "blocks".into())
        );
    }

    #[test]
    fn urls_and_client_addresses() {
        assert_eq!(
            split_url("ws://127.0.0.1:9/regtest/app/ws").unwrap(),
            ("127.0.0.1:9".into(), "/regtest/app/ws".into())
        );
        assert_eq!(split_url("http://h:1").unwrap(), ("h:1".into(), "/".into()));
        assert_eq!(client_ip(258, 1000).as_deref(), Some("10.0.1.2"));
        assert_eq!(client_ip(5, 0), None);
    }
}
