//! The record of recent replacements: what replaced what, when we saw it, and the fee figures of
//! each as far as the mempool tracker knew them. In memory, bounded by `REPLACEMENTS_KEPT`, oldest
//! out first, and lost on restart. Fed by [`crate::ws::track`] from every tracker delta; read by
//! the `tx:<txid>` topic and `GET /app/tx/:txid/replacements` (`docs/FRONTEND.md`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::response::{IntoResponse, Response};
use axum::{Json, http::StatusCode};
use bitcoin::Txid;
use serde_json::{Value, json};

use crate::{ApiError, NetworkContext, esplora};

/// Most links one answer or message carries, in each direction. A longer history is cut and
/// marked `truncated`.
pub const MAX_LINKS: usize = 32;

/// One replacement: `txid` left the mempool because `by` spent one of its inputs, or one of its
/// ancestors' (a child leaves with its replaced parent and carries the same replacement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub txid: Txid,
    pub by: Txid,
    /// Unix seconds when the tracker saw it.
    pub seen: u64,
    /// The replaced transaction's own fee and sigop-adjusted size.
    pub fee: u64,
    pub vsize: u32,
    /// The replacement's, when the tracker held it: `None` when it was confirmed in the same
    /// block that removed the original, or left before we could read it.
    pub by_fee: Option<u64>,
    pub by_vsize: Option<u32>,
}

/// Sat per 1,000 vB, rounded down, like every rate the server sends.
fn rate(fee: u64, vsize: u32) -> Option<u64> {
    (vsize > 0).then(|| fee.saturating_mul(1000) / u64::from(vsize))
}

impl Link {
    pub fn json(&self) -> Value {
        json!({
            "txid": self.txid,
            "by": self.by,
            "seen": self.seen,
            "fee": self.fee,
            "vsize": self.vsize,
            "rate": rate(self.fee, self.vsize),
            "by_fee": self.by_fee,
            "by_vsize": self.by_vsize,
            "by_rate": self.by_fee.zip(self.by_vsize).and_then(|(f, v)| rate(f, v)),
        })
    }
}

/// The bounded record.
#[derive(Debug)]
pub struct Replacements {
    cap: usize,
    /// By replaced txid, with the insertion number that `order` refers to.
    links: HashMap<Txid, (u64, Link)>,
    /// Insertion order, oldest first. Holds at most `cap` entries, some of them for links since
    /// overwritten (a transaction that came back and was replaced again).
    order: VecDeque<(u64, Txid)>,
    /// Replaced txids by their replacement, to walk a history backwards.
    replaced: HashMap<Txid, Vec<Txid>>,
    next: u64,
    /// Links recorded since the counter was last taken, for the log.
    recorded: u64,
}

impl Replacements {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            links: HashMap::new(),
            order: VecDeque::new(),
            replaced: HashMap::new(),
            next: 0,
            recorded: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Links recorded since the last call.
    pub fn take_recorded(&mut self) -> u64 {
        std::mem::take(&mut self.recorded)
    }

    pub fn get(&self, txid: &Txid) -> Option<&Link> {
        self.links.get(txid).map(|(_, l)| l)
    }

    pub fn insert(&mut self, link: Link) {
        self.unlink(&link.txid);
        self.next += 1;
        self.links.insert(link.txid, (self.next, link));
        self.order.push_back((self.next, link.txid));
        self.replaced.entry(link.by).or_default().push(link.txid);
        self.recorded += 1;
        while self.order.len() > self.cap {
            if let Some((n, t)) = self.order.pop_front()
                && self.links.get(&t).is_some_and(|(m, _)| *m == n)
            {
                self.unlink(&t);
            }
        }
    }

    fn unlink(&mut self, txid: &Txid) {
        let Some((_, old)) = self.links.remove(txid) else {
            return;
        };
        if let Some(list) = self.replaced.get_mut(&old.by) {
            list.retain(|t| t != txid);
            if list.is_empty() {
                self.replaced.remove(&old.by);
            }
        }
    }

    /// What replaced `txid`, then what replaced that, to the newest we know: at most
    /// [`MAX_LINKS`], and `true` if there were more.
    pub fn chain(&self, txid: &Txid) -> (Vec<Link>, bool) {
        let mut out: Vec<Link> = Vec::new();
        let mut at = *txid;
        while let Some(l) = self.get(&at) {
            // A replacement that came back and was itself replaced by an earlier one: stop.
            if out.iter().any(|o| o.txid == l.txid) {
                break;
            }
            if out.len() == MAX_LINKS {
                return (out, true);
            }
            out.push(*l);
            at = l.by;
        }
        (out, false)
    }

    /// What `txid` replaced, and what those replaced, newest first: at most [`MAX_LINKS`], and
    /// `true` if there were more.
    pub fn earlier(&self, txid: &Txid) -> (Vec<Link>, bool) {
        let mut out = Vec::new();
        let mut seen = HashSet::from([*txid]);
        let mut queue = VecDeque::from([*txid]);
        while let Some(by) = queue.pop_front() {
            for t in self.replaced.get(&by).into_iter().flatten() {
                if !seen.insert(*t) {
                    continue;
                }
                if out.len() == MAX_LINKS {
                    return (out, true);
                }
                if let Some(l) = self.get(t) {
                    out.push(*l);
                }
                queue.push_back(*t);
            }
        }
        (out, false)
    }
}

/// `GET /app/tx/:txid/replacements`: what this transaction replaced and what replaced it, as far
/// as the record goes back.
pub async fn route(
    State(ctx): State<Arc<NetworkContext>>,
    Path(txid): Path<String>,
) -> Result<Response, ApiError> {
    let txid = esplora::txid(&txid)?;
    if !ctx.track.mempool_enabled() {
        return Err(crate::ws::track::MEMPOOL_OFF);
    }
    let body = ctx.track.with_replacements(|r| {
        let (later, more_later) = r.chain(&txid);
        let (earlier, more_earlier) = r.earlier(&txid);
        json!({
            "txid": txid,
            "replaced_by": later.iter().map(Link::json).collect::<Vec<_>>(),
            "replaces": earlier.iter().map(Link::json).collect::<Vec<_>>(),
            "truncated": more_later || more_earlier,
        })
    });
    let mut res = (StatusCode::OK, Json(body)).into_response();
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use bitcoin::hashes::Hash;

    use super::*;

    fn id(n: u8) -> Txid {
        Txid::from_byte_array([n; 32])
    }

    fn link(a: u8, b: u8) -> Link {
        Link {
            txid: id(a),
            by: id(b),
            seen: u64::from(a),
            fee: 100,
            vsize: 200,
            by_fee: Some(300),
            by_vsize: None,
        }
    }

    fn txids(links: &[Link]) -> Vec<Txid> {
        links.iter().map(|l| l.txid).collect()
    }

    #[test]
    fn chains_run_both_ways_and_stop_at_a_loop() {
        let mut r = Replacements::new(100);
        for (a, b) in [(1, 2), (2, 3), (3, 4), (9, 2)] {
            r.insert(link(a, b));
        }
        assert_eq!(txids(&r.chain(&id(1)).0), vec![id(1), id(2), id(3)]);
        assert_eq!(r.chain(&id(4)), (Vec::new(), false));
        let (back, more) = r.earlier(&id(4));
        assert!(!more);
        assert_eq!(back.len(), 4);
        assert_eq!(back[0].txid, id(3));
        // 4 came back and was replaced by 1: the chain from 1 ends where it would loop.
        r.insert(link(4, 1));
        assert_eq!(txids(&r.chain(&id(1)).0), vec![id(1), id(2), id(3), id(4)]);
        assert_eq!(
            link(1, 2).json()["rate"],
            json!(500),
            "100 sat over 200 vB is 0.5 sat/vB"
        );
        assert_eq!(link(1, 2).json()["by_rate"], Value::Null);
    }

    #[test]
    fn long_chains_are_cut_and_marked() {
        let mut r = Replacements::new(1000);
        for i in 0..100 {
            r.insert(link(i, i + 1));
        }
        let (chain, more) = r.chain(&id(0));
        assert_eq!((chain.len(), more), (MAX_LINKS, true));
        let (back, more) = r.earlier(&id(100));
        assert_eq!((back.len(), more), (MAX_LINKS, true));
    }

    #[test]
    fn the_oldest_fall_out_first_and_a_rewrite_counts_once() {
        let mut r = Replacements::new(3);
        for (a, b) in [(1, 2), (3, 4), (5, 6)] {
            r.insert(link(a, b));
        }
        r.insert(link(7, 8));
        assert_eq!((r.len(), r.get(&id(1))), (3, None));
        assert!(
            r.earlier(&id(2)).0.is_empty(),
            "the reverse index forgets it too"
        );
        // 3 is replaced again by 9: its old entry in the order no longer evicts the new one.
        r.insert(link(3, 9));
        r.insert(link(10, 11));
        assert_eq!(r.get(&id(3)).map(|l| l.by), Some(id(9)));
        assert!(r.earlier(&id(4)).0.is_empty());
        assert_eq!(r.len(), 3);
        // Many rewrites of one link never grow the record past its cap.
        for _ in 0..50 {
            r.insert(link(3, 9));
        }
        assert!(r.order.len() <= 3 && r.len() <= 3);
        assert_eq!(r.take_recorded(), 56);
    }
}
