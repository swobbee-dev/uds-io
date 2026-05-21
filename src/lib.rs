//! Unix datagram socket-backed digital pins implementing the
//! `embedded-hal-async` digital traits.
//!
//! Designed for software-in-the-loop (SIL) simulation: each pin maps to a
//! `UnixDatagram` socket; outputs send a single byte (0x00 = LOW, 0x01 = HIGH)
//! to a peer path; inputs bind to a path and receive bytes from peers.
//!
//! The crate is runtime-agnostic: it depends on `async-io` for the reactor
//! and spawns no background tasks. It works under any async executor that
//! polls the returned futures (tokio, smol, async-std, embassy-on-host, ...).

mod input;
mod output;

pub use input::{DatagramInputPin, InputPinInjector};
pub use output::DatagramOutputPin;

pub(crate) const LOW: u8 = 0x00;
pub(crate) const HIGH: u8 = 0x01;

pub(crate) fn byte_to_bool(b: u8) -> bool {
    b != LOW
}

pub(crate) fn bool_to_byte(v: bool) -> u8 {
    if v {
        HIGH
    } else {
        LOW
    }
}
