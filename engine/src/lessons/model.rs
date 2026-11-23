//! The lesson record: what was tried, what went wrong, the fix, the
//! evidence for it, and where it applies. Stored as validated JSON inside a
//! procedural-tier `Memory`'s metadata (see `super::store`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::{BackendError, BackendResult, ErrorCode};

use super::version::VersionRange;

pub const SCHEMA_VERSION: u32 = 1;

pub(crate) const MAX_TITLE: usize = 200;
pub(crate) const MAX_TEXT: usize = 4_000;
pub(crate) const MAX_EVIDENCE: usize = 20;
pub(crate) const MAX_EVIDENCE_TEXT: usize = 2_000;
pub(crate) const MAX_SCOPE_ITEMS: usize = 50;
pub(crate) const MAX_NAME: usize = 200;
/// Cap on the whole serialized lesson, so one record can't balloon the
/// store or a recall's context budget.
pub(crate) const MAX_SERIALIZED_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LessonStatus {
    /// Proposed (by an agent, CI, or a human who didn't confirm it). Not
    /// recalled unless the caller opts in, and then labelled unverified.
    Candidate,
    /// A human confirmed it. The only status recalled by default.
    Confirmed,
    /// No longer applies (stale, superseded, or withdrawn). Never recalled.
    Retired,
    /// A human judged it wrong. Never recalled. Terminal.
    Rejected,
}

impl LessonStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            LessonStatus::Candidate => "candidate",
            LessonStatus::Confirmed => "confirmed",
            LessonStatus::Retired => "retired",
            LessonStatus::Rejected => "rejected",
        }
    }
}

impl std::str::FromStr for LessonStatus {
    type Err = BackendError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "candidate" => Ok(LessonStatus::Candidate),
            "confirmed" => Ok(LessonStatus::Confirmed),
            "retired" => Ok(LessonStatus::Retired),
            "rejected" => Ok(LessonStatus::Rejected),
            other => Err(invalid(format!(
                "unknown status {other:?} (expected candidate, confirmed, retired or rejected)"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Agent,
    Ci,
}

/// Who performed an operation. The engine is an embedded library with no
/// authentication of its own: it trusts the interface that built this
/// value. Interfaces an agent drives (the MCP server) must only ever build
/// `Agent` actors; only an interactive human surface builds `Human`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub kind: ActorKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl Actor {
    pub fn human(name: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Human,
            name: name.into(),
            session: None,
        }
    }

    pub fn agent(name: impl Into<String>, session: Option<String>) -> Self {
        Self {
            kind: ActorKind::Agent,
            name: name.into(),
            session,
        }
    }

    pub fn ci(name: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Ci,
            name: name.into(),
            session: None,
        }
    }

    pub fn is_human(&self) -> bool {
        self.kind == ActorKind::Human
    }

    pub(crate) fn validate(&self) -> BackendResult<()> {
        check_required("actor.name", &self.name, MAX_NAME)?;
        if let Some(session) = &self.session {
            check_len("actor.session", session, MAX_NAME)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ErrorText,
    FailingTest,
    Commit,
    FilePath,
    DiffExcerpt,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub text: String,
    /// Optional pointer (URL, test id, commit sha, file:line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyScope {
    pub name: String,
    /// e.g. `">=18, <19"`; `None` means any version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
}

/// Where a lesson applies. A lesson either names a repo or is explicitly
/// `global`; recall never matches a scopeless lesson by accident.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LessonScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default)]
    pub global: bool,
    /// Glob patterns relative to the repo root (`src/auth/**`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DependencyScope>,
    /// A command or tool name the lesson is about (`cargo`, `terraform`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Caller-authored content of a lesson: everything a human or agent writes.
/// System-managed fields (status, timestamps, counters) live on `Lesson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LessonDraft {
    pub title: String,
    pub what_was_tried: String,
    pub what_went_wrong: String,
    pub fix: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    pub scope: LessonScope,
    /// Memory ids this lesson was derived from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<Uuid>,
}

/// A stored lesson: the draft plus its lifecycle state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub id: Uuid,
    pub schema_version: u32,
    #[serde(flatten)]
    pub draft: LessonDraft,
    pub status: LessonStatus,
    pub origin: Actor,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_by: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_shown_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub shown_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Uuid>,
}

impl Lesson {
    /// The text that gets embedded and lexically matched: everything a
    /// reader would search on, in one string.
    pub fn searchable_text(&self) -> String {
        self.draft.searchable_text()
    }
}

impl LessonDraft {
    /// The text that gets embedded and lexically matched: everything a
    /// reader would search on, in one string.
    pub fn searchable_text(&self) -> String {
        let mut text = format!(
            "{}\n{}\n{}\n{}",
            self.title, self.what_was_tried, self.what_went_wrong, self.fix
        );
        for e in &self.evidence {
            text.push('\n');
            text.push_str(&e.text);
        }
        text
    }

    /// Validates shape and size. Called on every create and edit, after
    /// redaction, so limits apply to what's actually stored.
    pub fn validate(&self) -> BackendResult<()> {
        check_required("title", &self.title, MAX_TITLE)?;
        check_required("what_was_tried", &self.what_was_tried, MAX_TEXT)?;
        check_required("what_went_wrong", &self.what_went_wrong, MAX_TEXT)?;
        check_required("fix", &self.fix, MAX_TEXT)?;

        if self.evidence.len() > MAX_EVIDENCE {
            return Err(invalid(format!(
                "evidence has {} items; at most {MAX_EVIDENCE} allowed",
                self.evidence.len()
            )));
        }
        for (i, e) in self.evidence.iter().enumerate() {
            check_required(&format!("evidence[{i}].text"), &e.text, MAX_EVIDENCE_TEXT)?;
            if let Some(r) = &e.reference {
                check_len(&format!("evidence[{i}].reference"), r, MAX_NAME)?;
            }
        }
        self.scope.validate()
    }
}

impl LessonScope {
    fn validate(&self) -> BackendResult<()> {
        match (&self.repo, self.global) {
            (None, false) => {
                return Err(invalid(
                    "scope needs a repo, or global = true for a lesson that applies everywhere",
                ))
            }
            (Some(_), true) => {
                return Err(invalid("scope can't name a repo and also be global"));
            }
            (Some(repo), false) => check_required("scope.repo", repo, MAX_NAME)?,
            (None, true) => {}
        }
        for (name, items) in [
            ("scope.paths", &self.paths),
            ("scope.languages", &self.languages),
        ] {
            if items.len() > MAX_SCOPE_ITEMS {
                return Err(invalid(format!(
                    "{name} has {} items; at most {MAX_SCOPE_ITEMS} allowed",
                    items.len()
                )));
            }
            for item in items {
                check_required(name, item, MAX_NAME)?;
            }
        }
        if self.dependencies.len() > MAX_SCOPE_ITEMS {
            return Err(invalid("scope.dependencies has too many items"));
        }
        for dep in &self.dependencies {
            check_required("scope.dependencies[].name", &dep.name, MAX_NAME)?;
            if let Some(range) = &dep.range {
                VersionRange::parse(range)
                    .map_err(|e| invalid(format!("scope.dependencies[{}].range: {e}", dep.name)))?;
            }
        }
        if let Some(tool) = &self.tool {
            check_required("scope.tool", tool, MAX_NAME)?;
        }
        Ok(())
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> BackendError {
    BackendError::rejected(ErrorCode::InvalidInput, message)
}

fn check_len(field: &str, value: &str, max: usize) -> BackendResult<()> {
    let len = value.chars().count();
    if len > max {
        return Err(invalid(format!(
            "{field} is {len} characters; at most {max} allowed"
        )));
    }
    Ok(())
}

fn check_required(field: &str, value: &str, max: usize) -> BackendResult<()> {
    if value.trim().is_empty() {
        return Err(invalid(format!("{field} is required")));
    }
    check_len(field, value, max)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn draft() -> LessonDraft {
        LessonDraft {
            title: "Mock the clock in token tests".into(),
            what_was_tried: "Asserted on expiry using the wall clock".into(),
            what_went_wrong: "Test flaked near second boundaries".into(),
            fix: "Inject a fixed clock".into(),
            evidence: vec![Evidence {
                kind: EvidenceKind::FailingTest,
                text: "auth::token::expires_after_ttl".into(),
                reference: None,
            }],
            scope: LessonScope {
                repo: Some("acme/api".into()),
                paths: vec!["src/auth/**".into()],
                ..Default::default()
            },
            derived_from: vec![],
        }
    }

    fn code(r: BackendResult<()>) -> Option<ErrorCode> {
        r.err().and_then(|e| e.code())
    }

    #[test]
    fn a_complete_draft_is_valid() {
        draft().validate().unwrap();
    }

    #[test]
    fn missing_required_fields_are_rejected_with_the_field_name() {
        let mut d = draft();
        d.fix = "   ".into();
        let err = d.validate().unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidInput));
        assert!(err.to_string().contains("fix is required"), "{err}");
    }

    #[test]
    fn scope_must_name_a_repo_or_be_global_but_not_both() {
        let mut d = draft();
        d.scope.repo = None;
        assert_eq!(code(d.validate()), Some(ErrorCode::InvalidInput));
        d.scope.global = true;
        d.validate().unwrap();
        d.scope.repo = Some("acme/api".into());
        assert_eq!(code(d.validate()), Some(ErrorCode::InvalidInput));
    }

    #[test]
    fn oversized_fields_and_lists_are_rejected() {
        let mut d = draft();
        d.title = "x".repeat(MAX_TITLE + 1);
        assert!(d.validate().is_err());

        let mut d = draft();
        d.evidence = vec![d.evidence[0].clone(); MAX_EVIDENCE + 1];
        assert!(d.validate().is_err());
    }

    #[test]
    fn malformed_dependency_ranges_are_rejected() {
        let mut d = draft();
        d.scope.dependencies = vec![DependencyScope {
            name: "react".into(),
            range: Some(">= banana".into()),
        }];
        assert_eq!(code(d.validate()), Some(ErrorCode::InvalidInput));
    }

    #[test]
    fn status_parses_from_its_wire_name() {
        for s in ["candidate", "confirmed", "retired", "rejected"] {
            assert_eq!(s.parse::<LessonStatus>().unwrap().as_str(), s);
        }
        assert!("done".parse::<LessonStatus>().is_err());
    }
}
