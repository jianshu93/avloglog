# Algorithm and paper comparison

This library implements Arithmetic Variable LogLog (AVLL), using the HLDLC estimator in Brian Bushnell's [Arithmetic Variable LogLog: Advancing the Memory-Variance Frontier](https://arxiv.org/abs/2608.12575). The implementation follows the [BBTools `ArithmeticVariableLogLog.java` reference](https://github.com/bbushnell/BBTools/blob/efe4f7b0317d34488a344045383a69c9af908b8d/current/cardinality/ArithmeticVariableLogLog.java) at commit `efe4f7b0317d34488a344045383a69c9af908b8d`, rather than its stale comments that mention the older history limit of 14. The executable code and current paper use a history limit of 13 and maximum relative exponent of 18.

The comparison baseline is the supplied `exaloglog-rs` library, which implements Otmar Ertl's [ExaLogLog: Space-Efficient and Practical Approximate Distinct Counting up to the Exa-Scale](https://arxiv.org/abs/2402.13726). Local benchmark measurements must be distinguished from the papers' published results.

## The two designs

| Property | AVLL in this crate | ExaLogLog baseline |
|---|---|---|
| Packed register storage | Eleven base-56 digits in each `u64` | Bit-packed registers of width `6+t+d` |
| Practical configuration | Fixed 56-state alphabet, 64/11 ≈ 5.82 bits/register | `t=2,d=24`: 32 bits/register |
| Registers at 1,024 payload bytes | 1,408 | 256 (`p=8`) |
| Registers at 2,048 payload bytes | 2,816 | 512 (`p=9`) |
| Per-register information | Relative leading-zero exponent and up to 2 history bits | Fractional rank refinement and `d` history bits |
| Global information | Shared sliding leading-zero floor, initially −1 | Absolute rank representation |
| Primary estimator | HLDLC: empirical blend of analytical estimators and fitted corrections | Maximum-likelihood equation solved numerically |
| Register update | Can reject hashes before register access; promotion occasionally scans all registers | Constant-time update independent of sketch size |
| Set-operation guarantees | Lossy floor changes and exponent clamping can introduce order and replay effects | Idempotent, commutative, associative merge and reducibility |
| Hash input in these Rust libraries | Prehashed `u64`, default xxh3-64 raw hashing, custom hasher adapters | The same API conventions |

Memory in the table counts the packed register array. AVLL also stores its floor and cached counters, and both implementations have their Rust object/allocator overhead. Equal payload bytes is useful for comparison, but should not be described as exactly equal total resident memory.

The AVLL paper reports 1.63% width-weighted mean absolute relative error at 1 KB, versus 1.71% for ELL(2,24), a 4.7% relative improvement. It reports approximately 3.4 empirical memory-variance product (MVP), compared with 3.78 for the practical ELL(2,24) configuration. Ertl's theoretical optimum is approximately 3.67 for ELL(2,20). These are claims under the papers' evaluation conditions, not guarantees for arbitrary streams, hash functions, or this implementation. A measured RMSE, mean absolute relative error, and bias are different metrics; one cannot substitute one for another without an error-distribution assumption. The AVLL paper's cache-pressure throughput experiment also differs from a single-sketch Rust microbenchmark.

## Packed representation

For eleven register states `r[i]` in `0..56`, a word is

```text
word = sum(r[i] * 56^i, i = 0..10)
r[i] = (word / 56^i) % 56
```

`56^11` fits within `u64`, so the representation requires unsigned arithmetic and rejects words greater than or equal to `56^11` when importing state. There are no fixed-width padding slots, although about 7.9% of possible 64-bit words are outside the valid base-56 domain. Saying that arithmetic packing uses all 64 bit positions does not mean every 64-bit word is valid.

| Relative exponent | State range | Encoded history |
|---|---|---|
| 0 | 0 | None |
| 1 | 1–2 | One bit |
| 2–13 | 3–50 | Two bits |
| 14–18 | 51–55 | None |

For exponent `e>=2`, history bit 1 describes an observation at exponent `e-1`, and history bit 0 describes an observation at `e-2`. Exponent 1 can retain only bit 1. State 0 represents the absence of an encoded observation above the current floor; after floor promotion this does not mean an empty bucket in the original stream.

`AvLogLog::new(requested_registers)` rounds the positive register request up to a multiple of eleven. For example, 2,048 requested registers means 2,057 actual registers and 1,496 payload bytes. `with_bytes` requires a positive multiple of eight and creates exactly that many payload bytes.

## Hash update and promotion

Given an already uniform 64-bit hash `h`, the bucket is `h % B`, with `B` the actual register count. Its absolute rank is `h.leading_zeros()`. The observation's relative bitmap position is

```text
position = leading_zeros(h) - (global_nlz + 1) + 2
```

The two extra positions retain history near the sliding floor. Negative positions cannot change the sketch and are rejected early. Normal updates OR the incoming observation into a small bitmap decoded from the existing state, then retain the highest exponent and the history allowed by the state alphabet. Relative exponents above 18 are clamped.

The sketch tracks how many registers have relative exponent at most 2. If that count reaches zero, it increments the shared floor and demotes every register by one tier, repeating if necessary. Demotion preserves the two relative history positions where representable; entering the history zone from exponent 14 supplies zero history because no history was stored there. Individual ordinary insertions allocate no memory. A floor promotion scans the packed registers, so worst-case insertion is proportional to the number of registers.

This library deliberately fixes two unsigned-shift edge cases in the Java reference:

* Very high ranks, including `h=0` with 64 leading zeroes, are clamped before any shift. The Java reference rejects initial bitmap positions of 64 or more, dropping hashes of ranks 62–64 in an empty sketch. The Rust API accepts every `u64` hash.
* When merge aligns floors separated by at least 64 tiers, the lower bitmap becomes zero. Rust must not shift by 64 or more, and Java's implicit masking of the shift distance does not represent the intended alignment.

The Rust raw-value path uses xxh3-64 to match the supplied ExaLogLog crate. The Java BBTools raw-number entry point uses a Thomas Wang integer finalizer plus its parent class's seed. Golden tests bypass raw hashing and supply identical `u64` hashes to both implementations. Use the same hash function and seed for streams that will be merged. A `u64` return type alone does not make an arbitrary application hash uniform.

## HLDLC estimation

The estimator summarizes relative registers into absolute rank counts and history patterns, accounting for the global floor, then computes

```text
HLDLC = 0.5 * LDLC + 0.5 * HybridPlus2
LDLC = blend(DlcSbs, HistoryCounting)
DlcSbs = blend(DynamicLinearCounting, StateBiasSum)
HybridPlus2 = blend(StateBiasSum, HistoryCorrectedMean)
```

* **State Bias Sum (SBS)** maps the rank/history pattern into one of eleven states. Cubic polynomials in an occupancy logarithm estimate the distinct count represented by each contributing bucket. This serves the low-cardinality range.
* **Dynamic Linear Counting (DLC)** computes linear-counting estimates at several rank thresholds and combines reliable tiers using inverse-error weighting in logarithmic space. It supplies the backbone and the cardinality reference for blend decisions.
* **History Counting (HC)** uses history-bit occupancy in neighboring rank tiers, combines the resulting estimates, and applies the reference correction formula. It contributes when sufficient buckets support the estimate.
* **History-Corrected Mean (Mean+H)** adjusts the harmonic-mean contribution of each bucket with history-state offsets and fitted correction factors. It serves the middle and high ranges.

HLDLC has no root solver, but it is not free of transcendental operations or empirical constants. Its formulas use logarithms, exponentials, powers, and correction functions. Degenerate synthetic states in which every register has exactly the same rank can leave DLC without a usable tier; the reference may then select its low-range SBS fallback even at a high floor. A large represented exponent therefore does not guarantee a meaningful estimate for arbitrary hand-constructed states. The reference's fitted coefficients are retained. Java used some single-precision intermediates; the golden tests allow a small floating-point tolerance while requiring identical packed words and floor on the compatible paths. `estimate()` returns a floating-point value and is not rounded or clamped to a total number of insertions as the Java parent cardinality API optionally does.

## Lossiness, duplicates, ordering, and merging

AVLL should not be advertised as an exact set-union semilattice. The AVLL paper itself acknowledges non-idempotency from its shared-floor architecture. It also reports no practically observed accuracy degradation under its duplicate-heavy workloads; this empirical observation is compatible with synthetic counterexamples.

A concrete history-loss example uses eleven buckets. First insert a rank-12 event in bucket 0, which has relative exponent 14 at floor −1 and stores no history. Then insert a rank-11 event in that same bucket; the event cannot be retained in the exponent-only state. Insert rank-1 events in each of the ten other buckets. This advances the floor to 0 and demotes bucket 0 to relative exponent 13 with history `00`. Replaying the already-seen rank-11 event now sets history to `10`, changing the state. Moving that rank-11 event from before promotion to after promotion also changes the result. A regression test records this limitation explicitly.

Clamping also loses the exact maximum of unusually high-rank events. Later promotion cannot recover the unclamped value. Replaying such an event can therefore raise a demoted register again. Merging aligns floors and ORs the *retained* information; it cannot recreate discarded history or unclamped maxima. Different partitioning, promotion schedules, and merge trees can produce different states. Fixed-floor tests establish duplicate, order, and merge behavior where the information remains representable, and the high-exponent regression prevents those limited tests from being mistaken for universal guarantees.

Merges require identical actual register counts and compatible hash functions/seeds. A self-merge of a state is unchanged, but that fact does not establish general insertion idempotency, associativity across independent promotion histories, or exact equivalence to a single stream. ExaLogLog preserves these stronger properties and supports precision reduction; this AVLL implementation does not promise a downsampling operation.

## Range and evidence

This is a **64-bit hash** implementation. The sliding floor avoids a fixed absolute exponent-18 ceiling, but it does not create information beyond the 64-bit hash space. At extreme cardinalities, hash collisions and dependence between the same hash's bucket and rank become material. The paper's discussion of 256-bit hashes describes a possible wider-hash architecture, not an implemented feature of this API.

Real streams can directly test zero, the low range, the estimator blends, floor promotions, and the large but feasible range. Testing every possible cardinality up to `2^64` by insertion is infeasible. Synthetic rank/floor fixtures exercise numeric and representation limits; any conditional or Poisson simulations at enormous cardinalities must be labeled as simulations rather than insertion throughput or proof of exact behavior for real streams.

The test suite includes all seven hash choices used in the supplied ExaLogLog tests (xxh3, aHash, Komihash, PolymurHash, WyHash, t1ha2-atonce, tabulation), importing and malformed state checks, serde round trips with continuation, fixed-floor unions, high-rank and wide-shift cases, a deterministic cardinality smoke test, and Java-produced packed-state/HLDLC golden vectors. The Java oracle is mechanically extracted from the pinned reference; parent-class and `CardStats` integration are removed, and its input accepts precomputed hashes. The fitted HLDLC pipeline remains intact.

## Attribution

The algorithm descriptions above summarize the two linked papers. The BBTools state mapping, update logic, and estimator coefficients are adapted under its BSD-style license; retain the upstream copyright, conditions, and disclaimer supplied with this package. The test oracle is an adapted upstream work and carries the same attribution. No claim is made that AVLL's source license or algorithmic guarantees are identical to ExaLogLog's reference implementation; the benchmark uses the user's supplied Rust ExaLogLog implementation.
