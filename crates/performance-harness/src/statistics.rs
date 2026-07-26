//! Integer-only summary recomputation from authoritative raw samples.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{HarnessError, Sample};

const OUTCOMES: [&str; 4] = ["success", "refused", "failed", "deadline_exceeded"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeCounts {
    pub success: usize,
    pub refused: usize,
    pub failed: usize,
    pub deadline_exceeded: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variance {
    pub numerator: String,
    pub denominator: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Distribution {
    pub outcome: String,
    pub count: usize,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    pub population_variance_ns2: Variance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalGroup {
    pub domain: String,
    pub code: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerSummary {
    pub sample_count: usize,
    pub outcome_counts: OutcomeCounts,
    pub distributions: Vec<Distribution>,
    pub refusal_groups: Vec<RefusalGroup>,
    pub diagnostic_outlier_sequences: Vec<usize>,
    pub disposition: String,
}

pub(crate) fn summarize(samples: &[Sample]) -> Result<ProducerSummary, HarnessError> {
    let mut by_outcome = BTreeMap::<&str, Vec<u64>>::new();
    let mut refusal_counts = BTreeMap::<(String, String), usize>::new();
    let mut counts = OutcomeCounts::default();
    for sample in samples {
        by_outcome
            .entry(sample.outcome())
            .or_default()
            .push(sample.duration_ns());
        match sample.outcome() {
            "success" => counts.success += 1,
            "refused" => counts.refused += 1,
            "failed" => counts.failed += 1,
            "deadline_exceeded" => counts.deadline_exceeded += 1,
            _ => return Err(HarnessError::InvalidResult("unknown outcome")),
        }
        if let Some(refusal) = sample.refusal() {
            *refusal_counts
                .entry((refusal.domain.clone(), refusal.code.clone()))
                .or_default() += 1;
        }
    }
    let distributions = OUTCOMES
        .into_iter()
        .filter_map(|outcome| {
            by_outcome
                .get(outcome)
                .map(|values| distribution(outcome, values))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let diagnostic_outlier_sequences = outliers(samples);
    let diagnostic = counts.failed > 0
        || counts.deadline_exceeded > 0
        || !diagnostic_outlier_sequences.is_empty();
    Ok(ProducerSummary {
        sample_count: samples.len(),
        outcome_counts: counts,
        distributions,
        refusal_groups: refusal_counts
            .into_iter()
            .map(|((domain, code), count)| RefusalGroup {
                domain,
                code,
                count,
            })
            .collect(),
        diagnostic_outlier_sequences,
        disposition: if diagnostic { "diagnostic" } else { "valid" }.to_owned(),
    })
}

fn distribution(outcome: &str, values: &[u64]) -> Result<Distribution, HarnessError> {
    let count = u128::try_from(values.len())
        .map_err(|_| HarnessError::InvalidResult("sample count exceeds u128"))?;
    let total = values.iter().try_fold(0_u128, |sum, value| {
        sum.checked_add(u128::from(*value))
            .ok_or(HarnessError::InvalidResult("duration sum overflow"))
    })?;
    let squares = values.iter().try_fold(0_u128, |sum, value| {
        let value = u128::from(*value);
        sum.checked_add(value * value)
            .ok_or(HarnessError::InvalidResult("duration square overflow"))
    })?;
    let numerator = count
        .checked_mul(squares)
        .and_then(|left| left.checked_sub(total * total))
        .ok_or(HarnessError::InvalidResult("variance overflow"))?;
    let denominator = count * count;
    Ok(Distribution {
        outcome: outcome.to_owned(),
        count: values.len(),
        p50_ns: percentile(values, 50),
        p95_ns: percentile(values, 95),
        p99_ns: percentile(values, 99),
        max_ns: *values.iter().max().expect("non-empty distribution"),
        population_variance_ns2: Variance {
            numerator: numerator.to_string(),
            denominator: denominator.to_string(),
        },
    })
}

fn percentile(values: &[u64], percent: usize) -> u64 {
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let rank = (ordered.len() * percent).div_ceil(100);
    ordered[rank - 1]
}

fn outliers(samples: &[Sample]) -> Vec<usize> {
    let successes = samples
        .iter()
        .filter(|sample| sample.outcome() == "success")
        .collect::<Vec<_>>();
    if successes.is_empty() {
        return Vec::new();
    }
    let durations = successes
        .iter()
        .map(|sample| sample.duration_ns())
        .collect::<Vec<_>>();
    let q1 = percentile(&durations, 25);
    let q3 = percentile(&durations, 75);
    let ceiling = u128::from(q3) + 3 * u128::from(q3 - q1);
    successes
        .into_iter()
        .filter(|sample| u128::from(sample.duration_ns()) > ceiling)
        .map(|sample| sample.sequence())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success(sequence: usize, duration_ns: u64) -> Sample {
        Sample::Success {
            sequence,
            duration_ns,
            cpu_time_ns: None,
            peak_rss_bytes: None,
        }
    }

    #[test]
    fn nearest_rank_and_exact_population_variance_match_v1() {
        let summary = summarize(&[success(0, 1), success(1, 2), success(2, 3)]).unwrap();
        let distribution = &summary.distributions[0];
        assert_eq!(
            (
                distribution.p50_ns,
                distribution.p95_ns,
                distribution.p99_ns
            ),
            (2, 3, 3)
        );
        assert_eq!(
            distribution.population_variance_ns2,
            Variance {
                numerator: "6".to_owned(),
                denominator: "9".to_owned(),
            }
        );
    }
}
