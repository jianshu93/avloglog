use avloglog::AvLogLog;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher};

const VALUES: [&str; 8] = [
    "apple",
    "banana",
    "cherry",
    "dragonfruit",
    "apple",
    "banana",
    "kiwi",
    "fig",
];

fn check_builder<S: BuildHasher>(builder: S) {
    let mut raw = AvLogLog::new(2048).unwrap();
    raw.add_raw_values_with_build_hasher(VALUES, &builder);
    let mut prehashed = AvLogLog::new(2048).unwrap();
    prehashed.add_hashes(VALUES.map(|x| AvLogLog::hash_value_with_build_hasher(x, &builder)));
    assert_eq!(raw, prehashed);
    let mut repeated = AvLogLog::new(2048).unwrap();
    for value in VALUES {
        repeated.add_raw_with_build_hasher(value, &builder);
    }
    assert_eq!(repeated, raw);
    assert!(raw.estimate().is_finite());
    assert!((raw.estimate() - 6.0).abs() < 0.6);
}

#[test]
fn xxh3_default_matches_explicit_hashes_and_builder() {
    let mut raw = AvLogLog::new(2048).unwrap();
    raw.add_raw_values(VALUES);
    let mut prehashed = AvLogLog::new(2048).unwrap();
    prehashed.add_hashes(VALUES.map(AvLogLog::hash_value));
    assert_eq!(raw, prehashed);
    let mut one_at_a_time = AvLogLog::new(2048).unwrap();
    for value in VALUES {
        one_at_a_time.add_raw(value);
    }
    assert_eq!(raw, one_at_a_time);
    check_builder(BuildHasherDefault::<xxhash_rust::xxh3::Xxh3>::default());
}

#[test]
fn ahash_64() {
    check_builder(ahash::RandomState::with_seeds(1, 2, 3, 4));
}

#[test]
fn komihash_64() {
    struct KomiBuilder;
    impl BuildHasher for KomiBuilder {
        type Hasher = komihash::v5::KomiHasher;
        fn build_hasher(&self) -> Self::Hasher {
            Self::Hasher::new(0x1234_5678_9abc_def0)
        }
    }
    check_builder(KomiBuilder);
}

#[test]
fn polymur_64() {
    check_builder(BuildHasherDefault::<polymur_hash::PolymurHasher>::default());
}

#[test]
fn wyhash_64() {
    check_builder(wyhash::WyHasherBuilder::default());
}

#[test]
fn t1ha2_atonce_64() {
    #[derive(Default)]
    struct T1ha {
        buf: Vec<u8>,
    }
    impl Hasher for T1ha {
        fn write(&mut self, bytes: &[u8]) {
            self.buf.extend_from_slice(bytes);
        }
        fn finish(&self) -> u64 {
            t1ha::t1ha2_atonce(&self.buf, 0)
        }
    }
    check_builder(BuildHasherDefault::<T1ha>::default());
}

#[test]
fn tabulation_hash_64() {
    let tab = tab_hash::Tab64Simple::new();
    let values = [1_u64, 2, 3, 5, 1, 2, 8];
    let mut raw = AvLogLog::new(2048).unwrap();
    raw.add_raw_values_with_hash_fn(values, |x| tab.hash(*x));
    let mut prehashed = AvLogLog::new(2048).unwrap();
    prehashed.add_hashes(values.map(|x| tab.hash(x)));
    assert_eq!(raw, prehashed);
    let mut one_at_a_time = AvLogLog::new(2048).unwrap();
    for value in values {
        one_at_a_time.add_raw_with_hash_fn(value, |x| tab.hash(*x));
    }
    assert_eq!(raw, one_at_a_time);
    assert!(raw.estimate() > 0.0 && raw.estimate().is_finite());
}

#[test]
fn constructors_round_registers_and_byte_budgets() {
    let a = AvLogLog::new(2048).unwrap();
    assert_eq!(a.num_registers(), 2057);
    assert_eq!(a.state().len(), 187);
    assert_eq!(a.global_nlz(), -1);
    assert_eq!(a.estimate(), 0.0);
    let b = AvLogLog::with_bytes(1024).unwrap();
    assert_eq!(b.num_registers(), 1408);
    assert_eq!(b.state().len() * 8, 1024);
    assert!(AvLogLog::with_bytes(1027).is_err());
    assert!(AvLogLog::new(0).is_err());
    assert!(AvLogLog::new(usize::MAX).is_err());
    assert!(AvLogLog::with_bytes(0).is_err());
    assert!(AvLogLog::with_bytes(7).is_err());
}

#[test]
fn add_alias_wrap_clone_and_reset() {
    let hashes = [
        u64::MAX,
        1 << 63,
        0x0123_4567_89ab_cdef,
        0x9e37_79b9_7f4a_7c15,
    ];
    let mut a = AvLogLog::new(22).unwrap();
    for hash in hashes {
        a.add(hash);
    }
    let mut b = AvLogLog::new(22).unwrap();
    b.add_hashes(hashes);
    assert_eq!(a, b);
    assert_eq!(
        a,
        AvLogLog::wrap(a.global_nlz(), a.state().to_vec()).unwrap()
    );
    let clone = a.clone();
    a.reset();
    assert_eq!(a, AvLogLog::new(22).unwrap());
    assert_ne!(a, clone);
}

#[test]
fn rejects_invalid_wrapped_state() {
    assert!(AvLogLog::wrap(-1, vec![]).is_err());
    assert!(AvLogLog::wrap(-2, vec![0]).is_err());
    assert!(AvLogLog::wrap(i32::MAX, vec![0]).is_err());
    assert!(AvLogLog::wrap(-1, vec![u64::MAX]).is_err());
    // A packed word must fit in the exact base-56 eleven-digit domain.
    assert!(AvLogLog::wrap(-1, vec![56_u64.pow(11)]).is_err());
}

#[test]
fn merge_at_fixed_floor_is_commutative_and_matches_union() {
    let hashes: Vec<_> = (0..128).map(AvLogLog::hash_value).collect();
    let mut left = AvLogLog::new(2048).unwrap();
    let mut right = AvLogLog::new(2048).unwrap();
    left.add_hashes(hashes[..80].iter().copied());
    right.add_hashes(hashes[48..].iter().copied());
    let mut ab = left.clone();
    let mut ba = right.clone();
    ab.add_sketch(&right).unwrap();
    ba.add_sketch(&left).unwrap();
    assert_eq!(ab, ba);
    let mut union = AvLogLog::new(2048).unwrap();
    union.add_hashes(hashes.iter().copied());
    assert_eq!(ab, union);
    let before = ab.clone();
    ab.add_sketch(&before).unwrap();
    assert_eq!(ab, before);
    let mut empty = AvLogLog::new(2048).unwrap();
    empty.add_sketch(&before).unwrap();
    assert_eq!(empty, before);
    assert!(empty.add_sketch(&AvLogLog::new(22).unwrap()).is_err());
}

#[test]
fn duplicates_and_order_are_invariant_before_floor_promotion() {
    let hashes: Vec<_> = (0..128).map(AvLogLog::hash_value).collect();
    let mut forward = AvLogLog::new(2048).unwrap();
    forward.add_hashes(hashes.iter().copied());
    let mut reverse = AvLogLog::new(2048).unwrap();
    reverse.add_hashes(hashes.iter().copied().rev());
    assert_eq!(forward.global_nlz(), -1);
    assert_eq!(forward, reverse);
    let saved = forward.clone();
    forward.add_hashes(hashes.iter().copied().cycle().take(128 * 10));
    assert_eq!(forward, saved);
}
