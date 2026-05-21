# uds-io

`embedded-hal` / `embedded-hal-async` digital pin implementations backed by Unix datagram sockets (`AF_UNIX SOCK_DGRAM`).

The crate exists for **software-in-the-loop (SIL) simulation**: code written against the `embedded-hal` digital traits can run on a host machine with its pins wired — over Unix sockets — to other processes such as simulators, test harnesses, or fault injectors.

> **Note on authorship**
> This crate was written by [Claude](https://claude.com/claude-code) (Anthropic's Claude Code). It is published as-is; review it before relying on it.

## What you get

- `DatagramOutputPin` — implements `embedded_hal::digital::OutputPin` and `StatefulOutputPin`. Each `set_high` / `set_low` sends one byte to a peer Unix datagram path.
- `DatagramInputPin` — implements `embedded_hal::digital::InputPin` and `embedded_hal_async::digital::Wait`. State updates arrive from either the bound socket or an out-of-band `InputPinInjector` handle.
- `InputPinInjector` — cloneable handle for directly injecting pin state from in-process code (tests, fault injection, gRPC services, …) without going through the socket.

### Wire format

One ASCII byte per datagram. Strictly enforced — any other value is logged at `trace` and dropped:

| Byte    | Meaning |
| ------- | ------- |
| `b'0'`  | LOW     |
| `b'1'`  | HIGH    |

Strict acceptance keeps the wire interpretable by eyeball (`socat`, `nc -uU`) and prevents stray traffic on a shared path from being silently misread as a level change. Any process that can `send(2)` an ASCII `0` or `1` to a Unix datagram socket can drive an input pin.

## Runtime

The library uses `async-io` for the reactor and `async-channel` for the injector. **No background tasks are spawned** — pins are purely reactive on the futures the caller polls. It works under any async executor (tokio, smol, async-std, embassy-on-host, …); the test suite uses `async-std` to demonstrate that.

## Example

```rust
use embedded_hal::digital::OutputPin;
use embedded_hal_async::digital::Wait;
use uds_io::{DatagramInputPin, DatagramOutputPin};

# async fn run() -> std::io::Result<()> {
// Bind an input pin to a path.
let (mut input, injector) = DatagramInputPin::bind("/tmp/uds-io-demo.sock", false)?;

// Point an output pin at that path.
let mut output = DatagramOutputPin::connect("/tmp/uds-io-demo.sock", false)?;

// Drive it.
output.set_high().unwrap();
input.wait_for_high().await.unwrap();

// Or inject state directly, bypassing the socket.
injector.inject(false);
input.wait_for_low().await.unwrap();
# Ok(()) }
```

## Behavioral notes

A few things worth knowing, because they're not obvious from the type signatures:

- **Outputs don't fail.** Both pins use `Infallible` as their `embedded_hal::digital::ErrorType::Error`. If the peer isn't bound (`ENOENT`) or its receive buffer is full (`EWOULDBLOCK`), the send is dropped and logged at `trace` level via `tracing`. This matches real GPIO semantics — outputs don't error because nobody is listening.
- **Edge detection does not pre-drain.** `wait_for_rising_edge` / `wait_for_falling_edge` walk events one at a time. Draining first would collapse a queued `[low, high]` pair into "state is high" and lose the edge.
- **Level reads do drain.** `is_high` / `is_low` drain any queued datagrams first, so they always reflect the most recent state in the kernel buffer.
- **Injector channel is unbounded.** `InputPinInjector::inject` is non-blocking and never applies backpressure.
- **A `DatagramInputPin` is not safe to share across tasks without external synchronization** — it mutates internal state from both the drain and recv paths.
- **`bind` refuses to clobber a live peer.** A stale socket file left behind by a crashed process is reclaimed automatically, but if another live process is bound to the path, `bind` fails with `AddrInUse` rather than ripping the socket out from under it. The probe (`connect(2)` to the path: `ECONNREFUSED` means stale, `Ok` means live) is the only way to distinguish the two cases.

## Building / testing

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

## License

MIT — see [LICENSE](LICENSE).
