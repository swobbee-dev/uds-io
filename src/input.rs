//! `DatagramInputPin`: a `UnixDatagram`-backed input pin implementing
//! `embedded_hal::digital::InputPin` and `embedded_hal_async::digital::Wait`.

use std::convert::Infallible;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::Path;

use async_io::Async;
use embedded_hal::digital::{ErrorType, InputPin};
use embedded_hal_async::digital::Wait;

use crate::byte_to_bool;

/// An input pin whose state is driven by:
/// - Datagrams received on a bound Unix socket, and
/// - Direct state injection via an [`InputPinInjector`] handle.
///
/// Both paths funnel into the same internal state. The pin caches the most
/// recent state and exposes it via `is_high` / `is_low`; awaiting any of the
/// `Wait` futures resumes when new state arrives.
///
/// The pin itself owns the socket; no background task is spawned. The pin's
/// futures must be polled by some executor for it to make progress.
///
/// Dropping every outstanding [`InputPinInjector`] is fine — the channel
/// stays open for the pin's lifetime.
pub struct DatagramInputPin {
    socket: Option<Async<UnixDatagram>>,
    override_rx: async_channel::Receiver<bool>,
    /// Keeps `override_rx` open for the pin's lifetime.
    _override_keepalive: async_channel::Sender<bool>,
    state: bool,
}

/// Handle for directly injecting a state value into a [`DatagramInputPin`],
/// bypassing the socket path. Useful for tests and for in-process fault
/// injection (e.g. from a gRPC service).
#[derive(Clone)]
pub struct InputPinInjector {
    override_tx: async_channel::Sender<bool>,
}

impl DatagramInputPin {
    /// Bind a Unix datagram socket at `path` and return a pin reading from it
    /// plus a cloneable injector handle.
    ///
    /// A stale socket file from a crashed prior run (file present, no live
    /// owner) is reclaimed; a live peer already bound to the path is *not*
    /// displaced — the call fails with `AddrInUse` rather than ripping the
    /// socket out from under it. See [`bind_or_reclaim`] for the probe logic.
    /// Parent directories are created if missing.
    pub fn bind(path: impl AsRef<Path>, initial: bool) -> io::Result<(Self, InputPinInjector)> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let socket = bind_or_reclaim(path)?;
        let (tx, rx) = async_channel::unbounded();
        Ok((
            Self {
                socket: Some(socket),
                override_rx: rx,
                _override_keepalive: tx.clone(),
                state: initial,
            },
            InputPinInjector { override_tx: tx },
        ))
    }

    /// Construct a pin with no socket: state is driven only by the injector.
    /// Useful for unit tests that don't need a real socket peer.
    pub fn unbound(initial: bool) -> (Self, InputPinInjector) {
        let (tx, rx) = async_channel::unbounded();
        (
            Self {
                socket: None,
                override_rx: rx,
                _override_keepalive: tx.clone(),
                state: initial,
            },
            InputPinInjector { override_tx: tx },
        )
    }

    /// Drain any queued datagrams and injected overrides into `self.state`.
    /// Non-blocking — returns once both sources would block. Bytes that
    /// don't match the strict wire format are logged at trace and skipped.
    fn drain_pending(&mut self) {
        if let Some(sock) = &self.socket {
            let mut buf = [0u8; 1];
            loop {
                match sock.get_ref().recv(&mut buf) {
                    Ok(1) => match byte_to_bool(buf[0]) {
                        Some(v) => self.state = v,
                        None => {
                            tracing::trace!(byte = buf[0], "uds-io: ignoring unknown wire byte")
                        }
                    },
                    Ok(_) => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
        while let Ok(v) = self.override_rx.try_recv() {
            self.state = v;
        }
    }

    /// Wait for either the socket or the override channel to yield one value
    /// and apply it to `self.state`. Bytes that don't match the strict wire
    /// format do not update state; a Wait future awaiting an edge will simply
    /// loop and re-await on the next datagram.
    async fn recv_one(&mut self) {
        let new_state: Option<bool> = {
            let socket = self.socket.as_ref();
            let override_rx = &self.override_rx;
            match socket {
                Some(sock) => {
                    let socket_fut = async {
                        let mut buf = [0u8; 1];
                        let n = sock.read_with(|s| s.recv(&mut buf)).await.ok()?;
                        if n != 1 {
                            return None;
                        }
                        let parsed = byte_to_bool(buf[0]);
                        if parsed.is_none() {
                            tracing::trace!(byte = buf[0], "uds-io: ignoring unknown wire byte");
                        }
                        parsed
                    };
                    let override_fut = async { override_rx.recv().await.ok() };
                    futures_lite::future::or(socket_fut, override_fut).await
                }
                None => override_rx.recv().await.ok(),
            }
        };
        if let Some(v) = new_state {
            self.state = v;
        }
    }
}

impl InputPinInjector {
    /// Inject a state value. Non-blocking; uses an unbounded channel so this
    /// never fails under normal conditions.
    pub fn inject(&self, value: bool) {
        if let Err(e) = self.override_tx.try_send(value) {
            tracing::warn!(?e, "uds-io: injector try_send failed");
        }
    }
}

impl ErrorType for DatagramInputPin {
    type Error = Infallible;
}

impl InputPin for DatagramInputPin {
    fn is_high(&mut self) -> Result<bool, Self::Error> {
        self.drain_pending();
        Ok(self.state)
    }

    fn is_low(&mut self) -> Result<bool, Self::Error> {
        self.drain_pending();
        Ok(!self.state)
    }
}

impl Wait for DatagramInputPin {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        loop {
            self.drain_pending();
            if self.state {
                return Ok(());
            }
            self.recv_one().await;
        }
    }

    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        loop {
            self.drain_pending();
            if !self.state {
                return Ok(());
            }
            self.recv_one().await;
        }
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        // Don't drain up front: doing so would collapse a queued [low, high]
        // pair into a single "state is high" reading and we'd miss the edge.
        // Instead, walk events one-at-a-time looking for the false->true
        // transition.
        let mut prev = self.state;
        loop {
            self.recv_one().await;
            if !prev && self.state {
                return Ok(());
            }
            prev = self.state;
        }
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        let mut prev = self.state;
        loop {
            self.recv_one().await;
            if prev && !self.state {
                return Ok(());
            }
            prev = self.state;
        }
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        let prev = self.state;
        loop {
            self.recv_one().await;
            if self.state != prev {
                return Ok(());
            }
        }
    }
}

/// Binds a UNIX datagram socket at `path`, reclaiming a stale file from a
/// crashed prior run while refusing to clobber a live peer.
///
/// On `EADDRINUSE`, probes the path via `connect()`: a `SOCK_DGRAM` UNIX
/// socket returns `ECONNREFUSED` when the file exists but no process is
/// bound (the crash case) and succeeds when a process is bound. The probe
/// is the only way to distinguish the two — `path.exists()` alone tells us
/// nothing about liveness, and an unconditional `remove_file` would
/// silently steal a concurrent process's socket. The bind retry after
/// `remove_file` is racy against another process binding in between, but
/// losing that race surfaces as a clean `AddrInUse` rather than data
/// corruption.
fn bind_or_reclaim(path: &Path) -> io::Result<Async<UnixDatagram>> {
    match Async::<UnixDatagram>::bind(path) {
        Ok(s) => return Ok(s),
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {}
        Err(e) => return Err(e),
    }

    let probe = UnixDatagram::unbound()?;
    match probe.connect(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "UDS path {} is bound by another live process",
                path.display()
            ),
        )),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(path)?;
            Async::<UnixDatagram>::bind(path)
        }
        Err(e) => Err(e),
    }
}
