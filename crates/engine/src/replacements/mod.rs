//! The record of recent replacements: what replaced what, when we saw it, and each side's own
//! figures as the mempool tracker held them. Bounded, oldest out first. The server keeps it in
//! memory, fed from the tracker's change stream, and with a store also in the `replacements`
//! table ([`db`]), so it survives a restart (`docs/MEMPOOL.md`, "The replacement record").
//!
//! Read as chains (what replaced this, and what replaced that) by the `tx:` topic and `/app`, and
//! as trees (a replacement and everything it replaced, recursively) by `/api/v1`.

pub mod db;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};

use bitcoin::Txid;
use serde_json::{Value, json};

use crate::mempool::TxInfo;

/// Most links one chain answer or message carries, in each direction. A longer history is cut
/// and marked `truncated`.
pub const MAX_LINKS: usize = 32;
/// Most transactions one tree holds. A replacement removes at most 100 transactions with their
/// descendants on rbitcoin, so a single step fits; a longer history is cut.
pub const MAX_TREE: usize = 100;

/// One transaction's own figures, as the tracker held them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    /// Base fee, sats.
    pub fee: u64,
    /// Sigop-adjusted virtual size.
    pub vsize: u32,
    pub weight: u32,
    /// Total output value, sats.
    pub value: u64,
    /// When the node accepted it, Unix seconds.
    pub time: u32,
    /// It signalled replaceability itself (BIP 125).
    pub rbf: bool,
}

impl Facts {
    pub fn of(info: &TxInfo) -> Self {
        Self {
            fee: info.fee.to_sat(),
            vsize: info.vsize,
            weight: info.weight,
            value: info.value.to_sat(),
            time: info.time,
            rbf: info.rbf,
        }
    }
}

/// One replacement: `txid` left the mempool because `by` spent one of its inputs, or one of its
/// ancestors' (a child leaves with its replaced parent and carries the same replacement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub txid: Txid,
    pub by: Txid,
    /// Unix seconds when the tracker saw it.
    pub seen: u64,
    /// The replaced transaction's.
    pub tx: Facts,
    /// The replacement's, when the tracker held it: `None` when it was confirmed in the same
    /// block that removed the original, or left before we could read it.
    pub by_tx: Option<Facts>,
    /// The replacement was later confirmed (seen leaving the mempool in a block).
    pub mined: bool,
}

/// Sat per 1,000 vB, rounded down, like every rate the server sends.
fn rate(fee: u64, vsize: u32) -> Option<u64> {
    (vsize > 0).then(|| fee.saturating_mul(1000) / u64::from(vsize))
}

impl Link {
    /// The link as the `tx:` topic and `/app` send it.
    pub fn json(&self) -> Value {
        let by_fee = self.by_tx.map(|f| f.fee);
        let by_vsize = self.by_tx.map(|f| f.vsize);
        json!({
            "txid": self.txid,
            "by": self.by,
            "seen": self.seen,
            "fee": self.tx.fee,
            "vsize": self.tx.vsize,
            "rate": rate(self.tx.fee, self.tx.vsize),
            "by_fee": by_fee,
            "by_vsize": by_vsize,
            "by_rate": self.by_tx.and_then(|f| rate(f.fee, f.vsize)),
        })
    }
}

/// One transaction in a replacement tree, and what it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub txid: Txid,
    /// `None` only for a tree's newest replacement when the tracker never held it.
    pub tx: Option<Facts>,
    /// When it entered the mempool: its accept time, or for a replacement we never held, when we
    /// saw it replace.
    pub time: u64,
    /// Confirmed: only ever set on a tree's newest replacement.
    pub mined: bool,
    /// What it replaced, oldest replacement first.
    pub replaces: Vec<Node>,
}

impl Node {
    /// Whether anything below it was replaced without signalling (full RBF).
    pub fn full_rbf(&self) -> bool {
        self.replaces
            .iter()
            .any(|c| c.tx.is_some_and(|f| !f.rbf) || c.full_rbf())
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
    /// Counts every change, so a reader can tell whether an answer built earlier still holds.
    version: u64,
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
            version: 0,
        }
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Links recorded since the last call.
    pub fn take_recorded(&mut self) -> u64 {
        std::mem::take(&mut self.recorded)
    }

    pub fn get(&self, txid: &Txid) -> Option<&Link> {
        self.links.get(txid).map(|(_, l)| l)
    }

    /// Records a replacement seen now.
    pub fn insert(&mut self, link: Link) {
        self.restore(link);
        self.recorded += 1;
    }

    /// Puts back a link recorded before (from the store), oldest first, without counting it as
    /// new.
    pub fn restore(&mut self, link: Link) {
        self.unlink(&link.txid);
        // `by` is back (in the mempool or a block), so what replaced it before no longer has:
        // keeping both would make a loop with no newest end.
        self.unlink(&link.by);
        self.next += 1;
        self.version += 1;
        self.links.insert(link.txid, (self.next, link));
        self.order.push_back((self.next, link.txid));
        self.replaced.entry(link.by).or_default().push(link.txid);
        while self.order.len() > self.cap {
            if let Some((n, t)) = self.order.pop_front()
                && self.links.get(&t).is_some_and(|(m, _)| *m == n)
            {
                self.unlink(&t);
            }
        }
    }

    /// `txid` was confirmed: if it replaced anything, its tree is now mined. Returns whether it
    /// replaced anything we hold.
    pub fn confirmed(&mut self, txid: &Txid) -> bool {
        let Some(list) = self.replaced.get(txid) else {
            return false;
        };
        for t in list {
            if let Some((_, l)) = self.links.get_mut(t) {
                l.mined = true;
            }
        }
        self.version += 1;
        true
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

    /// What `txid` itself replaced, oldest first.
    pub fn replaced_by(&self, txid: &Txid) -> &[Txid] {
        self.replaced.get(txid).map_or(&[], Vec::as_slice)
    }

    /// `txid`'s own figures, if the record holds them: as a replaced transaction, or as the
    /// replacement of one.
    pub fn facts(&self, txid: &Txid) -> Option<Facts> {
        self.get(txid).map(|l| l.tx).or_else(|| {
            self.replaced_by(txid)
                .iter()
                .find_map(|t| self.get(t).and_then(|l| l.by_tx))
        })
    }

    /// The newest replacement in `txid`'s history: `txid` itself if nothing replaced it.
    /// The walk is as long as the history, which the record's bound limits.
    pub fn root(&self, txid: &Txid) -> Txid {
        self.root_with(txid, &mut HashMap::new())
    }

    /// [`Self::root`], remembering every transaction walked through in `memo`, so walking every
    /// link of the record costs one step per link.
    fn root_with(&self, txid: &Txid, memo: &mut HashMap<Txid, Txid>) -> Txid {
        let mut path = Vec::new();
        let mut walked = HashSet::new();
        let mut at = *txid;
        let root = loop {
            if let Some(r) = memo.get(&at) {
                break *r;
            }
            // `restore` leaves no loop; this guards the walk all the same.
            if !walked.insert(at) {
                break at;
            }
            match self.get(&at) {
                Some(l) => {
                    path.push(at);
                    at = l.by;
                }
                None => break at,
            }
        };
        for t in path {
            memo.insert(t, root);
        }
        root
    }

    /// `txid` is in the mempool again (rebroadcast, or back from a block a reorg undid): it no
    /// longer stands replaced, and what it replaced is no longer mined. Returns whether that
    /// changed the record.
    pub fn returned(&mut self, txid: &Txid) -> bool {
        let mut changed = self.links.contains_key(txid);
        self.unlink(txid);
        if let Some(list) = self.replaced.get(txid) {
            for t in list {
                if let Some((_, l)) = self.links.get_mut(t)
                    && l.mined
                {
                    l.mined = false;
                    changed = true;
                }
            }
        }
        if changed {
            self.version += 1;
        }
        changed
    }

    /// The tree of `txid`'s history: its newest replacement and everything that replaced,
    /// recursively, up to [`MAX_TREE`] transactions. `None` if `txid` neither replaced nor was
    /// replaced.
    pub fn tree(&self, txid: &Txid) -> Option<Node> {
        let root = self.root(txid);
        let first = self.replaced_by(&root);
        if first.is_empty() {
            return None;
        }
        let links: Vec<&Link> = first.iter().filter_map(|t| self.get(t)).collect();
        let tx = links.iter().find_map(|l| l.by_tx);
        let time = tx.map_or_else(
            || links.iter().map(|l| l.seen).min().unwrap_or(0),
            |f| u64::from(f.time),
        );
        let mut node = Node {
            txid: root,
            tx,
            time,
            mined: links.iter().any(|l| l.mined),
            replaces: Vec::new(),
        };
        let mut seen = HashSet::from([root]);
        self.grow(&mut node, &mut seen);
        Some(node)
    }

    /// Adds what `node` replaced, depth first, until the tree holds [`MAX_TREE`].
    fn grow(&self, node: &mut Node, seen: &mut HashSet<Txid>) {
        for t in self.replaced_by(&node.txid) {
            if seen.len() >= MAX_TREE {
                return;
            }
            let Some(l) = self.get(t) else { continue };
            if !seen.insert(*t) {
                continue;
            }
            let mut child = Node {
                txid: *t,
                tx: Some(l.tx),
                time: u64::from(l.tx.time),
                mined: false,
                replaces: Vec::new(),
            };
            self.grow(&mut child, seen);
            node.replaces.push(child);
        }
    }

    /// The `n` newest trees, newest replacement first; with `full_rbf`, only those where
    /// something was replaced without signalling.
    pub fn recent(&self, n: usize, full_rbf: bool) -> Vec<Node> {
        let mut out = Vec::new();
        let mut roots = HashSet::new();
        let mut memo = HashMap::new();
        for (i, t) in self.order.iter().rev() {
            if out.len() >= n {
                break;
            }
            if self.links.get(t).is_none_or(|(m, _)| m != i) {
                continue;
            }
            let root = self.root_with(t, &mut memo);
            if !roots.insert(root) {
                continue;
            }
            if let Some(tree) = self.tree(&root)
                && (!full_rbf || tree.full_rbf())
            {
                out.push(tree);
            }
        }
        out
    }
}
