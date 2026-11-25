use micromegas_telemetry::compression::{compress, decompress};

#[test]
fn round_trip_exactly_at_cap() {
    let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let compressed = compress(&data).expect("compress");
    let out = decompress(&compressed, data.len()).expect("decompress at cap");
    assert_eq!(out, data);
}

#[test]
fn over_cap_is_rejected() {
    let compressed = compress(&vec![0u8; 4097]).expect("compress");
    let err = decompress(&compressed, 4096).expect_err("must exceed cap");
    assert!(err.to_string().contains("4096"), "unexpected error: {err}");
}

#[test]
fn empty_round_trip_with_zero_cap() {
    let compressed = compress(&[]).expect("compress");
    let out = decompress(&compressed, 0).expect("decompress empty");
    assert!(out.is_empty());
}
