//! `DatagramOutput<C>`: a `UnixDatagram`-backed output pin generic over a
//! [`Codec`]. The bool-codec specialization implements
//! `embedded_hal::digital::OutputPin` / `StatefulOutputPin`; the analog one
//! exposes `set()` / `get()`.

use std::convert::Infallible;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

use embedded_hal::digital::{ErrorType, OutputPin, StatefulOutputPin};

use crate::{BoolCodec, Codec, MAX_DATAGRAM};

/// An output pin that sends one datagram per state change to a peer Unix
/// datagram socket path.
///
/// Sends are non-blocking: if the peer is not bound (`ENOENT`) or its receive
/// buffer is full (`EAGAIN` / `EWOULDBLOCK`), the send is dropped and logged at
/// trace level. This matches GPIO semantics — outputs don't "fail" because
/// nobody is listening.
pub struct DatagramOutput<C: Codec> {
    socket: UnixDatagram,
    peer: PathBuf,
    state: C::Value,
}

impl<C: Codec> DatagramOutput<C> {
    /// Create an unbound non-blocking datagram socket targeted at `peer_path`.
    /// Emits the initial state immediately (best-effort).
    pub fn connect(peer_path: impl Into<PathBuf>, initial: C::Value) -> io::Result<Self> {
        let socket = UnixDatagram::unbound()?;
        socket.set_nonblocking(true)?;
        let pin = Self {
            socket,
            peer: peer_path.into(),
            state: initial,
        };
        pin.send();
        Ok(pin)
    }

    /// Set the pin's value and emit it (best-effort).
    pub fn set(&mut self, value: C::Value) {
        self.state = value;
        self.send();
    }

    /// The last value set on this pin.
    pub fn get(&self) -> C::Value {
        self.state
    }

    fn send(&self) {
        let mut buf = [0u8; MAX_DATAGRAM];
        let n = C::encode(self.state, &mut buf);
        if let Err(e) = self.socket.send_to(&buf[..n], &self.peer) {
            tracing::trace!(
                error = %e,
                peer = %self.peer.display(),
                "uds-io: send_to failed (peer not bound / would block?)",
            );
        }
    }
}

// --- Digital (bool-codec) specialization: the embedded-hal pin traits ---

impl ErrorType for DatagramOutput<BoolCodec> {
    type Error = Infallible;
}

impl OutputPin for DatagramOutput<BoolCodec> {
    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.set(true);
        Ok(())
    }

    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.set(false);
        Ok(())
    }
}

impl StatefulOutputPin for DatagramOutput<BoolCodec> {
    fn is_set_high(&mut self) -> Result<bool, Self::Error> {
        Ok(self.state)
    }

    fn is_set_low(&mut self) -> Result<bool, Self::Error> {
        Ok(!self.state)
    }
}
