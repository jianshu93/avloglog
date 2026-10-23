use avloglog::AvLogLog;

// Select an exactly specified geometric rank and modular bucket without hashing.
fn hash_for_rank_bucket(rank: u32, bucket: usize, registers: usize) -> u64 {
    assert!(rank < 60);
    let base = 1_u64 << (63 - rank);
    let delta = (bucket as u64 + registers as u64 - base % registers as u64) % registers as u64;
    let value = base + delta;
    assert_eq!(value.leading_zeros(), rank);
    assert_eq!(value % registers as u64, bucket as u64);
    value
}

fn register(sketch: &AvLogLog, index: usize) -> u64 {
    (sketch.state()[index / 11] / 56_u64.pow((index % 11) as u32)) % 56
}

fn splitmix(n: u64) -> u64 {
    let mut z = n.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[test]
fn high_rank_hashes_including_zero_are_not_ignored_or_shifted_modulo_64() {
    // Intentional correction to the Java reference: clamp out-of-window events
    // before shifting, including its originally ignored NLZ=62,63,64 cases.
    for hash in [0_u64, 1, 2, 3, 4, 7, 1 << 20] {
        let mut a = AvLogLog::new(11).unwrap();
        a.add_hash(hash);
        assert_eq!(register(&a, (hash % 11) as usize), 55, "hash={hash}");
        assert!(a.estimate() > 0.0 && a.estimate().is_finite());
        let saved = a.clone();
        a.add_hash(hash);
        assert_eq!(a, saved);
    }
}

#[test]
fn every_hash_bit_and_rank_boundary_is_accepted() {
    for bit in 0..64 {
        let mut a = AvLogLog::new(22).unwrap();
        a.add_hash(1_u64 << bit);
        assert!(a.state().iter().any(|x| *x != 0), "bit={bit}");
        assert!(a.estimate() > 0.0 && a.estimate().is_finite());
    }
}

#[test]
fn documents_reference_history_loss_after_promotion() {
    // Exponent14 discards history. After demotion to exponent13, replaying the
    // already-seen rank11 event populates a history bit that was discarded.
    // This is an intrinsic AVLL reference limitation, not set-union invariance.
    let top = hash_for_rank_bucket(12, 0, 11);
    let history = hash_for_rank_bucket(11, 0, 11);
    let mut a = AvLogLog::new(11).unwrap();
    a.add_hash(top).add_hash(history);
    assert_eq!(register(&a, 0), 51); // relative exponent14, no history
    for i in 1..11 {
        a.add_hash(hash_for_rank_bucket(1, i, 11));
    }
    assert_eq!(a.global_nlz(), 0);
    assert_eq!(register(&a, 0), 47); // relative exponent13, history00
    let before = a.clone();
    a.add_hash(history);
    assert_eq!(register(&a, 0), 49); // relative exponent13, history10
    assert_ne!(before, a);
    // Processing the same multiset in a different order changes packed state.
    let mut reordered = AvLogLog::new(11).unwrap();
    reordered.add_hash(top);
    for i in 1..11 {
        reordered.add_hash(hash_for_rank_bucket(1, i, 11));
    }
    reordered.add_hash(history);
    assert_eq!(a, reordered);
    assert_ne!(before, reordered);
}

#[test]
fn merge_aligns_floors_when_difference_exceeds_word_width() {
    // Avoid Rust shift-by-64 panics / Java's masked shift distance when aligning
    // synthetic valid states at widely separated floors.
    let mut lower = AvLogLog::wrap(-1, vec![55]).unwrap();
    let upper = AvLogLog::wrap(63, vec![0]).unwrap();
    lower.add_sketch(&upper).unwrap();
    assert_eq!(lower.global_nlz(), 63);
    assert_eq!(lower.state(), &[0]);
    assert!(lower.estimate().is_finite());
}

#[test]
fn deterministic_accuracy_smoke_across_low_transition_and_high_ranges() {
    let mut a = AvLogLog::new(2048).unwrap();
    let mut previous = 0;
    for count in [1_u64, 8, 64, 512, 2048, 8192, 32768, 131072, 524288] {
        a.add_hashes((previous..count).map(splitmix));
        let estimate = a.estimate();
        // A deterministic smoke test, not a claim about confidence bounds.
        // Full multi-trial RMSE/bias results belong to the benchmark executable.
        assert!(estimate.is_finite() && estimate >= 0.0);
        assert!(
            (estimate / count as f64 - 1.0).abs() < 0.20,
            "count={count}, estimate={estimate}"
        );
        previous = count;
    }
    assert!(a.global_nlz() > -1);
}

#[test]
fn sliding_floor_does_not_impose_an_absolute_exponent_18_ceiling() {
    let mut a = AvLogLog::new(88).unwrap();
    // Controlled synthetic ranks force promotion without inserting quadrillions
    // of elements. These are numerical/range tests, not true-cardinality tests.
    for rank in 1..=52 {
        for bucket in 0..88 {
            a.add_hash(hash_for_rank_bucket(rank, bucket, 88));
        }
        assert_eq!(a.global_nlz(), rank as i32 - 1);
        assert!(a.estimate().is_finite() && a.estimate() > 0.0);
    }
    assert!((3..=6).contains(&register(&a, 0))); // exp2 at the current moving floor
    // Uniform single-tier states are a degenerate input for Java's blended
    // estimator (no usable DLC tier). Test large estimates on a varying-rank
    // synthetic stream instead of asserting statistical meaning for this case.
    let mut varied = AvLogLog::new(88).unwrap();
    varied.add_hashes((0..65536).map(|i| splitmix(i) >> 48));
    assert!(varied.global_nlz() > 18);
    assert!(varied.estimate() > (1_u64 << 52) as f64);
}

#[test]
fn documents_merge_tree_sensitivity_when_history_is_discarded() {
    // Same retained events as the replay counterexample, distributed across
    // three shards. Merging the top+history shard before floor promotion drops
    // information that survives if the history+floor shards are merged first.
    let mut top = AvLogLog::new(11).unwrap();
    top.add_hash(hash_for_rank_bucket(12, 0, 11));
    let mut history = AvLogLog::new(11).unwrap();
    history.add_hash(hash_for_rank_bucket(11, 0, 11));
    let mut floor = AvLogLog::new(11).unwrap();
    for bucket in 1..11 {
        floor.add_hash(hash_for_rank_bucket(1, bucket, 11));
    }
    let mut left_tree = top.clone();
    left_tree
        .add_sketch(&history)
        .unwrap()
        .add_sketch(&floor)
        .unwrap();
    let mut right_partial = history.clone();
    right_partial.add_sketch(&floor).unwrap();
    let mut right_tree = top;
    right_tree.add_sketch(&right_partial).unwrap();
    assert_eq!(left_tree.global_nlz(), 0);
    assert_eq!(right_tree.global_nlz(), 0);
    assert_eq!(register(&left_tree, 0), 47);
    assert_eq!(register(&right_tree, 0), 49);
    assert_ne!(left_tree, right_tree);
}
