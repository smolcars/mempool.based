//! The two files the peer keeps in its data directory: the identity seed and the graph.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use lightning::bitcoin::Network;
use lightning::bitcoin::constants::ChainHash;
use lightning::util::ser::ReadableArgs;

use crate::gossip::Graph;
use crate::log::LogSink;

pub(crate) const KEY_FILE: &str = "node_key";
pub(crate) const GRAPH_FILE: &str = "graph";

/// What [`crate::Peer::start`] found on disk.
#[derive(Clone, Debug)]
pub enum GraphLoad {
    /// No graph file: a first run, or the file was removed.
    Fresh,
    Loaded {
        nodes: usize,
        channels: usize,
        bytes: u64,
        duration: Duration,
        /// Time since the peer that saved it was last connected and caught up: the file's
        /// modification time is set to that moment, not to when it was written.
        age: Duration,
    },
    /// The file exists but could not be read or decoded. The peer starts with an empty graph and
    /// overwrites the file at its next save. `error` is a fixed label or an I/O error.
    Unreadable { error: String },
}

/// Reads the 32-byte identity seed, or creates it with owner-only permissions.
///
/// An existing seed readable by group or others is refused rather than used.
pub(crate) fn load_or_create_key(dir: &Path) -> io::Result<[u8; 32]> {
    let path = dir.join(KEY_FILE);
    let mut seed = [0u8; 32];
    match File::open(&path) {
        Ok(mut f) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if f.metadata()?.permissions().mode() & 0o077 != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "node key is readable by others: chmod 600 it",
                    ));
                }
            }
            f.read_exact(&mut seed)?;
            if f.read(&mut [0u8; 1])? != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "node key is not 32 bytes",
                ));
            }
            Ok(seed)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            File::open("/dev/urandom")?.read_exact(&mut seed)?;
            write_atomic(&path, |w| w.write_all(&seed))?;
            Ok(seed)
        }
        Err(e) => Err(e),
    }
}

/// Writes `path` through a temporary file in the same directory, synced and renamed over it, so a
/// crash at any point leaves either the old file or the new one. Owner-only permissions.
pub(crate) fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp_name = path.file_name().unwrap_or_default().to_owned();
    tmp_name.push(".tmp");
    let tmp = dir.join(tmp_name);
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut f = options.open(&tmp)?;
        write(&mut f)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        // The rename is durable only once the directory entry is.
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Bytes of graph file allowed per channel of the ceiling. A mainnet graph uses about 900; the
/// most a channel can bring is its announcement, two updates and two node announcements of
/// 1 KB each (`MAX_GOSSIP_BYTES`), plus LDK's own fields.
pub(crate) const MAX_FILE_BYTES_PER_CHANNEL: u64 = 8192;

pub(crate) fn load_graph(
    dir: &Path,
    network: Network,
    max_channels: usize,
    logger: &Arc<LogSink>,
) -> (Option<Graph>, GraphLoad) {
    let path = dir.join(GRAPH_FILE);
    let started = Instant::now();
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (None, GraphLoad::Fresh),
        Err(e) => return (None, unreadable(e.to_string())),
    };
    let meta = file.metadata().ok();
    let bytes = meta.as_ref().map_or(0, |m| m.len());
    let age = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .unwrap_or_default();
    if bytes > (max_channels as u64).saturating_mul(MAX_FILE_BYTES_PER_CHANNEL) {
        return (
            None,
            unreadable("larger than the ceiling allows".to_owned()),
        );
    }
    let mut buf = Vec::with_capacity(usize::try_from(bytes).unwrap_or(0));
    if let Err(e) = BufReader::new(file).read_to_end(&mut buf) {
        return (None, unreadable(e.to_string()));
    }
    // LDK's encoding: two version bytes, then the chain hash.
    let chain = ChainHash::using_genesis_block(network);
    if buf.get(2..34) != Some(chain.as_bytes().as_slice()) {
        return (None, unreadable("not this network's graph".to_owned()));
    }
    let graph = match Graph::read(&mut buf.as_slice(), Arc::clone(logger)) {
        Ok(g) => g,
        Err(e) => return (None, unreadable(format!("decode: {e:?}"))),
    };
    drop(buf);
    let ro = graph.read_only();
    let (nodes, channels) = (ro.nodes().len(), ro.channels().len());
    drop(ro);
    // Saved under a higher ceiling than the one configured now.
    if channels > max_channels {
        return (None, unreadable("over the ceiling".to_owned()));
    }
    let load = GraphLoad::Loaded {
        nodes,
        channels,
        bytes,
        duration: started.elapsed(),
        age,
    };
    (Some(graph), load)
}

fn unreadable(error: String) -> GraphLoad {
    GraphLoad::Unreadable { error }
}
