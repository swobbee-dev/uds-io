//! Analog (f64-pair) codec + pin tests.

use uds_io::{Codec, DatagramF64InputPin, DatagramF64OutputPin, F64PairCodec};

#[test]
fn f64_pair_codec_round_trips() {
    let mut buf = [0u8; 16];
    let n = F64PairCodec::encode((54.25, -5.0), &mut buf);
    assert_eq!(n, 16);
    assert_eq!(F64PairCodec::decode(&buf[..n]), Some((54.25, -5.0)));
}

#[test]
fn f64_pair_codec_rejects_wrong_length() {
    assert_eq!(F64PairCodec::decode(&[0u8; 8]), None);
    assert_eq!(F64PairCodec::decode(&[0u8; 1]), None);
    assert_eq!(F64PairCodec::decode(&[]), None);
}

#[async_std::test]
async fn f64_output_reaches_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("net.sock");

    let (mut input, _inj) = DatagramF64InputPin::bind(&path, (0.0, 0.0)).unwrap();
    let mut output = DatagramF64OutputPin::connect(&path, (0.0, 0.0)).unwrap();
    let _ = input.recv().await; // consume connect()'s initial (0,0) datagram

    output.set((54.25, -5.0));
    let got = input.recv().await;
    assert_eq!(got, (54.25, -5.0));
    assert_eq!(output.get(), (54.25, -5.0));
}

#[async_std::test]
async fn f64_input_value_caches_latest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("net2.sock");

    let (mut input, _inj) = DatagramF64InputPin::bind(&path, (1.0, 2.0)).unwrap();
    assert_eq!(input.value(), (1.0, 2.0)); // initial

    let mut output = DatagramF64OutputPin::connect(&path, (0.0, 0.0)).unwrap();
    let _ = input.recv().await; // consume connect()'s initial (0,0)

    output.set((10.0, 0.5));
    output.set((20.0, 0.25)); // a later send supersedes the earlier one
    let _ = input.recv().await; // ensure at least one arrived...
    assert_eq!(input.value(), (20.0, 0.25)); // ...then drain to the latest
}
