# AVLL Rust implementation and ExaLogLog comparison

Measured on 2026-10-23. The new library is `avloglog-rs` 0.1.0, imported as `avloglog`. The baseline is the supplied, unmodified `exaloglog-rs` 0.1.1 checkout, using ExaLogLog(2,24).

AVLL offers a modest average accuracy advantage over the ordinary large-cardinality range in this experiment, and its early exit substantially speeds mature-sketch updates. It does not win at every memory/cardinality point. ExaLogLog merges faster, preserves stronger set-operation guarantees, and remains more accurate near the finite 64-bit hash ceiling.

## What was implemented

* Eleven unsigned base-56 registers per 64-bit word, relative exponents, two history bits where representable, shared-floor promotion and early exit.
* The primary HLDLC estimator, including the reference SBS, DLC, HC, Mean+H, blend rules and embedded correction coefficients.
* Prehashed insertion, default xxh3 raw hashing, custom `BuildHasher` and hash-function adapters, batch insertion, reset, checked state import, equal-size merge and optional serde/bincode save/load.
* Tests for all seven hash functions used by the ExaLogLog project, all 64 hash-bit/rank boundaries and hash zero, packing, promotions, malformed persistence, merge/replay counterexamples, and 52 independently generated Java state/estimate fixtures.

The input API follows the supplied ExaLogLog crate. AVLL has no equivalent lossless downsize or ELL token format. The algorithm and its guarantees are compared in [algorithm.md](algorithm.md); [the README](../README.md) documents usage.

## Coverage and reproducibility

Five equal register-payload budgets were tested: 256, 512, 1,024, 2,048 and 4,096 bytes. Actual streams cover 70 cardinalities from zero through 1,000,000, including every value 0–32, powers of two, and a logarithmic decimal grid. Each checkpoint has 100 pseudorandom trials and both algorithms receive the same unique-hash stream. SplitMix64 is a permutation, so fixed-count stream hashes are distinct.

The separate full-range simulation covers 241 increasing checkpoints from 16 through `2^64`, with four checkpoints per octave. AVLL uses exact adaptive Poisson thinning through its public insertion API; ELL uses exact Poisson occupancy of its register/rank/fraction cells. Both represent a Poisson number of uniform 64-bit arrivals with mean N, including finite-hash collisions. AVLL arrival order, clipped history, duplicate hash arrivals and the dependence between rank and modular bucket are preserved. These are accuracy simulations; no simulated count is reported as actual insertion throughput.

The model overlaps real streams. Across overlapping powers from `2^10` through `2^19`, the ratio of simulated relative MSE to actual relative MSE plus `1/N` is tabulated in the [detailed summary](../benchmarks/results/full/summary.md). Poisson count variation is significant at small N, so fixed-count real streams provide the low-cardinality comparison.

The host was aarch64 macOS with Rust 1.99.1 and LLVM 23.1.1, using Cargo release optimization. The sandbox did not expose the CPU model. Insertion timings use seven repetitions of one million actual updates; estimate and clone-plus-merge timings use 700 repetitions. The workloads use up to 16,384 independently allocated sketches (4–64 MiB of register payload). Input generation, template simulation and allocation are excluded from insertion timing; cloning/allocation are included in clone-plus-merge. No CPU pinning, isolated host or timing confidence interval is claimed. See [metadata](../benchmarks/results/full/metadata.txt).

Reproduce from the crate root, with the sibling `../exaloglog` checkout present:

```sh
cargo test --all-features
cargo test --manifest-path benchmarks/Cargo.toml
cargo run --release --manifest-path benchmarks/Cargo.toml -- \
  --full --many-sketches 16384 --seed 0x243f6a8885a308d3 \
  --output benchmarks/results/full
python3 benchmarks/summarize.py benchmarks/results/full
```

Matplotlib is optional for plots; the summarizer uses the Python standard library for numeric tables. Cargo.lock files pin dependencies. Baseline Git commit: `aee01a882c884f7bfcdf42a9b262e60e87f81ba9`. Exact source checksums are included with the results.

## Accuracy at one million actual distinct hashes

| Payload bytes | AVLL relative RMSE | ELL relative RMSE |
| ---: | ---: | ---: |
| 256 | 4.232% | 4.133% |
| 512 | 3.385% | 3.172% |
| 1,024 | 1.809% | 2.152% |
| 2,048 | 1.543% | 1.474% |
| 4,096 | 1.063% | 1.122% |

These are observations from 100 trials. Individual points vary; they do not establish universal superiority.

## Broad large-cardinality accuracy

The following combines simulated checkpoints in `[2^20, 2^55]` with equal weight per checkpoint: `sqrt(mean(relative_RMSE_at_checkpoint^2))`. It is a pooled relative RMSE, not the AVLL paper's width-weighted mean absolute error, and no paper metric is relabeled as a measurement here.

| Payload bytes | AVLL pooled RMSE | ELL pooled RMSE | AVLL RMSE reduction |
| ---: | ---: | ---: | ---: |
| 256 | 4.211% | 4.329% | 2.7% |
| 512 | 2.905% | 3.045% | 4.6% |
| 1,024 | 2.059% | 2.133% | 3.5% |
| 2,048 | 1.436% | 1.520% | 5.5% |
| 4,096 | 1.018% | 1.081% | 5.8% |

With approximately centered Gaussian errors, 100 trials give a single-checkpoint RMSE roughly 7% relative standard error. Rare low-cardinality events need more trials for stable tails. AVLL checkpoints share evolving sketches, so they are correlated and cannot be treated as independent samples when constructing an aggregate confidence interval. The four-point octave grid reduces phase blind spots but does not enumerate every cardinality or prove a worst-case bound.

![Full-range accuracy](../benchmarks/results/full/accuracy.png)

## Finite-hash ceiling

At mean `N=2^64`, the retained reference calibration produces a large AVLL underbias across all five budgets. It contains no new finite-hash correction. ELL's discrete register model behaves better here.

| Payload bytes | AVLL bias | AVLL RMSE | ELL bias | ELL RMSE |
| ---: | ---: | ---: | ---: | ---: |
| 256 | -11.512% | 11.890% | 0.510% | 4.244% |
| 512 | -11.264% | 11.498% | -0.037% | 3.226% |
| 1,024 | -11.212% | 11.296% | 0.735% | 2.311% |
| 2,048 | -11.296% | 11.350% | -0.200% | 1.465% |
| 4,096 | -11.429% | 11.454% | 0.125% | 1.054% |

## Throughput and query/merge cost at 1 KiB

| Actual operation | AVLL | ELL |
| --- | ---: | ---: |
| Unique stream, ns/item | 2.78 | 3.70 |
| 1,024-item duplicate stream, ns/item | 4.93 | 3.30 |
| 16,384 initially empty sketches, ns/item | 7.53 | 6.13 |
| Uncached estimate, ns/query | 1503.93 | 1765.77 |
| Clone and merge, ns/operation | 7380.65 | 1073.15 |
| 16,384 sketches initially at mean 1,000,000, ns/item | 2.98 | 5.26 |
| 16,384 sketches initially at mean 1,000,000,000,000, ns/item | 1.17 | 5.07 |

Mature prefills are sampled outside the timed region and identically cloned into separate allocations; subsequent update streams are disjoint. Their cardinalities evolve during timing. Early exit becomes more effective as the floor rises. The full CSVs also show low-cardinality cases where AVLL is slower and the substantially higher cost of its denser register-wise merge.

Hash-plus-insert measurements cover xxh3, ahash, komihash, polymurhash, wyhash, t1ha2-atonce and tab-hash. These use the same adapter styles as the supplied ELL benchmark, including buffered/allocating adapters. Their fixed-seed error columns are single workload observations; seven timing repetitions are not seven independent accuracy trials. See [hash results](../benchmarks/results/full/hash_functions.csv).

## Correctness and reference limitations

All library tests with and without serde, the documentation example, benchmark model tests, formatting and Clippy checks pass. Cargo packaging and verification succeed. Java fixture regeneration reproduces the checked-in fixture byte for byte. The port computes HLDLC through a compact histogram and decodes sequential digits for scan operations.

Two extreme-shift behaviors are deliberately corrected: every u64 hash is accepted, including zero, and aligning floors at distance 64 or more discards lower bits rather than wrapping the shift. Ordinary streams match the pinned Java state and estimates.

AVLL's information loss remains visible: high relative exponents omit history and clamp extreme maxima, so replay after promotion, insertion ordering and merge trees can change the state. Executable counterexamples cover each case. Equal-rank synthetic register populations can also trigger the reference HLDLC pipeline's inappropriate SBS fallback and yield a very low estimate despite a high represented floor. No unvalidated calibration is added to hide these behaviors.

## Papers and source

* Brian Bushnell, [Arithmetic Variable LogLog: Advancing the Memory-Variance Frontier](https://arxiv.org/abs/2608.12575), arXiv:2608.12575v1.
* Otmar Ertl, [ExaLogLog: Space-Efficient and Practical Approximate Distinct Counting up to the Exa-Scale](https://arxiv.org/abs/2402.13726), arXiv:2402.13726v2.
* [Pinned BBTools AVLL source](https://github.com/bbushnell/BBTools/blob/efe4f7b0317d34488a344045383a69c9af908b8d/current/cardinality/ArithmeticVariableLogLog.java), commit `efe4f7b0317d34488a344045383a69c9af908b8d`.

The original BBTools license and the supplied ExaLogLog MIT notice are retained. This report separates published claims from measured Rust results.
