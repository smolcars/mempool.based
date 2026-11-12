//! Page paths: the app shell with a head that says what the page is, so a link pasted into a chat
//! previews as that page (docs/FRONTEND.md, "Link previews"). The head is built from the path's
//! parsed identifiers, the server's memory and the block table, never the node: anyone can ask
//! for any path at any rate (SECURITY.md rule 11).

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::Uri;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use engine::blocks::Row;
use engine::mining::pools::{self, UNKNOWN};
use tokio::sync::Semaphore;

use crate::config::Network;
use crate::{ApiError, NetworkContext, esplora, lightning};

/// Ends every title, as `usePageTitle` in `web/src/lib/page.ts` ends it.
const SITE: &str = "rbitcoin.space";

/// Block table reads for previews running at once, server-wide. Past it a block's head goes
/// without its facts, so a flood of block paths holds at most this many of a store's readers.
const STORE_READS: usize = 2;

/// The narrow no-break space the frontend groups digits with (`NNBSP` in `lib/format.ts`).
const NNBSP: char = '\u{202f}';

/// The built shell, read once at start, and what a head is made from.
pub(crate) struct Pages {
    shell: Option<Shell>,
    networks: Vec<Arc<NetworkContext>>,
    reads: Semaphore,
}

/// The build's `index.html`, split around its `<title>` element.
struct Shell {
    /// Served as built for a path that names no page we can describe.
    raw: String,
    /// Where the `<title>` element starts and ends; `None` if the build has none.
    title: Option<(usize, usize)>,
}

impl Shell {
    fn new(raw: String) -> Self {
        let start = raw.find("<title>");
        let end = start.and_then(|s| {
            raw.get(s..)?
                .find("</title>")
                .map(|e| s + e + "</title>".len())
        });
        Self {
            title: start.zip(end),
            raw,
        }
    }
}

/// What a page is: its title as the page sets it, without the network or the site, and one
/// sentence about it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Head {
    pub title: String,
    pub description: String,
}

impl Pages {
    pub(crate) fn new(web_dir: &Path, networks: Vec<Arc<NetworkContext>>) -> Self {
        Self {
            shell: std::fs::read_to_string(web_dir.join("index.html"))
                .ok()
                .map(Shell::new),
            networks,
            reads: Semaphore::new(STORE_READS),
        }
    }

    /// The head for `path`, or `None` for a path that is no page of an enabled network or whose
    /// identifier does not parse.
    async fn head(&self, path: &str, query: Option<&str>) -> Option<(Network, Head)> {
        let (network, rest) = split(path);
        let ctx = self.networks.iter().find(|c| c.config.network == network)?;
        let mut segs = rest.split('/');
        // `rest` is empty or starts with `/`, so the first segment is always empty.
        let s = [segs.next(), segs.next(), segs.next(), segs.next()];
        if segs.next().is_some() {
            return None;
        }
        let on = chain(network);
        let head = match s {
            [_, None | Some(""), None, None] => Head {
                title: "Blocks and mempool".into(),
                description: format!(
                    "{on}'s newest blocks, its mempool and the blocks it would fill next, live."
                ),
            },
            [_, Some("block"), Some("next"), n] => projected(on, n)?,
            [_, Some("block"), Some(id), None] => self.block(ctx, on, id).await?,
            [_, Some("blocks"), from, None] => {
                let from = from.map(height).transpose().ok()?;
                Head {
                    title: "Blocks".into(),
                    description: match from {
                        Some(h) => format!(
                            "{on}'s blocks from height {} down: their time, miner, transactions, fees and size.",
                            group(u64::from(h))
                        ),
                        None => format!(
                            "{on}'s blocks, newest first: their time, miner, transactions, fees and size."
                        ),
                    },
                }
            }
            [_, Some("tx"), Some(id), None] => {
                let txid = esplora::txid(id).ok()?.to_string();
                Head {
                    title: format!("Transaction {}", short(&txid)),
                    description: format!(
                        "{on} transaction {txid}: its inputs, outputs, fee and status."
                    ),
                }
            }
            [_, Some("address"), Some(a), None] => {
                let address = esplora::address(a, network).ok()?.to_string();
                Head {
                    title: format!("Address {}", short(&address)),
                    description: format!(
                        "{on} address {address}: its balance and its transactions."
                    ),
                }
            }
            [_, Some("mining"), None, None] => Head {
                title: "Mining".into(),
                description: format!(
                    "Who mines {on}'s blocks: each pool's share, the hashrate, the difficulty and the rewards."
                ),
            },
            [_, Some("mining"), Some("pool"), Some(slug)] => {
                let pool = pools::list().by_slug(slug)?;
                Head {
                    title: pool.name.clone(),
                    description: format!(
                        "Blocks mined by {} on {on}: its share, its hashrate and its newest blocks.",
                        pool.name
                    ),
                }
            }
            [_, Some("mempool"), None, None] => Head {
                title: "Mempool history".into(),
                description: format!(
                    "{on}'s mempool over time: its size by fee rate, and the flow in and out."
                ),
            },
            [_, Some("lightning"), None, None] => Head {
                title: "Lightning".into(),
                description: format!(
                    "The Lightning network on {on}, as this server's gossip peer sees it: nodes, channels and capacity over time."
                ),
            },
            [_, Some("lightning"), Some("node"), Some(key)] => node(ctx, on, key)?,
            [_, Some("lightning"), Some("channel"), Some(id)] => {
                let scid = lightning::nodes::scid(id).ok()?;
                // `scid` takes only canonical forms, so this writes the id back as it came.
                let id = if id.contains('x') {
                    lightning::scid_text(scid)
                } else {
                    scid.to_string()
                };
                Head {
                    title: format!("Lightning channel {id}"),
                    description: format!(
                        "Lightning channel {id} on {on}: its capacity, its two nodes and their fees."
                    ),
                }
            }
            [_, Some("lightning"), Some("search"), None] => Head {
                title: "Lightning search".into(),
                description: format!(
                    "Find Lightning nodes and channels on {on} by name, public key or channel id."
                ),
            },
            [_, Some("lightning"), Some("rankings"), None] => rankings(on, query),
            _ => return None,
        };
        Some((network, head))
    }

    async fn block(&self, ctx: &NetworkContext, on: &str, id: &str) -> Option<Head> {
        let by_height = height(id);
        let by_hash = match by_height {
            Ok(_) => None,
            Err(()) => Some(esplora::block_hash(id).ok()?),
        };
        let row = self.row(ctx, by_height.ok(), by_hash).await;
        Some(match (row, by_hash) {
            (Some(r), _) => {
                let pool = pools::list();
                let (id, _) = pool.attribute(&r.coinbase.tag, &r.coinbase.payouts);
                let miner = pool
                    .by_id(id)
                    .filter(|p| p.id != UNKNOWN)
                    .map_or_else(|| "an unknown miner".to_owned(), |p| p.name.clone());
                let time = crate::mining::iso8601(r.time);
                let time = time.get(..16).unwrap_or_default().replace('T', " ");
                Head {
                    title: format!("Block {}", group(u64::from(r.height))),
                    description: format!(
                        "{on} block {}, mined {time} UTC by {miner}: {} and {} BTC in fees.",
                        group(u64::from(r.height)),
                        count(r.tx_count, "transaction"),
                        btc(r.fees),
                    ),
                }
            }
            (None, Some(hash)) => {
                let hash = hash.to_string();
                Head {
                    title: format!("Block {}", short(&hash)),
                    description: format!(
                        "{on} block {hash}: its height, miner, fees and transactions."
                    ),
                }
            }
            (None, None) => {
                let h = group(u64::from(by_height.ok()?));
                Head {
                    title: format!("Block {h}"),
                    description: format!("{on} block {h}: its miner, fees and transactions."),
                }
            }
        })
    }

    /// The block table's row, if the table is open, holds the block, and a read slot is free.
    async fn row(
        &self,
        ctx: &NetworkContext,
        height: Option<u32>,
        hash: Option<bitcoin::BlockHash>,
    ) -> Option<Row> {
        let store = &ctx.blocks.as_ref()?.store;
        let _slot = self.reads.try_acquire().ok()?;
        match (height, hash) {
            (Some(h), _) => engine::blocks::by_height(store, h).await,
            (None, Some(h)) => engine::blocks::by_hash(store, h).await,
            (None, None) => return None,
        }
        .ok()
        .flatten()
    }
}

/// The network a page path is on, and the path after its prefix.
fn split(path: &str) -> (Network, &str) {
    Network::ALL
        .into_iter()
        .filter(|n| !n.path_prefix().is_empty())
        .find_map(|n| {
            let rest = path.strip_prefix(n.path_prefix())?;
            (rest.is_empty() || rest.starts_with('/')).then_some((n, rest))
        })
        .unwrap_or((Network::Mainnet, path))
}

/// How a description names the chain.
fn chain(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "Bitcoin",
        Network::Signet => "Signet",
        Network::Testnet4 => "Testnet4",
        Network::Regtest => "Regtest",
    }
}

/// What a test network's titles start with, as `NETWORK_NAMES` in `web/src/lib/network.ts`.
fn title_prefix(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "",
        Network::Signet => "Signet: ",
        Network::Testnet4 => "Testnet4: ",
        Network::Regtest => "Regtest: ",
    }
}

/// A height as the frontend takes one: 1 to 10 decimal digits within `u32` (`parseBlockId`).
fn height(s: &str) -> Result<u32, ()> {
    if s.is_empty() || s.len() > 10 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    s.parse().map_err(|_| ())
}

/// `/block/next` and `/block/next/:n`, `n` from 1 to 8, named as `positionName` names them.
fn projected(on: &str, n: Option<&str>) -> Option<Head> {
    const ORDINALS: [&str; 6] = ["Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh"];
    let n = match n {
        None => 1,
        Some(s) if s.len() == 1 => s.parse::<usize>().ok().filter(|n| (1..=8).contains(n))?,
        Some(_) => return None,
    };
    Some(match n {
        1 => Head {
            title: "Next block".into(),
            description: format!(
                "The block {on}'s mempool would fill next: its transactions and fee rates, live."
            ),
        },
        8 => Head {
            title: "The rest of the mempool".into(),
            description: format!(
                "Everything in {on}'s mempool past the next seven blocks: its transactions and fee rates, live."
            ),
        },
        n => {
            let name = ORDINALS.get(n - 2)?;
            Head {
                title: format!("{name} block from now"),
                description: format!(
                    "The {} block from now, as {on}'s mempool would fill it: its transactions and fee rates, live.",
                    name.to_lowercase()
                ),
            }
        }
    })
}

/// `/lightning/node/:key`. The alias is the node's own claim, read from the graph in memory.
fn node(ctx: &NetworkContext, on: &str, key: &str) -> Option<Head> {
    let id = lightning::nodes::pubkey(key).ok()?;
    let hex: String = id.iter().fold(String::with_capacity(66), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    let index = ctx.lightning.as_ref().and_then(|l| l.index());
    let Some(n) = index.as_deref().and_then(|i| i.node(&id)) else {
        return Some(Head {
            title: "Lightning node".into(),
            description: format!("Lightning node {hex} on {on}: its channels, capacity and fees."),
        });
    };
    let name = n
        .alias()
        .and_then(plain_alias)
        .unwrap_or_else(|| hex.get(..20).unwrap_or_default());
    Some(Head {
        title: format!("Lightning node {name}"),
        description: format!(
            "Lightning node {hex} on {on}: {} and {} BTC of capacity.",
            count(u32::try_from(n.open()).unwrap_or(u32::MAX), "open channel"),
            btc(n.capacity)
        ),
    })
}

/// `/lightning/rankings?by=`, as `parseRanking` reads it: the first `by`, capacity otherwise.
fn rankings(on: &str, query: Option<&str>) -> Head {
    let by = query
        .unwrap_or_default()
        .split('&')
        .find_map(|p| p.strip_prefix("by="));
    let (words, which) = match by {
        Some("channels") => ("by open channels", "with the most open channels"),
        Some("age") => ("oldest first", "that have been around longest"),
        _ => ("by capacity", "with the most capacity"),
    };
    Head {
        title: format!("Lightning nodes {words}"),
        description: format!("The 100 Lightning nodes on {on} {which}."),
    }
}

/// The alias as the node page shows it in its title, or `None` where the frontend's
/// `cleanAlias` might change it (a control, format or stacked combining character) or leave no
/// name: then the title is the key's first 20 digits, as the page's own is for a nameless node.
/// Only characters known to be none of those pass. Gossip limits an alias to 32 bytes, under the
/// page's 64-character cut.
fn plain_alias(alias: &str) -> Option<&str> {
    let mut marks = 0;
    for c in alias.chars() {
        // Variation selector 16 and the keycap mark: emoji use them; three in a row get thinned.
        if matches!(c, '\u{fe0f}' | '\u{20e3}') {
            marks += 1;
            if marks > 2 {
                return None;
            }
            continue;
        }
        marks = 0;
        let ok = matches!(c,
            ' '..='~'
            | '\u{a0}'..='\u{ac}' | '\u{ae}'..='\u{24f}' // Latin, without the soft hyphen
            | '\u{370}'..='\u{482}' | '\u{48a}'..='\u{52f}' // Greek and Cyrillic letters
            | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205e}' // punctuation
            | '\u{20a0}'..='\u{20c0}' | '\u{2100}'..='\u{23ff}' | '\u{2460}'..='\u{27bf}' // symbols
            | '\u{3041}'..='\u{3096}' | '\u{30a0}'..='\u{30ff}' // kana
            | '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7a3}' // CJK and Hangul
            | '\u{200d}' // the joiner emoji are built with, which `cleanAlias` keeps
            | '\u{1f000}'..='\u{1faff}'); // emoji
        if !ok {
            return None;
        }
    }
    (!alias.trim().is_empty()).then_some(alias)
}

/// Shortens a hash or address in the middle, as `truncateMiddle` does: 8 first, `…`, 8 last.
fn short(s: &str) -> String {
    // Only parsed hashes and addresses come here: ASCII, so byte offsets are characters.
    match (s.get(..8), s.get(s.len().saturating_sub(8)..)) {
        (Some(head), Some(tail)) if s.len() > 17 => format!("{head}…{tail}"),
        _ => s.to_owned(),
    }
}

/// Digits in threes, as `groupDigits`.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(NNBSP);
        }
        out.push(c);
    }
    out
}

fn count(n: u32, what: &str) -> String {
    format!(
        "{} {what}{}",
        group(u64::from(n)),
        if n == 1 { "" } else { "s" }
    )
}

/// Sats as BTC with eight decimals.
fn btc(sats: u64) -> String {
    format!("{}.{:08}", group(sats / 100_000_000), sats % 100_000_000)
}

/// Text for an HTML text node or a double-quoted attribute.
fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// The title element and the preview tags that replace the build's `<title>`.
fn render(network: Network, head: &Head) -> String {
    let mut title = String::from(title_prefix(network));
    escape(&head.title, &mut title);
    let mut description = String::new();
    escape(&head.description, &mut description);
    format!(
        "<title>{title} | {SITE}</title>\n    \
         <meta name=\"description\" content=\"{description}\" />\n    \
         <meta property=\"og:title\" content=\"{title}\" />\n    \
         <meta property=\"og:description\" content=\"{description}\" />\n    \
         <meta property=\"og:type\" content=\"website\" />\n    \
         <meta property=\"og:site_name\" content=\"{SITE}\" />\n    \
         <meta name=\"twitter:card\" content=\"summary\" />"
    )
}

/// Every page path: the shell, its head filled in for the page.
pub(crate) async fn page(State(pages): State<Arc<Pages>>, uri: Uri) -> Response {
    let Some(shell) = &pages.shell else {
        return ApiError::NOT_FOUND.into_response();
    };
    let html = match (shell.title, pages.head(uri.path(), uri.query()).await) {
        (Some((start, end)), Some((network, head))) => {
            let (before, after) = (
                shell.raw.get(..start).unwrap_or_default(),
                shell.raw.get(end..).unwrap_or_default(),
            );
            format!("{before}{}{after}", render(network, &head))
        }
        _ => shell.raw.clone(),
    };
    (
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        Body::from(html),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_and_quotes_are_escaped() {
        let mut out = String::new();
        escape("<a href=\"x\" onclick='y'>&amp;</a>", &mut out);
        assert_eq!(
            out,
            "&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn numbers_are_written_as_the_frontend_writes_them() {
        assert_eq!(group(0), "0");
        assert_eq!(group(975), "975");
        assert_eq!(group(975_411), "975\u{202f}411");
        assert_eq!(group(1_234_567), "1\u{202f}234\u{202f}567");
        assert_eq!(btc(12_345), "0.00012345");
        assert_eq!(
            btc(2_100_000_000_000_000),
            "21\u{202f}000\u{202f}000.00000000"
        );
        assert_eq!(count(1, "transaction"), "1 transaction");
        assert_eq!(count(4198, "transaction"), "4\u{202f}198 transactions");
        assert_eq!(short("a3f9c2e1aaaaaaaaaaaaaa7b0d44e9"), "a3f9c2e1…7b0d44e9");
        assert_eq!(short("bcrt1pfeesnyr2tx"), "bcrt1pfeesnyr2tx");
    }

    #[test]
    fn heights_are_what_the_frontend_takes() {
        assert_eq!(height("0"), Ok(0));
        assert_eq!(height("0975411"), Ok(975_411));
        assert_eq!(height("4294967295"), Ok(u32::MAX));
        for bad in [
            "",
            "4294967296",
            "+5",
            "-1",
            "1e3",
            "12345678901",
            " 1",
            "１",
        ] {
            assert_eq!(height(bad), Err(()), "{bad}");
        }
    }

    #[test]
    fn only_aliases_the_frontend_shows_unchanged_are_used() {
        for ok in [
            "ACINQ",
            "⚡️ Bolt ⚡️",
            "Kraken 🐙",
            "café",
            "Ünïcødé",
            "東京",
            "👨‍👩‍👧",
            "1️⃣",
        ] {
            assert_eq!(plain_alias(ok), Some(ok), "{ok:?}");
        }
        for changed in [
            "a\u{202e}b",             // a bidi override
            "a\u{200b}b",             // a zero-width space
            "a\u{0}b",                // a control character
            "e\u{301}\u{301}\u{301}", // stacked marks
            "x\u{fe0f}\u{fe0f}\u{fe0f}",
            "\u{ad}",
            "   ",
            "",
        ] {
            assert_eq!(plain_alias(changed), None, "{changed:?}");
        }
    }

    #[test]
    fn paths_split_into_network_and_page() {
        assert_eq!(split("/"), (Network::Mainnet, "/"));
        assert_eq!(split("/signet"), (Network::Signet, ""));
        assert_eq!(split("/signet/tx/a"), (Network::Signet, "/tx/a"));
        assert_eq!(split("/signetx/tx"), (Network::Mainnet, "/signetx/tx"));
        assert_eq!(split("/regtest/"), (Network::Regtest, "/"));
    }

    #[test]
    fn the_shell_is_split_around_its_title() {
        let s = Shell::new("<head><title>rbitcoin.space</title></head>".into());
        assert_eq!(s.title, Some((6, 35)));
        assert_eq!(Shell::new("<!doctype html>app-shell".into()).title, None);
        assert_eq!(Shell::new("<title>no end".into()).title, None);
    }
}
