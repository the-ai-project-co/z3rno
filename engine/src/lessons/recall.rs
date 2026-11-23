//! Ranking for scoped lesson recall. Candidates are first narrowed by scope
//! and status (exact, see `super::scope`), then ordered by fusing up to
//! three independent rankings with reciprocal rank fusion:
//!
//! - vector similarity, when the caller supplies a query embedding;
//! - lexical relevance (BM25 over the lesson text), when it supplies text;
//! - scope specificity (a path-scoped lesson beats a repo-wide one).
//!
//! Fusing ranks rather than raw scores means a weak embedding (e.g. the
//! hashing fallback) can't drown out an exact path or keyword hit.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::model::Lesson;
use super::scope::TaskContext;

/// Standard RRF damping constant.
pub const RRF_K: f64 = 60.0;
pub const DEFAULT_LIMIT: usize = 5;
pub const MAX_LIMIT: usize = 20;

/// A scoped recall request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LessonQuery {
    pub context: TaskContext,
    /// Free-text description of the task, matched lexically.
    #[serde(default)]
    pub text: Option<String>,
    /// Embedding of the task description, matched by vector similarity.
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
    /// Also return candidate (unconfirmed) lessons, labelled unverified.
    #[serde(default)]
    pub include_candidates: bool,
    /// Allow recall with no repo in `context` (global lessons only then).
    #[serde(default)]
    pub include_global: bool,
    /// At most this many lessons (clamped to `MAX_LIMIT`).
    #[serde(default)]
    pub limit: Option<usize>,
}

impl LessonQuery {
    pub fn effective_limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
}

/// One ranked result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecalledLesson {
    pub lesson: Lesson,
    /// Fused RRF score (higher is better); only meaningful within one call.
    pub score: f64,
    /// Reasons the reader should double-check this lesson.
    pub warnings: Vec<String>,
}

/// Inputs to fusion for one applicable lesson.
pub(crate) struct Candidate {
    pub lesson: Lesson,
    pub specificity: u32,
    pub warnings: Vec<String>,
}

/// Orders `candidates` by fused rank and keeps the top `limit`. Ties break
/// on most recently confirmed/created, then id, so output is deterministic.
pub(crate) fn rank(
    candidates: Vec<Candidate>,
    text: Option<&str>,
    vector_order: &[Uuid],
    limit: usize,
) -> Vec<RecalledLesson> {
    let ids: Vec<Uuid> = candidates.iter().map(|c| c.lesson.id).collect();
    let mut score: HashMap<Uuid, f64> = ids.iter().map(|id| (*id, 0.0)).collect();

    let mut add_ranking = |ordered: Vec<Uuid>| {
        for (rank, id) in ordered.into_iter().enumerate() {
            if let Some(s) = score.get_mut(&id) {
                *s += 1.0 / (RRF_K + rank as f64 + 1.0);
            }
        }
    };

    // Vector: the backend's order, restricted to applicable lessons.
    let applicable: HashSet<Uuid> = ids.iter().copied().collect();
    add_ranking(
        vector_order
            .iter()
            .copied()
            .filter(|id| applicable.contains(id))
            .collect(),
    );

    // Lexical: only lessons with a non-zero match are ranked.
    if let Some(text) = text {
        let docs: Vec<(Uuid, String)> = candidates
            .iter()
            .map(|c| (c.lesson.id, c.lesson.searchable_text()))
            .collect();
        add_ranking(bm25_order(text, &docs));
    }

    // Specificity: every applicable lesson, most specific first; equal
    // specificity shares a rank so it never decides between equals.
    let mut by_spec: Vec<(Uuid, u32)> = candidates
        .iter()
        .map(|c| (c.lesson.id, c.specificity))
        .collect();
    by_spec.sort_by_key(|(_, spec)| std::cmp::Reverse(*spec));
    let mut rank = 0usize;
    let mut previous = None;
    for (position, (id, spec)) in by_spec.iter().enumerate() {
        if previous != Some(*spec) {
            rank = position;
            previous = Some(*spec);
        }
        if let Some(s) = score.get_mut(id) {
            *s += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }

    let mut ranked: Vec<RecalledLesson> = candidates
        .into_iter()
        .map(|c| RecalledLesson {
            score: score[&c.lesson.id],
            lesson: c.lesson,
            warnings: c.warnings,
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| freshness(&b.lesson).cmp(&freshness(&a.lesson)))
            .then_with(|| a.lesson.id.cmp(&b.lesson.id))
    });
    ranked.truncate(limit);
    ranked
}

fn freshness(lesson: &Lesson) -> chrono::DateTime<chrono::Utc> {
    lesson.confirmed_at.unwrap_or(lesson.created_at)
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.chars().count() > 1)
        .map(str::to_lowercase)
        .collect()
}

/// BM25 (k1 = 1.2, b = 0.75) over the candidate set; returns ids with a
/// positive score, best first.
fn bm25_order(query: &str, docs: &[(Uuid, String)]) -> Vec<Uuid> {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let query: HashSet<String> = tokens(query).into_iter().collect();
    if query.is_empty() || docs.is_empty() {
        return Vec::new();
    }
    let doc_tokens: Vec<Vec<String>> = docs.iter().map(|(_, t)| tokens(t)).collect();
    let n = docs.len() as f64;
    let avg_len = doc_tokens.iter().map(Vec::len).sum::<usize>() as f64 / n;
    let df: HashMap<&str, usize> = query
        .iter()
        .map(|q| {
            let count = doc_tokens
                .iter()
                .filter(|d| d.iter().any(|t| t == q))
                .count();
            (q.as_str(), count)
        })
        .collect();

    let mut scored: Vec<(Uuid, f64)> = docs
        .iter()
        .zip(&doc_tokens)
        .map(|((id, _), toks)| {
            let len = toks.len() as f64;
            let s: f64 = query
                .iter()
                .map(|q| {
                    let tf = toks.iter().filter(|t| *t == q).count() as f64;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    let d = df[q.as_str()] as f64;
                    let idf = ((n - d + 0.5) / (d + 0.5) + 1.0).ln();
                    idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * len / avg_len.max(1.0)))
                })
                .sum();
            (*id, s)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::model::{tests::draft, Actor, LessonStatus, SCHEMA_VERSION};
    use chrono::Utc;

    fn candidate(title: &str, specificity: u32) -> Candidate {
        let mut d = draft();
        d.title = title.into();
        Candidate {
            lesson: Lesson {
                id: Uuid::new_v4(),
                schema_version: SCHEMA_VERSION,
                draft: d,
                status: LessonStatus::Confirmed,
                origin: Actor::human("ana"),
                created_at: Utc::now(),
                confirmed_by: None,
                confirmed_at: None,
                last_shown_at: None,
                shown_count: 0,
                superseded_by: None,
            },
            specificity,
            warnings: vec![],
        }
    }

    #[test]
    fn lexical_hit_outranks_an_unrelated_lesson() {
        let a = candidate("Postgres migrations need a lock timeout", 1);
        let b = candidate("Webpack cache busting", 1);
        let (a_id, b_id) = (a.lesson.id, b.lesson.id);
        let ranked = rank(vec![b, a], Some("add postgres migration"), &[], 5);
        assert_eq!(ranked[0].lesson.id, a_id);
        assert_eq!(ranked[1].lesson.id, b_id);
    }

    #[test]
    fn specificity_breaks_ties_when_nothing_else_differs() {
        let broad = candidate("x", 1);
        let narrow = candidate("x", 4);
        let narrow_id = narrow.lesson.id;
        let ranked = rank(vec![broad, narrow], None, &[], 5);
        assert_eq!(ranked[0].lesson.id, narrow_id);
    }

    #[test]
    fn vector_order_only_counts_applicable_ids_and_limit_is_respected() {
        let cands: Vec<_> = (0..6).map(|i| candidate(&format!("t{i}"), 1)).collect();
        let favourite = cands[5].lesson.id;
        let order = vec![Uuid::new_v4(), favourite];
        let ranked = rank(cands, None, &order, 3);
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].lesson.id, favourite);
    }

    #[test]
    fn ordering_is_deterministic_for_identical_inputs() {
        let cands: Vec<_> = (0..5).map(|_| candidate("same", 1)).collect();
        let first: Vec<_> = rank(
            cands.iter().map(clone_candidate).collect(),
            Some("same"),
            &[],
            5,
        )
        .into_iter()
        .map(|r| r.lesson.id)
        .collect();
        let second: Vec<_> = rank(
            cands.iter().rev().map(clone_candidate).collect(),
            Some("same"),
            &[],
            5,
        )
        .into_iter()
        .map(|r| r.lesson.id)
        .collect();
        assert_eq!(first, second);
    }

    fn clone_candidate(c: &Candidate) -> Candidate {
        Candidate {
            lesson: c.lesson.clone(),
            specificity: c.specificity,
            warnings: c.warnings.clone(),
        }
    }
}
