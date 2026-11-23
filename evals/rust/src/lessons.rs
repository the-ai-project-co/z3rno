//! Loader and pure metrics for the lessons recall eval
//! (`evals/fixtures/lessons_v1.json`, run by `tests/lessons_run.rs`).

use std::collections::HashSet;

use serde::Deserialize;
use uuid::Uuid;
use z3rno_engine::lessons::{LessonScope, LessonStatus, TaskContext};

#[derive(Debug, Clone, Deserialize)]
pub struct LessonDataset {
    pub version: u32,
    pub name: String,
    pub description: String,
    pub tenant_id: String,
    pub lessons: Vec<LessonSeed>,
    pub tasks: Vec<LessonTask>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LessonSeed {
    pub id: String,
    /// Final status after seeding (`retired` = a stale lesson).
    pub status: LessonStatus,
    pub scope: LessonScope,
    pub title: String,
    pub what_was_tried: String,
    pub what_went_wrong: String,
    pub fix: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LessonTask {
    pub id: String,
    pub context: TaskContext,
    pub text: String,
    /// Lessons this task should get back.
    pub expected: Vec<String>,
    /// Lessons whose scope rules them out for this task.
    pub forbidden: Vec<String>,
}

impl LessonDataset {
    pub fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }
}

/// Totals for a rate: `hits` of `total` returned items. Summed across
/// tasks, so a task that returns nothing doesn't count as a perfect score.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rate {
    pub hits: usize,
    pub total: usize,
}

impl Rate {
    /// Counts how many `returned` ids are in `bad`.
    pub fn add(&mut self, returned: &[Uuid], bad: &HashSet<Uuid>) {
        self.hits += returned.iter().filter(|id| bad.contains(id)).count();
        self.total += returned.len();
    }

    /// `hits / total`, or `0.0` when nothing was returned at all.
    pub fn value(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.hits as f64 / self.total as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_sums_across_calls() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let bad = HashSet::from([b]);
        let mut r = Rate::default();
        assert_eq!(r.value(), 0.0);
        r.add(&[a, b], &bad);
        r.add(&[c], &bad);
        r.add(&[], &bad);
        assert_eq!(r, Rate { hits: 1, total: 3 });
        assert!((r.value() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn fixture_parses_and_references_known_lessons() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/lessons_v1.json");
        let d = LessonDataset::load(path).unwrap();
        let ids: HashSet<_> = d.lessons.iter().map(|l| l.id.as_str()).collect();
        for t in &d.tasks {
            for id in t.expected.iter().chain(&t.forbidden) {
                assert!(ids.contains(id.as_str()), "{} references {id}", t.id);
            }
        }
    }
}
