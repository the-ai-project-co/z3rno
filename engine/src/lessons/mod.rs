//! Lessons learned from coding mistakes: a typed, validated record kept in
//! the procedural tier, with a human-gated lifecycle, scoped recall, secret
//! redaction, and a safe renderer for putting recalled lessons in front of
//! an agent. Entry point: `MemoryEngine::lessons(tenant_id)`.

pub mod lifecycle;
pub mod model;
pub mod recall;
pub mod redact;
pub mod render;
pub mod scope;
pub mod store;
pub mod version;

#[cfg(test)]
mod tests;

pub use lifecycle::{LessonAction, LessonEvent, Transition};
pub use model::{
    Actor, ActorKind, DependencyScope, Evidence, EvidenceKind, Lesson, LessonDraft, LessonScope,
    LessonStatus, SCHEMA_VERSION,
};
pub use recall::{LessonQuery, RecalledLesson};
pub use redact::Redactor;
pub use render::{render_for_agent, Rendered, DEFAULT_MAX_CHARS};
pub use scope::TaskContext;
pub use store::{lesson_from_memory, LessonFilter, Lessons};
