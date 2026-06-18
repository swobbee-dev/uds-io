//! Unix datagram socket-backed pins implementing the `embedded-hal-async`
//! digital traits — plus an analog (f64-pair) variant on the same transport.
//!
//! Designed for software-in-the-loop (SIL) simulation: each pin maps to a
//! `UnixDatagram` socket; an output sends one datagram per state change to a
//! peer path; an input binds to a path and receives datagrams from peers.
//!
//! The payload is governed by a [`Codec`]:
//! - [`BoolCodec`] — a single ASCII byte (`b'0'` = LOW, `b'1'` = HIGH). Any
//!   other byte is ignored — strict acceptance keeps the wire interpretable by
//!   eyeball (`socat`, `nc -uU`) and prevents stray traffic on a shared path
//!   from being silently misread as a level change. This is the digital pin.
//! - [`F64PairCodec`] — 16 bytes: two little-endian `f64`s. Used to carry an
//!   analog quantity such as a Norton source `(I_inject, G)` or a resolved node
//!   `(V_node, I)` across the same datagram transport.
//!
//! `DatagramInputPin` / `DatagramOutputPin` are back-compat aliases for the
//! bool-codec pins; `DatagramF64InputPin` / `DatagramF64OutputPin` are the
//! analog ones.
//!
//! The crate is runtime-agnostic: it depends on `async-io` for the reactor and
//! spawns no background tasks. It works under any async executor that polls the
//! returned futures (tokio, smol, async-std, embassy-on-host, ...).

mod input;
mod output;

pub use input::{DatagramInput, InputInjector};
pub use output::DatagramOutput;

/// The bool-codec (digital) input pin and its injector — the original API.
pub type DatagramInputPin = DatagramInput<BoolCodec>;
pub type InputPinInjector = InputInjector<BoolCodec>;
/// The bool-codec (digital) output pin — the original API.
pub type DatagramOutputPin = DatagramOutput<BoolCodec>;

/// The analog (f64-pair) input pin and its injector.
pub type DatagramF64InputPin = DatagramInput<F64PairCodec>;
pub type F64InputInjector = InputInjector<F64PairCodec>;
/// The analog (f64-pair) output pin.
pub type DatagramF64OutputPin = DatagramOutput<F64PairCodec>;

/// Largest datagram any built-in codec encodes/decodes (the f64-pair is 16).
pub(crate) const MAX_DATAGRAM: usize = 16;

/// Maps a typed pin value to/from the bytes on the wire. Implementors are
/// zero-sized markers; the pin types are generic over the codec.
pub trait Codec {
    /// The value carried by a pin using this codec.
    type Value: Copy + Send + 'static;
    /// Encode `v` into `buf` (which is at least [`MAX_DATAGRAM`] bytes) and
    /// return the number of bytes written.
    fn encode(v: Self::Value, buf: &mut [u8]) -> usize;
    /// Decode one received datagram. `None` if it doesn't match the wire
    /// format (wrong length / unknown bytes) — such datagrams are ignored.
    fn decode(buf: &[u8]) -> Option<Self::Value>;
}

const LOW: u8 = b'0';
const HIGH: u8 = b'1';

/// Single-byte digital codec (`b'0'` / `b'1'`). This is the original wire
/// format; `DatagramInputPin` / `DatagramOutputPin` use it.
pub struct BoolCodec;

impl Codec for BoolCodec {
    type Value = bool;

    fn encode(v: bool, buf: &mut [u8]) -> usize {
        buf[0] = if v { HIGH } else { LOW };
        1
    }

    fn decode(buf: &[u8]) -> Option<bool> {
        match buf {
            [LOW] => Some(false),
            [HIGH] => Some(true),
            _ => None,
        }
    }
}

/// Analog codec: two little-endian `f64`s (16 bytes). Carries a `(value, value)`
/// pair — e.g. a Norton source `(I_inject, G)` or a node state `(V_node, I)`.
pub struct F64PairCodec;

impl Codec for F64PairCodec {
    type Value = (f64, f64);

    fn encode((a, b): (f64, f64), buf: &mut [u8]) -> usize {
        buf[0..8].copy_from_slice(&a.to_le_bytes());
        buf[8..16].copy_from_slice(&b.to_le_bytes());
        16
    }

    fn decode(buf: &[u8]) -> Option<(f64, f64)> {
        if buf.len() != 16 {
            return None;
        }
        let a = f64::from_le_bytes(buf[0..8].try_into().ok()?);
        let b = f64::from_le_bytes(buf[8..16].try_into().ok()?);
        Some((a, b))
    }
}
