//! `DatagramInput<C>`: a `UnixDatagram`-backed input pin generic over a
//! [`Codec`]. The bool-codec specialization implements
//! `embedded_hal::digital::InputPin` and `embedded_hal_async::digital::Wait`;
//! the analog one exposes `value()` / `recv()`.

use std::convert::Infallible;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::Path;

use async_io::Async;
use embedded_hal::digital::{ErrorType, InputPin};
use embedded_hal_async::digital::Wait;

use crate::{BoolCodec, Codec, MAX_DATAGRAM};

/// An input pin whose state is driven by:
/// - Datagrams received on a bound Unix socket, and
/// - Direct value injection via an [`InputInjector`] handle.
///
/// Both paths funnel into the same internal state. The pin caches the most
/// recent value and exposes it via `value()` (and, for the bool codec,
/// `is_high` / `is_low`); awaiting `recv()` (or any `Wait` future) resumes when
/// new state arrives.
///
/// The pin itself owns the socket; no background task is spawned. The pin's
/// futures must be polled by some executor for it to make progress.
///
/// Dropping every outstanding [`InputInjector`] is fine — the channel stays
/// open for the pin's lifetime.
pub struct DatagramInput<C: Codec> {
    socket: Option<Async<UnixDatagram>>,
    override_rx: async_channel::Receiver<C::Value>,
    /// Keeps `override_rx` open for the pin's lifetime.
    _override_keepalive: async_channel::Sender<C::Value>,
    state: C::Value,
}

/// Handle for directly injecting a value into a [`DatagramInput`], bypassing the
/// socket path. Useful for tests and in-process fault injection (e.g. from a
/// gRPC service).
pub struct InputInjector<C: Codec> {
    override_tx: async_channel::Sender<C::Value>,
}

impl<C: Codec> Clone for InputInjector<C> {
    fn clone(&self) -> Self {
        Self {
            override_tx: self.override_tx.clone(),
        }
    }
}

impl<C: Codec> DatagramInput<C> {
    /// Bind a Unix datagram socket at `path` and return a pin reading from it
    /// plus a cloneable injector handle.
    ///
    /// A stale socket file from a crashed prior run (file present, no live
    /// owner) is reclaimed; a live peer already bound to the path is *not*
    /// displaced — the call fails with `AddrInUse` rather than ripping the
    /// socket out from under it. See [`bind_or_reclaim`] for the probe logic.
    /// Parent directories are created if missing.
    pub fn bind(path: impl AsRef<Path>, initial: C::Value) -> io::Result<(Self, InputInjector<C>)> {
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
            InputInjector { override_tx: tx },
        ))
    }

    /// Construct a pin with no socket: state is driven only by the injector.
    /// Useful for unit tests that don't need a real socket peer.
    pub fn unbound(initial: C::Value) -> (Self, InputInjector<C>) {
        let (tx, rx) = async_channel::unbounded();
        (
            Self {
                socket: None,
                override_rx: rx,
                _override_keepalive: tx.clone(),
                state: initial,
            },
            InputInjector { override_tx: tx },
        )
    }

    /// Latest cached value, after draining any queued datagrams / injections.
    pub fn value(&mut self) -> C::Value {
        self.drain_pending();
        self.state
    }

    /// Await the next datagram or injection, then return the (now latest) value.
    pub async fn recv(&mut self) -> C::Value {
        self.recv_one().await;
        self.state
    }

    /// Drain any queued datagrams and injected overrides into `self.state`.
    /// Non-blocking — returns once both sources would block. Datagrams that
    /// don't match the codec's wire format are logged at trace and skipped.
    fn drain_pending(&mut self) {
        if let Some(sock) = &self.socket {
            let mut buf = [0u8; MAX_DATAGRAM];
            loop {
                match sock.get_ref().recv(&mut buf) {
                    Ok(n) => match C::decode(&buf[..n]) {
                        Some(v) => self.state = v,
                        None => {
                            tracing::trace!(len = n, "uds-io: ignoring unparseable datagram")
                        }
                    },
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
    /// and apply it to `self.state`. Datagrams that don't match the codec's
    /// wire format do not update state; a caller looping for an edge will
    /// simply re-await on the next datagram.
    async fn recv_one(&mut self) {
        let new_state: Option<C::Value> = {
            let socket = self.socket.as_ref();
            let override_rx = &self.override_rx;
            match socket {
                Some(sock) => {
                    let socket_fut = async {
                        let mut buf = [0u8; MAX_DATAGRAM];
                        let n = sock.read_with(|s| s.recv(&mut buf)).await.ok()?;
                        let parsed = C::decode(&buf[..n]);
                        if parsed.is_none() {
                            tracing::trace!(len = n, "uds-io: ignoring unparseable datagram");
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

impl<C: Codec> InputInjector<C> {
    /// Inject a value. Non-blocking; uses an unbounded channel so this never
    /// fails under normal conditions.
    pub fn inject(&self, value: C::Value) {
        if self.override_tx.try_send(value).is_err() {
            tracing::warn!("uds-io: injector try_send failed");
        }
    }
}

// --- Digital (bool-codec) specialization: the embedded-hal pin traits ---

impl ErrorType for DatagramInput<BoolCodec> {
    type Error = Infallible;
}

impl InputPin for DatagramInput<BoolCodec> {
    fn is_high(&mut self) -> Result<bool, Self::Error> {
        Ok(self.value())
    }

    fn is_low(&mut self) -> Result<bool, Self::Error> {
        Ok(!self.value())
    }
}

impl Wait for DatagramInput<BoolCodec> {
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
        // Instead, walk events one-at-a-time looking for the false->true edge.
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
/// socket returns `ECONNREFUSED` when the file exists but no process is bound
/// (the crash case) and succeeds when a process is bound. The probe is the only
/// way to distinguish the two — `path.exists()` alone tells us nothing about
/// liveness, and an unconditional `remove_file` would silently steal a
/// concurrent process's socket. The bind retry after `remove_file` is racy
/// against another process binding in between, but losing that race surfaces as
/// a clean `AddrInUse` rather than data corruption.
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
