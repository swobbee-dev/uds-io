//! `DatagramOutputPin`: a `UnixDatagram`-backed output pin implementing
//! `embedded_hal::digital::OutputPin` and `embedded_hal::digital::StatefulOutputPin`.

use std::convert::Infallible;
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

use embedded_hal::digital::{ErrorType, OutputPin, StatefulOutputPin};

use crate::bool_to_byte;

/// An output pin that sends a single byte per state change to a peer Unix
/// datagram socket path.
///
/// Sends are non-blocking: if the peer is not bound (`ENOENT`) or its receive
/// buffer is full (`EAGAIN` / `EWOULDBLOCK`), the send is dropped and logged
/// at trace level. This matches GPIO semantics — outputs don't "fail" because
/// nobody is listening.
pub struct DatagramOutputPin {
    socket: UnixDatagram,
    peer: PathBuf,
    state: bool,
}

impl DatagramOutputPin {
    /// Create an unbound non-blocking datagram socket targeted at `peer_path`.
    /// Emits the initial state immediately (best-effort).
    pub fn connect(peer_path: impl Into<PathBuf>, initial: bool) -> io::Result<Self> {
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

    fn send(&self) {
        match self.socket.send_to(&[bool_to_byte(self.state)], &self.peer) {
            Ok(_) => {}
            Err(e) => {
                tracing::trace!(
                    error = %e,
                    peer = %self.peer.display(),
                    "uds-io: send_to failed (peer not bound / would block?)",
                );
            }
        }
    }
}

impl ErrorType for DatagramOutputPin {
    type Error = Infallible;
}

impl OutputPin for DatagramOutputPin {
    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.state = true;
        self.send();
        Ok(())
    }

    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.state = false;
        self.send();
        Ok(())
    }
}

impl StatefulOutputPin for DatagramOutputPin {
    fn is_set_high(&mut self) -> Result<bool, Self::Error> {
        Ok(self.state)
    }

    fn is_set_low(&mut self) -> Result<bool, Self::Error> {
        Ok(!self.state)
    }
}
