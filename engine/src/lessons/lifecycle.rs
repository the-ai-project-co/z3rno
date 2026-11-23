//! The lesson lifecycle: which status changes exist, who may make them, and
//! the event record each change leaves behind.
//!
//! Status history is kept as its own record kind (`lesson_event`), not in
//! the store/forget hash chain, so existing audit chains keep verifying
//! unchanged.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::{BackendError, BackendResult, ErrorCode};

use super::model::{Actor, LessonStatus};

pub(crate) const KIND_LESSON_EVENT: &str = "lesson_event";

/// A requested status change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    Confirm,
    Reject,
    Retire,
    /// Bring a retired lesson back to confirmed.
    Reinstate,
}

impl Transition {
    /// The status this transition moves to, if it's allowed from `from`.
    pub fn target(self, from: LessonStatus) -> Option<LessonStatus> {
        use LessonStatus::*;
        match (self, from) {
            (Transition::Confirm, Candidate) => Some(Confirmed),
            (Transition::Reject, Candidate) => Some(Rejected),
            (Transition::Retire, Candidate | Confirmed) => Some(Retired),
            (Transition::Reinstate, Retired) => Some(Confirmed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Transition::Confirm => "confirm",
            Transition::Reject => "reject",
            Transition::Retire => "retire",
            Transition::Reinstate => "reinstate",
        }
    }
}

/// Every status change, edit and delete is a human decision. Agents and CI
/// may only propose (create candidates); a model repeating a claim, or a
/// failure recurring, never promotes a lesson on its own.
pub(crate) fn require_human(actor: &Actor, action: &str) -> BackendResult<()> {
    if actor.is_human() {
        return Ok(());
    }
    Err(BackendError::rejected(
        ErrorCode::NotPermitted,
        format!("only a human can {action} a lesson"),
    ))
}

/// Checks the transition is allowed from `from` and that `actor` may make
/// it, returning the new status.
pub(crate) fn authorize(
    transition: Transition,
    from: LessonStatus,
    actor: &Actor,
) -> BackendResult<LessonStatus> {
    require_human(actor, transition.as_str())?;
    transition.target(from).ok_or_else(|| {
        BackendError::rejected(
            ErrorCode::InvalidTransition,
            format!(
                "can't {} a lesson that is {}",
                transition.as_str(),
                from.as_str()
            ),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LessonAction {
    Created {
        status: LessonStatus,
    },
    Edited,
    StatusChanged {
        from: LessonStatus,
        to: LessonStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        superseded_by: Option<Uuid>,
    },
    /// Records who deleted which id, never the deleted content.
    Deleted,
}

/// One entry in a lesson's history: who did what, when, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LessonEvent {
    pub id: Uuid,
    pub lesson_id: Uuid,
    pub action: LessonAction,
    pub actor: Actor,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl LessonEvent {
    pub(crate) fn new(
        lesson_id: Uuid,
        action: LessonAction,
        actor: &Actor,
        note: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            lesson_id,
            action,
            actor: actor.clone(),
            at: Utc::now(),
            note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LessonStatus::*;

    const ALL_STATUSES: [LessonStatus; 4] = [Candidate, Confirmed, Retired, Rejected];
    const ALL: [Transition; 4] = [
        Transition::Confirm,
        Transition::Reject,
        Transition::Retire,
        Transition::Reinstate,
    ];

    #[test]
    fn allowed_transitions_table() {
        let allowed: Vec<_> = ALL
            .iter()
            .flat_map(|t| ALL_STATUSES.iter().map(move |s| (*t, *s)))
            .filter_map(|(t, s)| t.target(s).map(|to| (t, s, to)))
            .collect();
        assert_eq!(
            allowed,
            vec![
                (Transition::Confirm, Candidate, Confirmed),
                (Transition::Reject, Candidate, Rejected),
                (Transition::Retire, Candidate, Retired),
                (Transition::Retire, Confirmed, Retired),
                (Transition::Reinstate, Retired, Confirmed),
            ]
        );
    }

    #[test]
    fn rejected_is_terminal() {
        assert!(ALL.iter().all(|t| t.target(Rejected).is_none()));
    }

    #[test]
    fn agents_and_ci_can_never_change_status() {
        for actor in [Actor::agent("claude", None), Actor::ci("github-actions")] {
            for t in ALL {
                let err = authorize(t, Candidate, &actor).unwrap_err();
                assert_eq!(err.code(), Some(ErrorCode::NotPermitted));
            }
        }
    }

    #[test]
    fn a_human_gets_invalid_transition_for_a_disallowed_move() {
        let err = authorize(Transition::Confirm, Confirmed, &Actor::human("ana")).unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidTransition));
        assert_eq!(
            authorize(Transition::Confirm, Candidate, &Actor::human("ana")).unwrap(),
            Confirmed
        );
    }
}
