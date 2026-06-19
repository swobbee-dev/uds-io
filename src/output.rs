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
/// Sends are non-blocking. `set` is best-effort (GPIO semantics — an output
/// doesn't fail because nobody is listening); `set_delivered` surfaces the
/// `send_to` result so a caller can retry until a late-binding peer is reachable.
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
        let _ = pin.send();
        Ok(pin)
    }

    /// Set the pin's value and emit it (best-effort).
    pub fn set(&mut self, value: C::Value) {
        self.state = value;
        let _ = self.send();
    }

    /// Set the pin's value and emit it, returning whether the datagram was
    /// delivered. `Err` (typically `ENOENT`) means the peer has not bound its
    /// socket yet — a caller that needs the value to land can retry.
    pub fn set_delivered(&mut self, value: C::Value) -> io::Result<()> {
        self.state = value;
        self.send()
    }

    /// The last value set on this pin.
    pub fn get(&self) -> C::Value {
        self.state
    }

    fn send(&self) -> io::Result<()> {
        let mut buf = [0u8; MAX_DATAGRAM];
        let n = C::encode(self.state, &mut buf);
        self.socket.send_to(&buf[..n], &self.peer).map(|_| ())
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
