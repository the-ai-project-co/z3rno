//! Pure scoring functions for the golden-dataset harness. None of these
//! touch the engine or the filesystem — they take plain ids/strings/numbers
//! in and return a score out, so they're unit-tested directly here rather
//! than only indirectly through the integration test.

use std::collections::HashSet;

use uuid::Uuid;

/// Fraction of `expected_ids` present anywhere in `retrieved_ids[..k]`.
/// `f64::NAN` if `expected_ids` is empty (nothing to score against).
pub fn recall_at_k(retrieved_ids: &[Uuid], expected_ids: &[Uuid], k: usize) -> f64 {
    if expected_ids.is_empty() {
        return f64::NAN;
    }
    let top_k = &retrieved_ids[..retrieved_ids.len().min(k)];
    let hits = expected_ids.iter().filter(|e| top_k.contains(e)).count();
    hits as f64 / expected_ids.len() as f64
}

/// Reciprocal rank (1-indexed) of the first `expected_ids` member found in
/// `retrieved_ids`. `f64::NAN` if `expected_ids` is empty; `0.0` if none of
/// `expected_ids` appear anywhere in `retrieved_ids`.
pub fn mrr(retrieved_ids: &[Uuid], expected_ids: &[Uuid]) -> f64 {
    if expected_ids.is_empty() {
        return f64::NAN;
    }
    match retrieved_ids
        .iter()
        .position(|id| expected_ids.contains(id))
    {
        Some(pos) => 1.0 / (pos + 1) as f64,
        None => 0.0,
    }
}

/// p50/p95/p99/mean over a set of latency samples (milliseconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyPercentiles {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub mean: f64,
    pub count: usize,
}

/// Nearest-rank percentiles: sort ascending, `idx = clamp(ceil(p*n) - 1, 0,
/// n-1)`. Empty input returns all-zero with `count: 0` rather than NaN or a
/// panic — there's no meaningful percentile of zero samples, but a caller
/// aggregating across items shouldn't have to special-case "no samples yet".
pub fn latency_percentiles(latencies_ms: &[f64]) -> LatencyPercentiles {
    let n = latencies_ms.len();
    if n == 0 {
        return LatencyPercentiles {
            p50: 0.0,
            p95: 0.0,
            p99: 0.0,
            mean: 0.0,
            count: 0,
        };
    }
    let mut sorted = latencies_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |p: f64| -> f64 {
        let idx = ((p * n as f64).ceil() as isize - 1).clamp(0, n as isize - 1) as usize;
        sorted[idx]
    };
    let mean = sorted.iter().sum::<f64>() / n as f64;
    LatencyPercentiles {
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
        mean,
        count: n,
    }
}

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "to", "of", "in", "on", "at", "and", "or", "for",
];

/// Lowercases and replaces every non-alphanumeric, non-whitespace character
/// with a space (punctuation-safe: "digest emails." doesn't glue into
/// "digestemails").
fn clean(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect()
}

/// A deterministic word-coverage stub judge — **not** an LLM-judged score.
/// See `evals/rust/README.md` for the framing. Fraction of `expected_answer`'s
/// non-stopword tokens that also appear (as a whole whitespace-delimited
/// token, not a raw substring) somewhere in `recalled_context`.
///
/// `f64::NAN` if `expected_answer` is empty, or reduces to zero tokens once
/// stopwords are stripped.
pub fn faithfulness(expected_answer: &str, recalled_context: &str) -> f64 {
    if expected_answer.trim().is_empty() {
        return f64::NAN;
    }
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    let expected_tokens: HashSet<String> = clean(expected_answer)
        .split_whitespace()
        .filter(|t| !t.is_empty() && !stop.contains(t))
        .map(String::from)
        .collect();
    if expected_tokens.is_empty() {
        return f64::NAN;
    }

    let cleaned_ctx = clean(recalled_context);
    let ctx_tokens: HashSet<&str> = cleaned_ctx.split_whitespace().collect();

    let hits = expected_tokens
        .iter()
        .filter(|t| ctx_tokens.contains(t.as_str()))
        .count();
    hits as f64 / expected_tokens.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuids(n: usize) -> Vec<Uuid> {
        // Deterministic, distinct ids for test fixtures.
        (0..n).map(|i| Uuid::from_u128(i as u128)).collect()
    }

    #[test]
    fn recall_at_k_all_hit() {
        let ids = uuids(5);
        let retrieved = vec![ids[0], ids[1], ids[2]];
        let expected = vec![ids[0], ids[1]];
        assert_eq!(recall_at_k(&retrieved, &expected, 5), 1.0);
    }

    #[test]
    fn recall_at_k_zero_hit() {
        let ids = uuids(5);
        let retrieved = vec![ids[0], ids[1]];
        let expected = vec![ids[3], ids[4]];
        assert_eq!(recall_at_k(&retrieved, &expected, 5), 0.0);
    }

    #[test]
    fn recall_at_k_partial_hit_respects_k_cutoff() {
        let ids = uuids(5);
        // expected[1] (ids[1]) is retrieved at rank 3, but k=2 cuts it off.
        let retrieved = vec![ids[0], ids[2], ids[1]];
        let expected = vec![ids[0], ids[1]];
        assert_eq!(recall_at_k(&retrieved, &expected, 2), 0.5);
    }

    #[test]
    fn recall_at_k_empty_expected_is_nan() {
        let ids = uuids(2);
        assert!(recall_at_k(&ids, &[], 5).is_nan());
    }

    #[test]
    fn mrr_first_hit_at_rank_one() {
        let ids = uuids(3);
        let retrieved = vec![ids[0], ids[1], ids[2]];
        assert_eq!(mrr(&retrieved, &[ids[0]]), 1.0);
    }

    #[test]
    fn mrr_hit_at_rank_three() {
        let ids = uuids(3);
        let retrieved = vec![ids[1], ids[2], ids[0]];
        assert_eq!(mrr(&retrieved, &[ids[0]]), 1.0 / 3.0);
    }

    #[test]
    fn mrr_zero_when_not_found() {
        let ids = uuids(4);
        let retrieved = vec![ids[1], ids[2]];
        assert_eq!(mrr(&retrieved, &[ids[3]]), 0.0);
    }

    #[test]
    fn mrr_empty_expected_is_nan() {
        let ids = uuids(2);
        assert!(mrr(&ids, &[]).is_nan());
    }

    #[test]
    fn latency_percentiles_empty_is_all_zero() {
        let p = latency_percentiles(&[]);
        assert_eq!(p.count, 0);
        assert_eq!(p.p50, 0.0);
        assert_eq!(p.p95, 0.0);
        assert_eq!(p.p99, 0.0);
        assert_eq!(p.mean, 0.0);
    }

    #[test]
    fn latency_percentiles_worked_example() {
        // n=5, nearest-rank: idx = clamp(ceil(p*n)-1, 0, n-1).
        // p50 -> ceil(2.5)-1=2 -> 30.0; p95/p99 -> ceil(4.75/4.95)-1=4 -> 50.0.
        let p = latency_percentiles(&[50.0, 10.0, 30.0, 20.0, 40.0]);
        assert_eq!(p.count, 5);
        assert_eq!(p.mean, 30.0);
        assert_eq!(p.p50, 30.0);
        assert_eq!(p.p95, 50.0);
        assert_eq!(p.p99, 50.0);
    }

    #[test]
    fn latency_percentiles_single_sample() {
        let p = latency_percentiles(&[42.0]);
        assert_eq!(p.count, 1);
        assert_eq!(p.p50, 42.0);
        assert_eq!(p.p95, 42.0);
        assert_eq!(p.p99, 42.0);
        assert_eq!(p.mean, 42.0);
    }

    #[test]
    fn faithfulness_empty_expected_answer_is_nan() {
        assert!(faithfulness("", "anything here").is_nan());
        assert!(faithfulness("   ", "anything here").is_nan());
    }

    #[test]
    fn faithfulness_all_stopwords_is_nan() {
        // "The a an" reduces to zero non-stopword tokens.
        assert!(faithfulness("The a an", "the user prefers dark mode").is_nan());
    }

    #[test]
    fn faithfulness_full_coverage_is_one() {
        // expected_tokens (stopwords "the"/"and" dropped): user, prefers,
        // dark, mode, weekly, digest, emails -> 7 tokens, all present in ctx.
        let expected = "The user prefers dark mode and weekly digest emails.";
        let ctx = "The user prefers dark mode and weekly digest emails.";
        assert_eq!(faithfulness(expected, ctx), 1.0);
    }

    #[test]
    fn faithfulness_zero_hit() {
        let expected = "The user prefers dark mode and weekly digest emails.";
        let ctx = "Quarterly revenue exceeded analyst forecasts substantially.";
        assert_eq!(faithfulness(expected, ctx), 0.0);
    }

    #[test]
    fn faithfulness_partial_hit_worked_example() {
        // expected_tokens: {user, prefers, dark, mode, weekly, digest,
        // emails} (7, "the"/"and" dropped). ctx has "user", "dark", "mode"
        // -> 3/7.
        let expected = "The user prefers dark mode and weekly digest emails.";
        let ctx = "The user is in dark mode most of the day.";
        assert_eq!(faithfulness(expected, ctx), 3.0 / 7.0);
    }

    #[test]
    fn faithfulness_does_not_match_inside_a_longer_unrelated_word() {
        // "cat" must not match "concatenate" -- word-safe token matching,
        // not substring-of-string.
        assert_eq!(faithfulness("cat", "we concatenate strings"), 0.0);
    }
}
