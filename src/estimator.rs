//! HLDLC estimation from the 56-state register histogram.
//!
//! This ports the primary pipeline and published correction constants in BBTools'
//! `ArithmeticVariableLogLog` (May 2026): SBS, DLC, HC, Mean+H, and the two
//! branches LDLC and Hybrid+2. Constants are inherited from that implementation;
//! no correction factors are fitted at runtime. Histogram aggregation avoids
//! materializing the Java implementation's per-register temporary arrays.
//!
//! `global_nlz` uses the reference's signed convention: -1 initially, and 0..64
//! after promotion. A zero register is empty only at -1; after promotion it
//! contributes to the floor tier in DLC, but has no observable history bits.

// Retain the reference's published decimal coefficients verbatim.
#![allow(clippy::excessive_precision)]

use std::f64::consts::{LN_2, PI};

const HISTORY_MARGIN: i32 = 2;
const HISTORY_TIER_LIMIT: usize = 13;
const HISTORY_STATES: usize = 4 * HISTORY_TIER_LIMIT - 1;
const TERMINAL_MEANH_CF: f32 = 0.855563;
const SBS_BASE: [f64; 11] = [1.0, 1.0, 2.0, 1.0, 2.0, 2.0, 3.0, 1.0, 2.0, 2.0, 3.0];
const SBS_A: [f64; 11] = [
    0.22105100, 0.12252876, 0.36952900, 0.06133797, 0.30751800, 0.18526204, 0.43143072, 0.67720200,
    0.70123350, 0.64688227, 0.73638900,
];
const SBS_B: [f64; 11] = [
    0.03969600, 0.00756061, 0.03269600, 0.00236917, 0.02818300, 0.00912048, 0.03450029, 0.03911000,
    0.01746612, 0.01820189, 0.01219500,
];
const SBS_C: [f64; 11] = [
    -0.00321100,
    -0.00065621,
    -0.00192200,
    -0.00029760,
    -0.00176600,
    -0.00068916,
    -0.00183925,
    -0.00248900,
    -0.00112447,
    -0.00079415,
    -0.00085800,
];
const MCF_MEANH: [f64; 16] = [
    -3.691071292933851,
    -0.363637261057505,
    2.567263769729832,
    1.066355956682916,
    4.999999754294653,
    -11.189952323115953,
    0.785742127708774,
    0.054343414450697,
    -8.335873985296551,
    1.101742014940036,
    -0.143358513726359,
    -0.794735788647950,
    2.755625941876753,
    -0.187952399977993,
    0.812771424938221,
    2.145566972133455,
];
const HIST_TIERS: [[f64; 4]; 3] = [
    [0.0, 0.0, 0.0, 0.0],
    [-1.64516934, 0.0, 0.32902894, 0.0],
    [-2.91470444, -1.11691781, -1.63200820, 0.37491432],
];
const HIST_STEADY: [f64; 4] = [-2.45693840, -1.03241245, -1.48290584, 0.33548548];

#[inline]
fn state_to_exp(state: usize) -> usize {
    match state {
        0 => 0,
        1..=2 => 1,
        3..HISTORY_STATES => (state - 3) / 4 + 2,
        _ => HISTORY_TIER_LIMIT + 1 + state - HISTORY_STATES,
    }
}

#[inline]
fn state_to_hist(state: usize) -> usize {
    match state {
        0 => 0,
        1..=2 => (state - 1) << 1,
        3..HISTORY_STATES => (state - 3) & 3,
        _ => 0,
    }
}

/// Compute HLDLC with the reference implementation's signed floor convention.
///
/// The histogram must sum to `buckets`. All 56 entries represent valid packed
/// states; history for the five exponent-only states is unavailable, just as in
/// the reference implementation. This estimator retains the paper's finite-hash
/// limitations and does not invent a large-cardinality collision correction.
pub(crate) fn estimate(histogram: &[u32; 56], global_nlz: i32, buckets: usize) -> f64 {
    debug_assert!(buckets > 0);
    debug_assert!((-1..=64).contains(&global_nlz));
    debug_assert_eq!(
        histogram.iter().map(|&n| u64::from(n)).sum::<u64>(),
        buckets as u64
    );
    let b = buckets as f64;
    // counts[e + 1] includes every register whose absolute NLZ is e. Zero
    // registers after promotion participate in DLC but not history estimators.
    let mut counts = [0_u64; 66];
    let mut observed = [[0_u64; 4]; 64];
    let mut filled = 0_u64;
    for (state, &count) in histogram.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let n = u64::from(count);
        let exp = state_to_exp(state) as i32;
        let abs_nlz = if state == 0 || exp < HISTORY_MARGIN {
            global_nlz
        } else {
            exp - HISTORY_MARGIN + global_nlz + 1
        };
        if (0..64).contains(&abs_nlz) {
            counts[abs_nlz as usize + 1] += n;
            filled += n;
            if state != 0 {
                observed[abs_nlz as usize][state_to_hist(state)] += n;
            }
        }
    }
    let empty = buckets as u64 - filled;
    counts[0] = empty;
    let lc_raw = b * (b / (empty as f64).max(0.5)).ln();

    // SBS counts only structurally valid history patterns, as in the paper.
    let mut sbs_counts = [0_u64; 11];
    let mut contributing = 0_u64;
    for (nlz, histories) in observed.iter().enumerate() {
        for (hist, &n) in histories.iter().enumerate() {
            if let Some(si) = sbs_state_index(nlz, hist) {
                sbs_counts[si] += n;
                contributing += n;
            }
        }
    }
    let sbs = if contributing == 0 {
        lc_raw
    } else {
        let l = (b / (b - contributing as f64).max(0.5)).ln();
        sbs_counts
            .iter()
            .enumerate()
            .map(|(si, &n)| {
                n as f64
                    * (SBS_BASE[si] + SBS_A[si] * l + SBS_B[si] * l * l + SBS_C[si] * l * l * l)
            })
            .sum::<f64>()
    }
    .max(filled as f64);

    let dlc_pure = dlc_pure(&counts, empty, buckets);
    let lc_min = lc_min(&counts, empty, b);
    let dlc_raw = dlc_blend_with_lc_min(dlc_pure, lc_min, empty, buckets);

    // Mean+H. Preserve the reference's f32 terminal constant and occupancy
    // correction: its numeric values are part of the published calibration.
    let div = filled.max(1) as f64;
    let correction = ((filled + buckets as u64) as f32 / (2 * buckets) as f32) as f64;
    let inv_terminal_cf = 1.0 / f64::from(TERMINAL_MEANH_CF);
    let mut corrected_dif_sum = 0.0;
    for (nlz, histories) in observed.iter().enumerate() {
        let dif = 2.0_f64.powi(63 - nlz as i32);
        for (hist, &n) in histories.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let offset = if nlz < HIST_TIERS.len() {
                HIST_TIERS[nlz][hist]
            } else {
                HIST_STEADY[hist]
            };
            let tier_multiplier = 2.0_f64.powf(-offset) * inv_terminal_cf;
            corrected_dif_sum += n as f64 * dif * tier_multiplier;
        }
    }
    let mean_h_raw =
        2.0 * (i64::MAX as f64 / (corrected_dif_sum / div).max(1.0)) * div * correction;
    let mean_h = mean_h_raw * iterative_cf(dlc_raw, mean_h_raw, b, &MCF_MEANH);

    let dlc_sbs = if dlc_pure.is_nan() || dlc_raw <= 2.0 * b {
        sbs
    } else if dlc_raw >= 6.0 * b {
        dlc_pure
    } else {
        let t = (dlc_raw / (2.0 * b)).ln() / 3.0_f64.ln();
        (1.0 - t) * sbs + t * dlc_pure
    };
    let hc = history_counting(&observed, b);
    let ldlc = if dlc_sbs <= 0.5 * b || !(hc > 0.0 && dlc_sbs > 0.0) {
        dlc_sbs
    } else {
        let hc_weight = if dlc_sbs <= 4.5 * b {
            ((dlc_sbs - 0.5 * b) / (4.0 * b)) * 0.5
        } else {
            0.5
        };
        (1.0 - hc_weight) * dlc_sbs + hc_weight * hc
    };
    let hybrid_plus_2 = if dlc_raw <= b {
        sbs
    } else if dlc_raw < 6.0 * b {
        let t = (dlc_raw / b).ln() / 6.0_f64.ln();
        (1.0 - t) * sbs + t * mean_h
    } else {
        mean_h
    };
    0.5 * ldlc + 0.5 * hybrid_plus_2
}

fn sbs_state_index(nlz: usize, history: usize) -> Option<usize> {
    let bin = nlz.min(3);
    let invalid_bits = 2 - bin.min(2);
    if history & ((1 << invalid_bits) - 1) != 0 {
        return None;
    }
    Some((1 << bin) - 1 + (history >> invalid_bits))
}

fn mean_cf_formula(card_per_bucket: f64, p: &[f64; 16]) -> f64 {
    if card_per_bucket <= 0.0 {
        return p[0] + p[1] + p[4] + p[7];
    }
    let x = card_per_bucket.ln() / LN_2;
    let s1 = (1.0 + ((x - p[2]) / p[3]).tanh()) * 0.5;
    let s2 = (1.0 + ((x - p[5]) / p[6]).tanh()) * 0.5;
    let s3 = (1.0 + ((x - p[8]) / p[9]).tanh()) * 0.5;
    let d1 = (x - p[11]) / p[12];
    let d2 = (x - p[14]) / p[15];
    p[0] + p[1] * s1 + p[4] * s2 + p[7] * s3 + p[10] * (-d1 * d1).exp() + p[13] * (-d2 * d2).exp()
}

fn iterative_cf(seed: f64, raw: f64, b: f64, p: &[f64; 16]) -> f64 {
    let mut cf = mean_cf_formula(seed / b, p);
    if seed > 10.0 * b {
        let best_estimate = cf * raw;
        if best_estimate > 0.0 {
            let next = mean_cf_formula(best_estimate / b, p);
            if (next - cf).abs() > 1e-6 {
                cf = next;
            }
        }
    }
    cf
}

fn lc_min(counts: &[u64; 66], empty: u64, b: f64) -> f64 {
    if empty > 0 {
        return b * (b / (empty as f64).max(0.5)).ln();
    }
    let mut unseen = empty;
    for (tier, &n) in counts[1..].iter().enumerate() {
        unseen += n;
        if unseen > 0 {
            return 2.0_f64.powi(tier as i32 + 1) * b * (b / (unseen as f64).max(0.5)).ln();
        }
    }
    0.0
}

fn dlc_pure(counts: &[u64; 66], empty: u64, buckets: usize) -> f64 {
    let min_unseen = 2_u64.max((buckets as f64 * 0.002) as u64);
    let max_unseen = (buckets as u64).saturating_sub(min_unseen);
    let b = buckets as f64;
    let start_tier = counts[1..].iter().position(|&n| n > 0).unwrap_or(0);
    let mut unseen = empty;
    let mut sum_weights = 0.0;
    let mut sum_weighted_logs = 0.0;
    for tier in start_tier..64 {
        if unseen >= min_unseen && unseen <= max_unseen {
            let log_occupancy = (b / unseen as f64).ln();
            let est = 2.0_f64.powi(tier as i32) * b * log_occupancy;
            let occupied = b - unseen as f64;
            let err = (2.0 / PI).sqrt() * (occupied / (b * unseen as f64)).sqrt() / log_occupancy;
            let weight = (1.0 / err).powf(4.5);
            sum_weights += weight;
            sum_weighted_logs += weight * est.ln();
        }
        if tier < 63 {
            unseen += counts[tier + 1];
        }
        if unseen >= buckets as u64 {
            break;
        }
    }
    if sum_weights <= 0.0 {
        f64::NAN
    } else {
        (sum_weighted_logs / sum_weights).exp()
    }
}

fn dlc_blend_with_lc_min(dlc: f64, lc_min: f64, empty: u64, buckets: usize) -> f64 {
    // Java declares these constants as float and multiplies by an int before
    // promotion to double. Reproduce those rounding semantics as well.
    const LOW: f32 = 0.088;
    const HIGH: f32 = 0.952;
    let lower = f64::from(LOW * buckets as f32);
    let upper = f64::from(HIGH * buckets as f32);
    if dlc.is_nan() || empty as f64 >= upper {
        lc_min
    } else if empty as f64 > lower {
        let t = (empty as f64 - lower) / f64::from((HIGH - LOW) * buckets as f32);
        t * lc_min + (1.0 - t) * dlc
    } else {
        dlc
    }
}

fn history_counting(observed: &[[u64; 4]; 64], b: f64) -> f64 {
    let mut counts = [0_u64; 64];
    let mut history_set = [[0_u64; 64]; 2];
    for (nlz, histories) in observed.iter().enumerate() {
        for (hist, &n) in histories.iter().enumerate() {
            counts[nlz] += n;
            if hist & 2 != 0 {
                history_set[0][nlz] += n;
            }
            if hist & 1 != 0 {
                history_set[1][nlz] += n;
            }
        }
    }
    let max_nlz = counts.iter().rposition(|&n| n > 0).unwrap_or(0);
    let max_tier = (max_nlz + 2).min(63);
    let mut sum_weights = 0.0;
    let mut sum_weighted_logs = 0.0;
    for tier in 0..=max_tier {
        let mut effective = 0_u64;
        let mut unseen = 0_u64;
        for (depth, bit_set) in history_set.iter().enumerate() {
            let source_tier = tier + depth + 1;
            if source_tier < 64 {
                effective += counts[source_tier];
                unseen += counts[source_tier] - bit_set[source_tier];
            }
        }
        if effective >= 8 && unseen >= 1 && unseen < effective {
            let log_occupancy = (effective as f64 / unseen as f64).ln();
            let est = 2.0_f64.powi(tier as i32 + 1) * b * log_occupancy;
            if est > 0.0 && !est.is_nan() {
                let occupied = effective - unseen;
                let err = (2.0 / PI).sqrt()
                    * (occupied as f64 / (effective as f64 * unseen as f64)).sqrt()
                    / log_occupancy;
                let weight = (1.0 / err).powi(2);
                sum_weights += weight;
                sum_weighted_logs += weight * est.ln();
            }
        }
    }
    if sum_weights > 0.0 {
        let raw = (sum_weighted_logs / sum_weights).exp();
        if raw > 0.0 {
            raw * hc_cf_formula(raw)
        } else {
            0.0
        }
    } else {
        0.0
    }
}

fn hc_cf_formula(card: f64) -> f64 {
    const TERMINAL: f64 = 0.998484777537405;
    if card <= 0.0 {
        return TERMINAL;
    }
    let x = card.ln() / LN_2;
    let base = TERMINAL - 562.709161557896778 * (-1.011821147415617 * x).exp();
    let angle = 2.0 * PI * x;
    base + (0.000344111534286 - 0.000037079356918 * x) * angle.sin()
        + (0.000424944596363 - 0.000035494763272 * x) * angle.cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_java_fixture(counts: &[u32], global: i32, expected: f64) {
        let mut histogram = [0_u32; 56];
        histogram[..counts.len()].copy_from_slice(counts);
        let buckets = counts.iter().map(|&n| n as usize).sum();
        let actual = estimate(&histogram, global, buckets);
        let tolerance = expected.abs().max(1.0) * 2e-13;
        assert!(
            (actual - expected).abs() <= tolerance,
            "global={global}, expected={expected:.17e}, actual={actual:.17e}"
        );
    }

    #[test]
    fn java_golden_estimates() {
        // Expected values were independently produced by javac from the
        // unmodified BBTools estimate() and helper methods. These exercise the
        // empty floor, low-cardinality SBS, transition, promoted floor, and all
        // 56 register states (including the history-free upper exponents).
        check_java_fixture(&[1408], -1, 0.0);
        check_java_fixture(&[1407, 0, 0, 1], -1, 1.0001570722635007);
        check_java_fixture(&[0, 0, 0, 1408], -1, 5140.8268691478);
        check_java_fixture(
            &[
                700, 1, 2, 50, 25, 10, 15, 70, 55, 20, 45, 90, 60, 75, 100, 40, 30, 10, 5, 5,
            ],
            -1,
            1069.6246597827094,
        );
        check_java_fixture(
            &[
                11, 20, 22, 25, 29, 36, 38, 44, 48, 55, 63, 68, 75, 83, 90, 98, 92, 85, 75, 65, 56,
                48, 42, 38, 33, 28, 22, 17, 12, 9, 7, 5, 4, 3, 2, 1,
            ],
            5,
            256886.87534127763,
        );
        check_java_fixture(
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 11],
            12,
            37.327723902788634,
        );
        let mut all_states = [0_u32; 56];
        all_states[0] = 30;
        for (state, count) in all_states.iter_mut().enumerate().skip(1) {
            *count = state as u32;
        }
        check_java_fixture(&all_states, 40, 1.4813559519561357e17);
    }

    #[test]
    fn sbs_history_slots_must_be_valid() {
        assert_eq!(sbs_state_index(0, 0), Some(0));
        for hist in 1..4 {
            assert_eq!(sbs_state_index(0, hist), None);
        }
        assert_eq!(sbs_state_index(1, 0), Some(1));
        assert_eq!(sbs_state_index(1, 2), Some(2));
        assert_eq!(sbs_state_index(1, 1), None);
        assert_eq!(sbs_state_index(1, 3), None);
        for hist in 0..4 {
            assert_eq!(sbs_state_index(2, hist), Some(3 + hist));
            assert_eq!(sbs_state_index(63, hist), Some(7 + hist));
        }
    }
}
