//! Roundtrip and behavior tests for `uds-io` pins.
//!
//! These tests use `async_std` to prove the crate is runtime-agnostic — the
//! same crate compiles and runs under a different executor than tokio.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::time::Duration;

use embedded_hal::digital::{InputPin, OutputPin, StatefulOutputPin};
use embedded_hal_async::digital::Wait;
use uds_io::{DatagramInputPin, DatagramOutputPin};

fn temp_path(name: &str) -> PathBuf {
    let dir = tempfile::tempdir().unwrap();
    // Keep dir alive via leak — fine for tests (process exits soon).
    let path = dir.path().join(format!("{name}.sock"));
    std::mem::forget(dir);
    path
}

#[async_std::test]
async fn output_high_triggers_input_wait_for_high() {
    let in_path = temp_path("trigger");
    let (mut input, _inj) = DatagramInputPin::bind(&in_path, false).unwrap();
    let mut output = DatagramOutputPin::connect(in_path.clone(), false).unwrap();

    output.set_high().unwrap();

    async_std::future::timeout(Duration::from_secs(1), input.wait_for_high())
        .await
        .expect("wait_for_high timed out")
        .unwrap();

    assert!(input.is_high().unwrap());
}

#[async_std::test]
async fn output_low_triggers_input_wait_for_low() {
    let in_path = temp_path("low");
    let (mut input, _inj) = DatagramInputPin::bind(&in_path, true).unwrap();
    let mut output = DatagramOutputPin::connect(in_path.clone(), true).unwrap();

    output.set_low().unwrap();

    async_std::future::timeout(Duration::from_secs(1), input.wait_for_low())
        .await
        .expect("wait_for_low timed out")
        .unwrap();

    assert!(input.is_low().unwrap());
}

#[async_std::test]
async fn wait_for_rising_edge_only_fires_on_low_to_high() {
    let in_path = temp_path("rising");
    let (mut input, _inj) = DatagramInputPin::bind(&in_path, true).unwrap();
    let mut output = DatagramOutputPin::connect(in_path.clone(), true).unwrap();

    // Drain any initial state datagrams.
    let _ = input.is_high();

    // Drive low then high; only the low->high should resolve wait_for_rising_edge.
    let fut = input.wait_for_rising_edge();
    output.set_low().unwrap();
    // Give the falling edge a moment to land in the kernel buffer.
    async_std::task::sleep(Duration::from_millis(20)).await;
    output.set_high().unwrap();

    async_std::future::timeout(Duration::from_secs(1), fut)
        .await
        .expect("wait_for_rising_edge timed out")
        .unwrap();
}

#[async_std::test]
async fn wait_for_falling_edge_only_fires_on_high_to_low() {
    let in_path = temp_path("falling");
    let (mut input, _inj) = DatagramInputPin::bind(&in_path, false).unwrap();
    let mut output = DatagramOutputPin::connect(in_path.clone(), false).unwrap();

    let _ = input.is_low();

    let fut = input.wait_for_falling_edge();
    output.set_high().unwrap();
    async_std::task::sleep(Duration::from_millis(20)).await;
    output.set_low().unwrap();

    async_std::future::timeout(Duration::from_secs(1), fut)
        .await
        .expect("wait_for_falling_edge timed out")
        .unwrap();
}

#[async_std::test]
async fn wait_for_any_edge_resolves_on_change() {
    let in_path = temp_path("any");
    let (mut input, _inj) = DatagramInputPin::bind(&in_path, false).unwrap();
    let mut output = DatagramOutputPin::connect(in_path.clone(), false).unwrap();

    let _ = input.is_low();

    let fut = input.wait_for_any_edge();
    output.set_high().unwrap();

    async_std::future::timeout(Duration::from_secs(1), fut)
        .await
        .expect("wait_for_any_edge timed out")
        .unwrap();
    assert!(input.is_high().unwrap());
}

#[async_std::test]
async fn injector_drives_unbound_pin() {
    let (mut input, injector) = DatagramInputPin::unbound(false);

    injector.inject(true);

    async_std::future::timeout(Duration::from_secs(1), input.wait_for_high())
        .await
        .expect("wait_for_high timed out")
        .unwrap();
    assert!(input.is_high().unwrap());
}

#[async_std::test]
async fn injector_overrides_bound_pin_without_peer() {
    // The pin has a real bound socket, but no peer is sending.
    let in_path = temp_path("inj_bound");
    let (mut input, injector) = DatagramInputPin::bind(&in_path, false).unwrap();

    injector.inject(true);

    async_std::future::timeout(Duration::from_secs(1), input.wait_for_high())
        .await
        .expect("wait_for_high timed out")
        .unwrap();
    assert!(input.is_high().unwrap());
}

#[async_std::test]
async fn is_high_reflects_latest_received_state() {
    let in_path = temp_path("latest");
    let (mut input, _inj) = DatagramInputPin::bind(&in_path, false).unwrap();
    let mut output = DatagramOutputPin::connect(in_path.clone(), false).unwrap();

    // Burst several updates; only the final should matter for is_high.
    output.set_high().unwrap();
    output.set_low().unwrap();
    output.set_high().unwrap();
    output.set_low().unwrap();

    // Allow the datagrams to be enqueued in the kernel.
    async_std::task::sleep(Duration::from_millis(20)).await;

    assert!(input.is_low().unwrap(), "expected final state LOW");
}

#[async_std::test]
async fn output_to_unbound_peer_does_not_panic() {
    let missing = PathBuf::from("/tmp/uds-io-test-does-not-exist-XXXX.sock");
    let mut output = DatagramOutputPin::connect(missing, false).unwrap();
    output.set_high().unwrap();
    output.set_low().unwrap();
    // No assertion: just verifying we don't panic / propagate an error.
    assert!(output.is_set_low().unwrap());
}

#[async_std::test]
async fn stateful_output_pin_tracks_state() {
    let in_path = temp_path("stateful");
    let mut output = DatagramOutputPin::connect(in_path, false).unwrap();
    assert!(output.is_set_low().unwrap());
    output.set_high().unwrap();
    assert!(output.is_set_high().unwrap());
    output.set_low().unwrap();
    assert!(output.is_set_low().unwrap());
}

#[async_std::test]
async fn bind_removes_stale_socket_file() {
    let path = temp_path("stale");
    // Pre-create a stale socket file.
    let stale = UnixDatagram::bind(&path).unwrap();
    drop(stale);
    assert!(path.exists());

    // bind() should remove the stale file and succeed.
    let (_pin, _inj) = DatagramInputPin::bind(&path, false).unwrap();
}

#[async_std::test]
async fn bind_refuses_to_clobber_a_live_peer() {
    let path = temp_path("live_peer");
    // A live bound socket — held for the duration of the test.
    let _live = UnixDatagram::bind(&path).unwrap();

    match DatagramInputPin::bind(&path, false) {
        Ok(_) => panic!("bind must refuse to displace a live owner"),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse),
    }
}

#[async_std::test]
async fn unknown_wire_bytes_do_not_change_state() {
    let path = temp_path("unknown_byte");
    let (mut input, _inj) = DatagramInputPin::bind(&path, false).unwrap();

    // Send a byte that is neither b'0' nor b'1'.
    let peer = UnixDatagram::unbound().unwrap();
    peer.send_to(b"X", &path).unwrap();

    async_std::task::sleep(Duration::from_millis(20)).await;
    assert!(input.is_low().unwrap(), "unknown byte must not flip state");
}
