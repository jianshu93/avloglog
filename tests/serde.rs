#![cfg(feature = "serde")]
use avloglog::AvLogLog;

#[test]
fn save_load_preserves_state_and_continuation() {
    let mut original = AvLogLog::new(88).unwrap();
    original.add_raw_values(0_u64..32768);
    assert!(original.global_nlz() >= 0);
    let mut bytes = Vec::new();
    original.save(&mut bytes).unwrap();
    let mut restored = AvLogLog::load(bytes.as_slice()).unwrap();
    assert_eq!(original, restored);
    assert_eq!(original.estimate(), restored.estimate());
    original.add_raw_values(32768_u64..65536);
    restored.add_raw_values(32768_u64..65536);
    assert_eq!(original, restored);
}

#[test]
fn direct_serde_roundtrip_rebuilds_derived_counters() {
    let mut original = AvLogLog::new(88).unwrap();
    original.add_raw_values(0_u64..32768);
    let json = serde_json::to_string(&original).unwrap();
    let mut restored: AvLogLog = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
    original.add_raw_values(32768_u64..65536);
    restored.add_raw_values(32768_u64..65536);
    assert_eq!(original, restored);
}

#[test]
fn rejects_truncation_and_malformed_serde_state() {
    let original = AvLogLog::new(22).unwrap();
    let mut bytes = Vec::new();
    original.save(&mut bytes).unwrap();
    for len in 0..bytes.len() {
        assert!(AvLogLog::load(&bytes[..len]).is_err(), "len={len}");
    }
    assert!(AvLogLog::load(&b"garbage"[..]).is_err());
    for json in [
        r#"{"version":2,"global_nlz":-1,"state":[0]}"#,
        r#"{"version":1,"global_nlz":-2,"state":[0]}"#,
        r#"{"version":1,"global_nlz":2147483647,"state":[0]}"#,
        r#"{"version":1,"global_nlz":-1,"state":[]}"#,
        r#"{"version":1,"global_nlz":-1,"state":[18446744073709551615]}"#,
    ] {
        assert!(serde_json::from_str::<AvLogLog>(json).is_err(), "{json}");
    }
}

#[test]
fn multiple_saved_sketches_can_be_loaded_from_one_stream() {
    let mut first = AvLogLog::new(22).unwrap();
    let mut second = AvLogLog::new(88).unwrap();
    first.add_raw_values(0_u64..4096);
    second.add_raw_values(4096_u64..32768);
    let mut bytes = Vec::new();
    first.save(&mut bytes).unwrap();
    second.save(&mut bytes).unwrap();
    let mut reader = std::io::Cursor::new(bytes);
    assert_eq!(AvLogLog::load(&mut reader).unwrap(), first);
    assert_eq!(AvLogLog::load(&mut reader).unwrap(), second);
    assert_eq!(reader.position(), reader.get_ref().len() as u64);
}
