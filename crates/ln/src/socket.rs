//! LDK's peer manager over a tokio socket, read at the pace each peer's budget allows.
//!
//! The peer manager reads and writes through [`Socket`]; [`run`] feeds it what arrives. After each
//! read, a peer whose gossip has spent its share of signature checks ([`Gossip::hold_off`]) is
//! not read again until it is back within it. Its bytes wait in the kernel and TCP slows the
//! sender: nothing it sent is dropped, and other peers are read as before, each on its own task.

use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lightning::ln::peer_handler::SocketDescriptor;
use tokio::net::TcpStream;
use tokio::sync::Notify;

use crate::PublicKey;
use crate::gossip::Gossip;
use crate::peer::Pm;

/// LDK's handle on one connection. Equal handles are the same connection.
#[derive(Clone)]
pub(crate) struct Socket(Arc<Conn>);

struct Conn {
    id: u64,
    stream: TcpStream,
    state: Mutex<State>,
    /// Wakes [`run`] when the state changed.
    wake: Notify,
}

#[derive(Default)]
struct State {
    /// LDK asked for no reads until its writes to the peer drain.
    paused: bool,
    /// A write did not fit; LDK waits for `write_buffer_space_avail`.
    blocked: bool,
    /// LDK disconnected the peer: it must not hear of this socket again.
    closed: bool,
    /// A write failed: the socket is gone, and LDK must be told.
    broken: bool,
}

impl Conn {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SocketDescriptor for Socket {
    fn send_data(&mut self, data: &[u8], continue_read: bool) -> usize {
        let mut s = self.0.state();
        if s.closed || s.broken {
            return 0;
        }
        s.paused = !continue_read;
        let mut sent = 0;
        while let Some(rest) = data.get(sent..).filter(|r| !r.is_empty()) {
            match self.0.stream.try_write(rest) {
                Ok(n) if n > 0 => sent += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    s.blocked = true;
                    break;
                }
                _ => {
                    s.broken = true;
                    break;
                }
            }
        }
        drop(s);
        self.0.wake.notify_one();
        sent
    }

    fn disconnect_socket(&mut self) {
        self.0.state().closed = true;
        self.0.wake.notify_one();
    }
}

impl PartialEq for Socket {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for Socket {}

impl Hash for Socket {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0.id.hash(h);
    }
}

/// Runs an outbound connection to `node_id` until it ends, whether or not the handshake
/// completed.
pub(crate) async fn run(pm: &Pm, gossip: &Gossip, node_id: PublicKey, stream: TcpStream) {
    static IDS: AtomicU64 = AtomicU64::new(0);
    let address = stream.peer_addr().ok().map(Into::into);
    let conn = Arc::new(Conn {
        id: IDS.fetch_add(1, Ordering::Relaxed),
        stream,
        state: Mutex::new(State::default()),
        wake: Notify::new(),
    });
    let mut socket = Socket(Arc::clone(&conn));
    let Ok(first) = pm.new_outbound_connection(node_id, socket.clone(), address) else {
        return;
    };
    // The handshake's 50 bytes always fit in a new socket's buffer.
    if socket.send_data(&first, true) < first.len() {
        pm.socket_disconnected(&socket);
        return;
    }
    // As LDK asks: about 4 KiB of messages handled at a time.
    let mut buf = [0u8; 4096];
    loop {
        let (paused, blocked) = {
            let s = conn.state();
            if s.closed {
                return;
            }
            if s.broken {
                break;
            }
            (s.paused, s.blocked)
        };
        tokio::select! {
            ready = conn.stream.readable(), if !paused => {
                if ready.is_err() {
                    break;
                }
                match conn.stream.try_read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let Some(read) = buf.get(..n) else { break };
                        if pm.read_event(&mut socket, read).is_err() {
                            // LDK forgets the peer itself; it must not hear of the socket again.
                            return;
                        }
                        pm.process_events();
                        let wait = gossip.hold_off(&node_id);
                        if !wait.is_zero() {
                            tokio::time::sleep(wait).await;
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
            }
            ready = conn.stream.writable(), if blocked => {
                conn.state().blocked = false;
                if ready.is_err() || pm.write_buffer_space_avail(&mut socket).is_err() {
                    break;
                }
            }
            () = conn.wake.notified() => {}
        }
    }
    pm.socket_disconnected(&socket);
    pm.process_events();
}
