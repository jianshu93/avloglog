//! Arithmetic Variable LogLog (AVLL), with eleven base-56 registers per `u64`.
//!
//! The API follows the accompanying ExaLogLog Rust library. Supply uniform 64-bit
//! hashes with [`AvLogLog::add_hash`], or use the default xxh3 hasher via
//! [`AvLogLog::add_raw`]. Estimation uses the paper's HLDLC blend.
//!
//! ```
//! use avloglog::AvLogLog;
//! let mut sketch = AvLogLog::with_bytes(1024)?;
//! sketch.add_raw_values(["apple", "banana", "apple"]);
//! assert!((sketch.estimate() - 2.0).abs() < 0.1);
//! # Ok::<(), avloglog::AvLogLogError>(())
//! ```
//!
//! Rare high ranks lose history, so AVLL does not provide ExaLogLog's exact
//! insertion-order, idempotency, or merge-associativity guarantees. Merging
//! requires equal register counts; there is no lossless precision reduction.
//! See `docs/algorithm.md` for the algorithm, calibration domain, and caveats.

mod estimator;

use std::error::Error;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use xxhash_rust::xxh3::Xxh3;

pub const REGISTERS_PER_WORD: usize = 11;
pub const RADIX: u64 = 56;
pub const MAX_EXPONENT: u32 = 18;
const HISTORY_LIMIT: u32 = 13;
const HISTORY_MARGIN: u32 = 2;
const WORD_BOUND: u64 = 16_985_107_389_382_393_856; // 56^11
const MAX_WORDS: usize = i32::MAX as usize / REGISTERS_PER_WORD;
const POW: [u64; REGISTERS_PER_WORD] = {
    let mut values = [1; REGISTERS_PER_WORD];
    let mut i = 1;
    while i < REGISTERS_PER_WORD {
        values[i] = values[i - 1] * RADIX;
        i += 1;
    }
    values
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvLogLogError {
    InvalidRegisterCount,
    InvalidByteCount,
    InvalidGlobalNlz,
    InvalidPackedWord,
    IncompatibleSize,
    AllocationFailed,
}

impl fmt::Display for AvLogLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRegisterCount => "register count must be positive and fit in i32",
            Self::InvalidByteCount => "byte count must be a positive multiple of eight",
            Self::InvalidGlobalNlz => "global NLZ floor must be in -1..=64",
            Self::InvalidPackedWord => "packed word must be less than 56^11",
            Self::IncompatibleSize => "sketches must have the same number of registers",
            Self::AllocationFailed => "unable to allocate register storage",
        })
    }
}
impl Error for AvLogLogError {}

/// An AVLL sketch with a shared rank floor and eleven registers per packed word.
///
/// Hash selection follows the Java reference: `hash % register_count`, with
/// rank `hash.leading_zeros()`. Use the same hashing configuration for all
/// sketches that will be merged. No hash seed or hasher is stored in the sketch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvLogLog {
    global_nlz: i32,
    floor_count: usize,
    state: Vec<u64>,
}

impl Default for AvLogLog {
    fn default() -> Self {
        Self::with_bytes(1024).expect("default sketch allocation")
    }
}

impl AvLogLog {
    /// Creates at least `registers` registers, rounded up to a multiple of 11.
    pub fn new(registers: usize) -> Result<Self, AvLogLogError> {
        let words = registers
            .checked_add(REGISTERS_PER_WORD - 1)
            .ok_or(AvLogLogError::InvalidRegisterCount)?
            / REGISTERS_PER_WORD;
        if words == 0 || words > MAX_WORDS {
            return Err(AvLogLogError::InvalidRegisterCount);
        }
        let mut state = Vec::new();
        state
            .try_reserve_exact(words)
            .map_err(|_| AvLogLogError::AllocationFailed)?;
        state.resize(words, 0);
        Ok(Self {
            global_nlz: -1,
            floor_count: words * REGISTERS_PER_WORD,
            state,
        })
    }

    /// Creates exactly `bytes` bytes of register storage (excluding object metadata).
    pub fn with_bytes(bytes: usize) -> Result<Self, AvLogLogError> {
        if bytes == 0 || bytes % 8 != 0 {
            return Err(AvLogLogError::InvalidByteCount);
        }
        let registers = (bytes / 8)
            .checked_mul(REGISTERS_PER_WORD)
            .ok_or(AvLogLogError::InvalidRegisterCount)?;
        Self::new(registers)
    }

    /// Restores packed words and the shared floor. Floor counts are recomputed.
    ///
    /// All words must be below `56^11`. If the floor zone is full, normalizes
    /// by promoting the floor, just as insertion and merging do.
    pub fn wrap(global_nlz: i32, state: Vec<u64>) -> Result<Self, AvLogLogError> {
        if !(-1..=64).contains(&global_nlz) {
            return Err(AvLogLogError::InvalidGlobalNlz);
        }
        if state.is_empty() || state.len() > MAX_WORDS {
            return Err(AvLogLogError::InvalidRegisterCount);
        }
        if state.iter().any(|&word| word >= WORD_BOUND) {
            return Err(AvLogLogError::InvalidPackedWord);
        }
        let mut sketch = Self {
            global_nlz,
            floor_count: 0,
            state,
        };
        sketch.floor_count = (0..sketch.num_registers())
            .filter(|&i| state_to_exp(sketch.get_register(i)) <= HISTORY_MARGIN)
            .count();
        sketch.normalize();
        Ok(sketch)
    }

    /// The shared floor, initially -1, matching the companion Java implementation.
    pub fn global_nlz(&self) -> i32 {
        self.global_nlz
    }
    pub fn num_registers(&self) -> usize {
        self.state.len() * REGISTERS_PER_WORD
    }
    pub fn state(&self) -> &[u64] {
        &self.state
    }
    pub fn into_state(self) -> Vec<u64> {
        self.state
    }
    pub fn memory_bytes(&self) -> usize {
        self.state.len() * 8
    }
    pub fn reset(&mut self) -> &mut Self {
        self.state.fill(0);
        self.global_nlz = -1;
        self.floor_count = self.num_registers();
        self
    }

    pub fn hash_value<T: Hash>(value: T) -> u64 {
        let mut hasher = Xxh3::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[allow(clippy::manual_hash_one)]
    pub fn hash_value_with_build_hasher<T: Hash, S: BuildHasher + ?Sized>(
        value: T,
        build_hasher: &S,
    ) -> u64 {
        let mut hasher = build_hasher.build_hasher();
        value.hash(&mut hasher);
        hasher.finish()
    }

    /// Inserts a uniformly distributed 64-bit hash, including zero and `u64::MAX`.
    /// Most low ranks return before accessing packed storage once the floor rises.
    pub fn add_hash(&mut self, hash_value: u64) -> &mut Self {
        let bit_pos = hash_value.leading_zeros() as i32 - self.global_nlz + 1;
        if bit_pos < 0 {
            return self;
        }
        let idx = (hash_value % self.num_registers() as u64) as usize;
        let old = self.get_register(idx);
        // Clamp before shifting, including the all-zero hash (NLZ=64).
        let new = if bit_pos as u32 > HISTORY_LIMIT {
            to_state((bit_pos as u32).min(MAX_EXPONENT), 0)
        } else {
            pack_register(unpack_register(old) | (1u64 << bit_pos))
        };
        if new <= old {
            return self;
        }
        self.set_register(idx, old, new);
        if state_to_exp(old) <= HISTORY_MARGIN && state_to_exp(new) > HISTORY_MARGIN {
            self.floor_count -= 1;
            self.normalize();
        }
        self
    }

    pub fn add(&mut self, hash_value: u64) -> &mut Self {
        self.add_hash(hash_value)
    }
    pub fn add_hashes<I: IntoIterator<Item = u64>>(&mut self, values: I) -> &mut Self {
        for value in values {
            self.add_hash(value);
        }
        self
    }
    pub fn add_raw<T: Hash>(&mut self, value: T) -> &mut Self {
        self.add_hash(Self::hash_value(value))
    }
    pub fn add_raw_values<I, T>(&mut self, values: I) -> &mut Self
    where
        I: IntoIterator<Item = T>,
        T: Hash,
    {
        for value in values {
            self.add_raw(value);
        }
        self
    }
    pub fn add_raw_with_build_hasher<T: Hash, S: BuildHasher + ?Sized>(
        &mut self,
        value: T,
        build_hasher: &S,
    ) -> &mut Self {
        self.add_hash(Self::hash_value_with_build_hasher(value, build_hasher))
    }
    pub fn add_raw_values_with_build_hasher<I, T, S>(
        &mut self,
        values: I,
        build_hasher: &S,
    ) -> &mut Self
    where
        I: IntoIterator<Item = T>,
        T: Hash,
        S: BuildHasher + ?Sized,
    {
        for value in values {
            self.add_raw_with_build_hasher(value, build_hasher);
        }
        self
    }
    pub fn add_raw_with_hash_fn<T, F: FnOnce(&T) -> u64>(
        &mut self,
        value: T,
        hash_fn: F,
    ) -> &mut Self {
        self.add_hash(hash_fn(&value))
    }
    pub fn add_raw_values_with_hash_fn<I, T, F>(&mut self, values: I, mut hash_fn: F) -> &mut Self
    where
        I: IntoIterator<Item = T>,
        F: FnMut(&T) -> u64,
    {
        for value in values {
            self.add_hash(hash_fn(&value));
        }
        self
    }

    /// Returns the paper's calibrated HLDLC estimate.
    ///
    /// Calibration is empirical. Near the finite 64-bit hash ceiling, estimates
    /// are not collision-corrected; consult the full-range benchmark results.
    pub fn estimate(&self) -> f64 {
        let mut histogram = [0u32; RADIX as usize];
        for &word in &self.state {
            let mut remainder = word;
            for _ in 0..REGISTERS_PER_WORD {
                histogram[(remainder % RADIX) as usize] += 1;
                remainder /= RADIX;
            }
        }
        estimator::estimate(&histogram, self.global_nlz, self.num_registers())
    }
    pub fn get_distinct_count_estimate(&self) -> f64 {
        self.estimate()
    }

    /// Merges equal-size sketches by aligning their floors and combining history.
    /// Rare dropped history means different merge trees can yield different states.
    pub fn add_sketch(&mut self, other: &Self) -> Result<&mut Self, AvLogLogError> {
        if self.state.len() != other.state.len() {
            return Err(AvLogLogError::IncompatibleSize);
        }
        let global = self.global_nlz.max(other.global_nlz);
        let delta_a = (global - self.global_nlz) as u32;
        let delta_b = (global - other.global_nlz) as u32;
        self.floor_count = 0;
        // Decode sequential digits with constant division by 56. Repeated
        // random-access division by 56^position is substantially more expensive.
        for (word_a, &word_b) in self.state.iter_mut().zip(&other.state) {
            let (mut a, mut b, mut merged) = (*word_a, word_b, 0);
            for power in POW {
                let bitmap_a = unpack_register((a % RADIX) as u32)
                    .checked_shr(delta_a)
                    .unwrap_or(0);
                let bitmap_b = unpack_register((b % RADIX) as u32)
                    .checked_shr(delta_b)
                    .unwrap_or(0);
                let new = pack_register(bitmap_a | bitmap_b);
                merged += u64::from(new) * power;
                self.floor_count += usize::from(state_to_exp(new) <= HISTORY_MARGIN);
                a /= RADIX;
                b /= RADIX;
            }
            *word_a = merged;
        }
        self.global_nlz = global;
        self.normalize();
        Ok(self)
    }

    pub fn merge(first: &Self, second: &Self) -> Result<Self, AvLogLogError> {
        let mut merged = first.clone();
        merged.add_sketch(second)?;
        Ok(merged)
    }

    #[inline]
    fn get_register(&self, idx: usize) -> u32 {
        ((self.state[idx / REGISTERS_PER_WORD] / POW[idx % REGISTERS_PER_WORD]) % RADIX) as u32
    }
    #[inline]
    fn set_register(&mut self, idx: usize, old: u32, new: u32) {
        let word = &mut self.state[idx / REGISTERS_PER_WORD];
        let power = POW[idx % REGISTERS_PER_WORD];
        if new >= old {
            *word += u64::from(new - old) * power;
        } else {
            *word -= u64::from(old - new) * power;
        }
    }
    fn normalize(&mut self) {
        while self.floor_count == 0 && self.global_nlz < 64 {
            self.global_nlz += 1;
            for word in &mut self.state {
                let (mut remaining, mut demoted) = (*word, 0);
                for power in POW {
                    let old = (remaining % RADIX) as u32;
                    let exp = state_to_exp(old);
                    let new = if exp > HISTORY_LIMIT + 1 {
                        old - 1
                    } else if exp == HISTORY_LIMIT + 1 {
                        to_state(exp - 1, 0)
                    } else if exp >= 3 {
                        old - 4
                    } else if exp == 2 {
                        to_state(1, state_to_hist(old))
                    } else {
                        0
                    };
                    demoted += u64::from(new) * power;
                    self.floor_count += usize::from(state_to_exp(new) <= HISTORY_MARGIN);
                    remaining /= RADIX;
                }
                *word = demoted;
            }
        }
    }

    #[cfg(feature = "serde")]
    pub fn save<W: std::io::Write>(&self, writer: W) -> std::io::Result<()> {
        bincode::serialize_into(writer, self).map_err(std::io::Error::other)
    }
    #[cfg(feature = "serde")]
    pub fn load<R: std::io::Read>(reader: R) -> std::io::Result<Self> {
        bincode::deserialize_from(reader)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

pub(crate) fn state_to_exp(state: u32) -> u32 {
    match state {
        0 => 0,
        1..=2 => 1,
        3..=50 => (state - 3) / 4 + 2,
        _ => 14 + state - 51,
    }
}
pub(crate) fn state_to_hist(state: u32) -> u32 {
    match state {
        0 => 0,
        1..=2 => (state - 1) << 1,
        3..=50 => (state - 3) & 3,
        _ => 0,
    }
}
fn to_state(exp: u32, hist: u32) -> u32 {
    match exp {
        0 => 0,
        1 => 1 + (hist >> 1),
        2..=13 => 4 * exp - 5 + hist,
        _ => 51 + exp - 14,
    }
}
fn unpack_register(state: u32) -> u64 {
    if state == 0 {
        return 0;
    }
    let exp = state_to_exp(state);
    let hist = state_to_hist(state);
    let mut bitmap = 1u64 << exp;
    if exp >= 2 {
        bitmap |= u64::from(hist & 1) << (exp - 2);
    }
    if exp >= 1 {
        bitmap |= u64::from(hist >> 1) << (exp - 1);
    }
    bitmap
}
fn pack_register(bitmap: u64) -> u32 {
    if bitmap == 0 {
        return 0;
    }
    let exp = 63 - bitmap.leading_zeros();
    if exp > HISTORY_LIMIT {
        return to_state(exp.min(MAX_EXPONENT), 0);
    }
    let hist = if exp >= 2 {
        ((bitmap >> (exp - 2)) & 3) as u32
    } else if exp == 1 {
        ((bitmap & 1) << 1) as u32
    } else {
        0
    };
    to_state(exp, hist)
}

#[cfg(feature = "serde")]
mod serialization {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wire {
        version: u32,
        global_nlz: i32,
        state: Vec<u64>,
    }
    impl Serialize for AvLogLog {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            // Serialize a borrowed representation to avoid copying packed storage.
            #[derive(Serialize)]
            struct Borrowed<'a> {
                version: u32,
                global_nlz: i32,
                state: &'a [u64],
            }
            Borrowed {
                version: 1,
                global_nlz: self.global_nlz,
                state: &self.state,
            }
            .serialize(serializer)
        }
    }
    impl<'de> Deserialize<'de> for AvLogLog {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let wire = Wire::deserialize(deserializer)?;
            if wire.version != 1 {
                return Err(serde::de::Error::custom("unsupported AVLL format version"));
            }
            Self::wrap(wire.global_nlz, wire.state).map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_maps_and_bitmap_roundtrip() {
        for state in 0..56 {
            assert_eq!(pack_register(unpack_register(state)), state);
            assert!(state_to_exp(state) <= MAX_EXPONENT);
        }
    }
    #[test]
    fn packed_words_use_unsigned_full_width() {
        let mut sketch = AvLogLog::new(22).unwrap();
        for i in 0..22 {
            sketch.set_register(i, 0, 55);
        }
        assert_eq!(sketch.state, vec![WORD_BOUND - 1; 2]);
        for i in 0..22 {
            assert_eq!(sketch.get_register(i), 55);
        }
        for i in 0..22 {
            sketch.set_register(i, 55, (i % 56) as u32);
        }
        for i in 0..22 {
            assert_eq!(sketch.get_register(i), i as u32);
        }
    }
}
