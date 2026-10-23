# avloglog

Standalone Rust implementation of **Arithmetic Variable LogLog (AVLL)** from
[Bushnell (2026)](https://arxiv.org/abs/2608.12575), using the HLDLC estimator
and the [BBTools reference class](https://github.com/bbushnell/BBTools/blob/efe4f7b0317d34488a344045383a69c9af908b8d/current/cardinality/ArithmeticVariableLogLog.java).
The public input and persistence APIs follow
[Jianshu Zhao's ExaLogLog crate](https://github.com/jianshu93/exaloglog).

AVLL packs eleven base-56 registers into each `u64`. A shared rank floor permits
early rejection of most hashes at high cardinality. HLDLC combines dynamic
linear counting, state-based sampling, history counting, and a corrected
harmonic mean, using the reference implementation's embedded calibration.

## Usage

```toml
[dependencies]
avloglog = { package = "avloglog-rs", path = "../avloglog" }
```

```rust
use avloglog::AvLogLog;

let mut sketch = AvLogLog::with_bytes(1024).unwrap(); // 1,408 registers
sketch.add_hash(0x0123_4567_89ab_cdef); // already uniformly hashed
sketch.add_hashes([0xfedc_ba98_7654_3210, 0x9e37_79b9_7f4a_7c15]);
sketch.add_raw_values(["apple", "banana", "cherry"]); // default xxh3-64
sketch.add_raw_with_hash_fn(42u64, |v| AvLogLog::hash_value(v));
println!("{}", sketch.estimate());
```

`new(registers)` rounds the requested count up to a multiple of eleven.
`with_bytes(bytes)` allocates exactly that many register bytes; `bytes` must be
a positive multiple of eight. `Default` uses 1,024 bytes. `memory_bytes()`
excludes the Rust object metadata and allocator overhead.

Custom `BuildHasher` support matches ExaLogLog:

```rust
# use avloglog::AvLogLog;
let mut sketch = AvLogLog::default();
let hasher = std::collections::hash_map::RandomState::new();
sketch.add_raw_with_build_hasher("apple", &hasher);
sketch.add_raw_values_with_build_hasher(["banana", "cherry"], &hasher);
```

Use the same hash function, seed, and input encoding across sketches to be
merged. Raw input uses Rust's `Hash` encoding, whose cross-platform stability
is not guaranteed. For persistent cross-language formats, hash a defined byte
encoding externally and call `add_hash`.

## Merge and persistence

```rust
# use avloglog::AvLogLog;
let mut a = AvLogLog::with_bytes(1024).unwrap();
let mut b = AvLogLog::with_bytes(1024).unwrap();
a.add_raw_values(["apple", "banana"]);
b.add_raw_values(["banana", "cherry"]);
a.add_sketch(&b).unwrap();
let merged = AvLogLog::merge(&a, &b).unwrap();
```

Merge requires equal register counts. AVLL does not support ExaLogLog's token
format or lossless downsize operation. Rare high-rank history truncation means
AVLL does **not** guarantee exact insertion-order invariance, duplicate
idempotency after promotions, or merge associativity. This is an inherited
algorithmic tradeoff, covered by regression tests. See
[algorithm and paper comparison](docs/algorithm.md).

Enable optional serde and bincode persistence:

```toml
avloglog = { package = "avloglog-rs", path = "../avloglog", features = ["serde"] }
```

```rust
# #[cfg(feature = "serde")] {
# use avloglog::AvLogLog;
let mut sketch = AvLogLog::default();
sketch.add_raw_values(["apple", "banana"]);
let mut bytes = Vec::new();
sketch.save(&mut bytes).unwrap();
let restored = AvLogLog::load(bytes.as_slice()).unwrap();
assert_eq!(sketch, restored);
# }
```

Serde's direct deserializer and `load` both validate the format version,
shared floor, state length and arithmetic encoding, then rebuild derived
counters. `wrap(global_nlz, words)` restores packed state with validation;
the initial floor is `-1`. Preserve the floor together with `state()`.

## Tests and benchmarks

```bash
cargo test
cargo test --all-features
cargo clippy --all-features --all-targets -- -D warnings
cargo run --release --manifest-path benchmarks/Cargo.toml -- --help
```

Tests include Java reference state and estimate fixtures, floor promotion,
merge alignment, all 64 rank boundaries, unsigned packing, serde
round trips and invalid input, and the seven hash functions used by the
ExaLogLog project: xxh3, ahash, komihash, polymur, wyhash, t1ha2 and tab-hash.

The separate `benchmarks/` package compares equal register-storage budgets
against the existing sibling `../exaloglog` crate, using ExaLogLog(2,24).
Its CSVs distinguish fixed-cardinality real streams, Poissonized simulations
through `2^64`, throughput, estimate/merge latency, and hash+insert timing.
See `benchmarks/README.md` and `docs/benchmark-report.md` for commands, measured
results, environment and statistical limitations. A logarithmic grid samples
the full range; it does not enumerate every cardinality.

## Reference deviations and attribution

This port clamps extremely high input ranks before shifts, including hash
zero, instead of silently ignoring them as Java does when `bitPos >= 64`.
Merge shifts of 64 or more produce zero rather than wrapping modulo 64.
It computes HLDLC from a compact state histogram and omits unused alternative
estimators and Java's cached cardinality result. Neither Rust nor ExaLogLog
estimate latency in the benchmark uses a cached result.

The calibration constants are unchanged. They are empirical; AVLL does not
add a new finite-hash collision correction near the 64-bit ceiling. The
benchmark reports behavior there rather than extending the paper's accuracy
claim to that range.

See [NOTICE](NOTICE), [LICENSE](LICENSE) and [LICENSE-EXALOGLOG](LICENSE-EXALOGLOG)
for retained source attribution and license terms. This is a new standalone
package; it does not modify the ExaLogLog implementation.
