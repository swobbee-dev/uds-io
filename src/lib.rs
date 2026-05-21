//! Unix datagram socket-backed digital pins implementing the
//! `embedded-hal-async` digital traits.
//!
//! Designed for software-in-the-loop (SIL) simulation: each pin maps to a
//! `UnixDatagram` socket; outputs send a single ASCII byte (`b'0'` = LOW,
//! `b'1'` = HIGH) to a peer path; inputs bind to a path and receive bytes
//! from peers. Any other byte value is ignored — strict acceptance keeps
//! the wire interpretable by eyeball (`socat`, `nc -uU`) and prevents stray
//! traffic on a shared path from being silently misread as a level change.
//!
//! The crate is runtime-agnostic: it depends on `async-io` for the reactor
//! and spawns no background tasks. It works under any async executor that
//! polls the returned futures (tokio, smol, async-std, embassy-on-host, ...).

mod input;
mod output;

pub use input::{DatagramInputPin, InputPinInjector};
pub use output::DatagramOutputPin;

pub(crate) const LOW: u8 = b'0';
pub(crate) const HIGH: u8 = b'1';

/// Parse a single wire byte. `Some(false)` for LOW (`b'0'`), `Some(true)`
/// for HIGH (`b'1'`), `None` for any other byte. Strict by design — see
/// the crate-level docs.
pub(crate) fn byte_to_bool(b: u8) -> Option<bool> {
    match b {
        LOW => Some(false),
        HIGH => Some(true),
        _ => None,
    }
}

pub(crate) fn bool_to_byte(v: bool) -> u8 {
    if v { HIGH } else { LOW }
}
