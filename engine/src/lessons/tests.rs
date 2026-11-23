//! Lesson API acceptance tests against a real embedded store.

use uuid::Uuid;

use super::model::tests::draft;
use super::*;
use crate::backend::ErrorCode;
use crate::MemoryEngine;

fn engine() -> MemoryEngine {
    let mut path = std::env::temp_dir();
    path.push(format!("z3rno-lessons-{}.db", Uuid::new_v4()));
    MemoryEngine::embedded(path).unwrap()
}

fn human() -> Actor {
    Actor::human("ana")
}

fn agent() -> Actor {
    Actor::agent("claude-code", Some("session-1".into()))
}

fn ctx(repo: &str, paths: &[&str]) -> TaskContext {
    TaskContext {
        repo: Some(repo.into()),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        ..Default::default()
    }
}

fn query(context: TaskContext) -> LessonQuery {
    LessonQuery {
        context,
        ..Default::default()
    }
}

fn ids(results: &[RecalledLesson]) -> Vec<Uuid> {
    results.iter().map(|r| r.lesson.id).collect()
}

#[tokio::test]
async fn create_round_trips_and_lists() {
    let e = engine();
    let l = e.lessons("t");
    let created = l
        .create(&human(), draft(), LessonStatus::Confirmed, None)
        .await
        .unwrap();
    assert_eq!(l.get(created.id).await.unwrap().unwrap(), created);
    assert_eq!(
        l.list(&LessonFilter::default()).await.unwrap(),
        vec![created.clone()]
    );
    let memory = e.advanced().list_memories("t").await.unwrap();
    assert_eq!(memory[0].tier, crate::Tier::Procedural);
    let history = l.history(created.id).await.unwrap();
    assert_eq!(
        history[0].action,
        LessonAction::Created {
            status: LessonStatus::Confirmed
        }
    );
}

#[tokio::test]
async fn invalid_lessons_are_rejected_with_a_code() {
    let e = engine();
    let mut d = draft();
    d.what_went_wrong = String::new();
    let err = e
        .lessons("t")
        .create(&human(), d, LessonStatus::Candidate, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::InvalidInput));
    assert!(e.advanced().list_memories("t").await.unwrap().is_empty());
}

#[tokio::test]
async fn agents_can_only_propose_candidates() {
    let e = engine();
    let l = e.lessons("t");
    let err = l
        .create(&agent(), draft(), LessonStatus::Confirmed, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::NotPermitted));

    let c = l
        .create(&agent(), draft(), LessonStatus::Candidate, None)
        .await
        .unwrap();
    for t in [Transition::Confirm, Transition::Reject, Transition::Retire] {
        let err = l.transition(&agent(), c.id, t, None).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::NotPermitted));
    }
    assert_eq!(
        l.edit(&agent(), c.id, draft(), None, None)
            .await
            .unwrap_err()
            .code(),
        Some(ErrorCode::NotPermitted)
    );
    assert_eq!(
        l.delete(&agent(), c.id).await.unwrap_err().code(),
        Some(ErrorCode::NotPermitted)
    );

    let confirmed = l
        .transition(&human(), c.id, Transition::Confirm, Some("verified".into()))
        .await
        .unwrap();
    assert_eq!(confirmed.status, LessonStatus::Confirmed);
    assert_eq!(confirmed.confirmed_by, Some(human()));
    let history = l.history(c.id).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].actor, human());
}

#[tokio::test]
async fn scoped_recall_respects_repo_path_and_status() {
    let e = engine();
    let l = e.lessons("t");
    let auth = l
        .create(&human(), draft(), LessonStatus::Confirmed, None)
        .await
        .unwrap();

    let found = l
        .recall(&query(ctx("acme/api", &["src/auth/session.rs"])))
        .await
        .unwrap();
    assert_eq!(ids(&found), vec![auth.id]);
    assert!(l
        .recall(&query(ctx("acme/web", &["src/auth/session.rs"])))
        .await
        .unwrap()
        .is_empty());
    assert!(l
        .recall(&query(ctx("acme/api", &["src/billing/x.rs"])))
        .await
        .unwrap()
        .is_empty());

    // No scope information: nothing, unless global lessons are asked for.
    assert!(l
        .recall(&query(TaskContext::default()))
        .await
        .unwrap()
        .is_empty());

    // Candidates only on opt-in; retired never.
    let cand = l
        .create(&agent(), draft(), LessonStatus::Candidate, None)
        .await
        .unwrap();
    let mut q = query(ctx("acme/api", &["src/auth/a.rs"]));
    assert_eq!(ids(&l.recall(&q).await.unwrap()), vec![auth.id]);
    q.include_candidates = true;
    assert_eq!(l.recall(&q).await.unwrap().len(), 2);

    l.transition(&human(), auth.id, Transition::Retire, None)
        .await
        .unwrap();
    l.transition(&human(), cand.id, Transition::Reject, None)
        .await
        .unwrap();
    assert!(l.recall(&q).await.unwrap().is_empty());
}

#[tokio::test]
async fn global_lessons_need_an_explicit_opt_in() {
    let e = engine();
    let l = e.lessons("t");
    let mut d = draft();
    d.scope = LessonScope {
        global: true,
        ..Default::default()
    };
    let g = l
        .create(&human(), d, LessonStatus::Confirmed, None)
        .await
        .unwrap();
    assert!(l
        .recall(&query(ctx("acme/api", &[])))
        .await
        .unwrap()
        .is_empty());
    let mut q = query(TaskContext::default());
    q.include_global = true;
    assert_eq!(ids(&l.recall(&q).await.unwrap()), vec![g.id]);
}

#[tokio::test]
async fn recall_is_capped_and_stable() {
    let e = engine();
    let l = e.lessons("t");
    for i in 0..8 {
        let mut d = draft();
        d.title = format!("lesson {i}");
        l.create(
            &human(),
            d,
            LessonStatus::Confirmed,
            Some(vec![1.0, i as f32, 0.0]),
        )
        .await
        .unwrap();
    }
    let mut q = query(ctx("acme/api", &["src/auth/a.rs"]));
    q.limit = Some(3);
    q.text = Some("lesson clock".into());
    q.embedding = Some(vec![1.0, 2.0, 0.0]);
    let first = ids(&l.recall(&q).await.unwrap());
    assert_eq!(first.len(), 3);
    assert_eq!(first, ids(&l.recall(&q).await.unwrap()));
    q.limit = Some(500);
    assert_eq!(l.recall(&q).await.unwrap().len(), 8);
}

#[tokio::test]
async fn secrets_are_redacted_before_storage() {
    let e = engine();
    let secret = format!("ghp_{}", "x1Y2".repeat(9));
    let mut d = draft();
    d.evidence[0].text = format!("401 from api using token {secret}");
    d.fix = format!("rotate API_KEY={secret} and retry");
    let l = e.lessons("t");
    let stored = l
        .create(&human(), d, LessonStatus::Candidate, None)
        .await
        .unwrap();
    let raw = serde_json::to_string(&e.advanced().list_memories("t").await.unwrap()).unwrap();
    assert!(!raw.contains(&secret), "secret reached storage");
    assert!(stored.draft.fix.contains(redact::REDACTED));
}

#[tokio::test]
async fn delete_by_repo_removes_only_that_repos_lessons_everywhere() {
    let e = engine();
    let l = e.lessons("t");
    let a = l
        .create(
            &human(),
            draft(),
            LessonStatus::Confirmed,
            Some(vec![1.0, 0.0]),
        )
        .await
        .unwrap();
    let mut other = draft();
    other.scope.repo = Some("acme/web".into());
    let b = l
        .create(
            &human(),
            other,
            LessonStatus::Confirmed,
            Some(vec![0.0, 1.0]),
        )
        .await
        .unwrap();

    assert_eq!(
        l.delete_by_repo(&human(), "acme/api").await.unwrap(),
        vec![a.id]
    );
    assert!(l.get(a.id).await.unwrap().is_none());
    assert!(l.get(b.id).await.unwrap().is_some());
    let hits = e.recall("t", vec![1.0, 0.0], 10).await.unwrap();
    assert!(hits.iter().all(|m| m.id != a.id));
    let history = l.history(a.id).await.unwrap();
    assert_eq!(history.last().unwrap().action, LessonAction::Deleted);
}

#[tokio::test]
async fn delete_never_touches_ordinary_memories() {
    let e = engine();
    let m = e
        .store(
            "t",
            crate::Tier::Semantic,
            "not a lesson".into(),
            None,
            serde_json::json!({}),
            vec![],
        )
        .await
        .unwrap();
    assert!(e
        .lessons("t")
        .delete(&human(), m.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(e.advanced().list_memories("t").await.unwrap().len(), 1);
}

#[tokio::test]
async fn supersede_retires_the_old_lesson_and_links_them() {
    let e = engine();
    let l = e.lessons("t");
    let old = l
        .create(&human(), draft(), LessonStatus::Confirmed, None)
        .await
        .unwrap();
    let new = l
        .create(&human(), draft(), LessonStatus::Confirmed, None)
        .await
        .unwrap();
    let retired = l.supersede(&human(), old.id, new.id, None).await.unwrap();
    assert_eq!(retired.status, LessonStatus::Retired);
    assert_eq!(retired.superseded_by, Some(new.id));
    let linked = e
        .advanced()
        .neighbors("t", new.id, Some(store::REL_SUPERSEDES))
        .await
        .unwrap();
    assert_eq!(linked, vec![old.id]);
    assert!(l.supersede(&agent(), new.id, old.id, None).await.is_err());
}

#[tokio::test]
async fn derived_from_must_exist_and_becomes_an_edge() {
    let e = engine();
    let source = e
        .store(
            "t",
            crate::Tier::Episodic,
            "failing CI run".into(),
            None,
            serde_json::json!({}),
            vec![],
        )
        .await
        .unwrap();
    let l = e.lessons("t");
    let mut d = draft();
    d.derived_from = vec![Uuid::new_v4()];
    assert_eq!(
        l.create(&human(), d.clone(), LessonStatus::Candidate, None)
            .await
            .unwrap_err()
            .code(),
        Some(ErrorCode::InvalidInput)
    );
    d.derived_from = vec![source.id];
    let lesson = l
        .create(&human(), d, LessonStatus::Candidate, None)
        .await
        .unwrap();
    let linked = e
        .advanced()
        .neighbors("t", lesson.id, Some(store::REL_DERIVED_FROM))
        .await
        .unwrap();
    assert_eq!(linked, vec![source.id]);
}

#[tokio::test]
async fn mark_shown_updates_counters_and_render_reports_ids() {
    let e = engine();
    let l = e.lessons("t");
    let a = l
        .create(&human(), draft(), LessonStatus::Confirmed, None)
        .await
        .unwrap();
    let recalled = l.recall(&query(ctx("acme/api", &[]))).await.unwrap();
    let rendered = render_for_agent(&recalled, DEFAULT_MAX_CHARS, chrono::Utc::now());
    l.mark_shown(&rendered.included).await.unwrap();
    let after = l.get(a.id).await.unwrap().unwrap();
    assert_eq!(after.shown_count, 1);
    assert!(after.last_shown_at.is_some());
}
