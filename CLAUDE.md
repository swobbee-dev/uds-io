# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this crate is

`uds-io` provides `embedded-hal` / `embedded-hal-async` digital pin implementations backed by Unix datagram sockets. It exists for software-in-the-loop (SIL) simulation: code written against the embedded-hal digital traits can run on a host machine with pins wired to other processes (simulators, test harnesses, fault injectors) over `AF_UNIX SOCK_DGRAM`.

Wire format: one byte per datagram. `0x00` = LOW, anything else = HIGH.

## Commands

```bash
cargo build
cargo test                                    # runs the tests/roundtrip.rs suite under async-std
cargo test --test roundtrip <name>            # run a single test by name substring
cargo clippy --all-targets -- -D warnings
cargo fmt
```

There are no examples or binaries — this is a library crate. Dev-dep tests use `async-std` deliberately to prove runtime-agnosticism (the lib itself uses `async-io` + `async-channel` and works under tokio, smol, async-std, etc.).

## Architecture

Three files in `src/`, all small:

- `lib.rs` — re-exports + the `LOW`/`HIGH` byte constants and `byte_to_bool` / `bool_to_byte` helpers. No background tasks are ever spawned; the lib is purely reactive on the futures the caller polls.
- `output.rs` — `DatagramOutputPin`: blocking `std::os::unix::net::UnixDatagram` set to non-blocking, sends one byte per `set_high`/`set_low` via `send_to(peer_path)`. Sends are best-effort: `ENOENT` (peer not bound) and `EWOULDBLOCK` (peer's rx buffer full) are logged at `trace` and swallowed. This matches GPIO semantics — outputs don't fail because nobody is listening.
- `input.rs` — `DatagramInputPin`: wraps `UnixDatagram` in `async_io::Async`, plus an `async_channel` for an out-of-band `InputPinInjector`. State updates come from *either* source. Two `bind` modes: `bind(path, initial)` binds a real socket (and removes a stale file at the path first), `unbound(initial)` is injector-only for unit tests.

Important behavioral details that aren't obvious from signatures:

- **Edge detection must not pre-drain.** `wait_for_rising_edge` / `wait_for_falling_edge` deliberately do *not* call `drain_pending` up front. Draining first would collapse a queued `[low, high]` pair into "state is high" and lose the edge. Instead they pull one event at a time via `recv_one` and watch for the transition. Don't "optimize" this by adding a pre-drain.
- **`is_high` / `is_low` drain first.** Level reads call `drain_pending` so they always reflect the most recent datagram in the kernel buffer, not a stale cached value.
- **Injector channel is unbounded.** `InputPinInjector::inject` is non-blocking and only logs (`tracing::warn!`) on the impossible `try_send` failure. Don't switch to a bounded channel without thinking through what backpressure means for a "GPIO pin" abstraction.
- **`DatagramInputPin` owns its socket and is `!Sync`-ish in practice** — it mutates `self.state` from both `drain_pending` and `recv_one`. Sharing across tasks requires external synchronization.

## Error type

Both pins use `Infallible` as their `embedded_hal::digital::ErrorType::Error`. Socket I/O failures are intentionally absorbed (logged via `tracing`) so they behave like real GPIO from the consumer's perspective. If you find yourself wanting to surface an `io::Error` up through the trait, reconsider — the embedded-hal contract is that digital pins don't fail.
