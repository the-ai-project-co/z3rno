//! Does a lesson apply to the task at hand? Pure functions over a lesson's
//! scope and the caller's `TaskContext`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::model::LessonScope;
use super::version::{Version, VersionRange};

/// What the caller is about to work on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContext {
    pub repo: Option<String>,
    /// Paths relative to the repo root that the task touches.
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    /// Dependency name -> installed version.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub tool: Option<String>,
}

/// Why a lesson matched; drives specificity ranking and the "why shown"
/// line in rendered output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applicability {
    pub repo_match: bool,
    pub path_match: bool,
    pub language_match: bool,
    pub dependency_match: bool,
    pub tool_match: bool,
}

impl Applicability {
    /// Higher means the lesson was scoped more narrowly to this task.
    pub fn specificity(&self) -> u32 {
        u32::from(self.repo_match)
            + 3 * u32::from(self.path_match)
            + u32::from(self.language_match)
            + 2 * u32::from(self.dependency_match)
            + u32::from(self.tool_match)
    }
}

/// `Some` if the lesson applies to `ctx`, `None` if any declared scope
/// dimension rules it out. Dimensions the caller didn't describe don't
/// exclude (the lesson is just less specific); dimensions the caller did
/// describe must overlap.
pub fn applicability(scope: &LessonScope, ctx: &TaskContext) -> Option<Applicability> {
    let mut a = Applicability::default();

    if !scope.global {
        let repo = scope.repo.as_deref()?;
        if ctx.repo.as_deref().map(normalize_repo) != Some(normalize_repo(repo)) {
            return None;
        }
        a.repo_match = true;
    }

    if !scope.paths.is_empty() && !ctx.paths.is_empty() {
        let hit = ctx.paths.iter().any(|path| {
            let path = normalize_path(path);
            scope
                .paths
                .iter()
                .any(|pattern| glob_match(&normalize_path(pattern), &path))
        });
        if !hit {
            return None;
        }
        a.path_match = true;
    }

    if !scope.languages.is_empty() && !ctx.languages.is_empty() {
        let hit = scope
            .languages
            .iter()
            .any(|l| ctx.languages.iter().any(|c| c.eq_ignore_ascii_case(l)));
        if !hit {
            return None;
        }
        a.language_match = true;
    }

    for dep in &scope.dependencies {
        let Some(installed) = ctx.dependencies.get(&dep.name) else {
            continue;
        };
        a.dependency_match = true;
        let (Some(range), Ok(version)) = (&dep.range, Version::parse(installed)) else {
            continue;
        };
        // Validation guarantees the range parses; an unparseable installed
        // version can't be ruled out, so it stays applicable.
        if VersionRange::parse(range).is_ok_and(|r| !r.contains(version)) {
            return None;
        }
    }

    if let (Some(tool), Some(ctx_tool)) = (&scope.tool, &ctx.tool) {
        if !tool.eq_ignore_ascii_case(ctx_tool) {
            return None;
        }
        a.tool_match = true;
    }

    Some(a)
}

fn normalize_repo(repo: &str) -> &str {
    repo.trim().trim_end_matches('/').trim_end_matches(".git")
}

fn normalize_path(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

/// Glob over `/`-separated paths: `*` matches within one segment, `**`
/// matches any number of whole segments (including none), `?` matches one
/// character other than `/`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&pattern, &path)
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| match_segments(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((segment, path_rest)) => {
                match_segment(first.as_bytes(), segment.as_bytes())
                    && match_segments(rest, path_rest)
            }
            None => false,
        },
    }
}

fn match_segment(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skip| match_segment(rest, &text[skip..])),
        Some((b'?', rest)) => !text.is_empty() && match_segment(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && match_segment(rest, &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::model::DependencyScope;

    fn scope() -> LessonScope {
        LessonScope {
            repo: Some("acme/api".into()),
            paths: vec!["src/auth/**".into()],
            ..Default::default()
        }
    }

    fn ctx(repo: &str, paths: &[&str]) -> TaskContext {
        TaskContext {
            repo: Some(repo.into()),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn glob_semantics() {
        assert!(glob_match("src/auth/**", "src/auth/token.rs"));
        assert!(glob_match("src/auth/**", "src/auth/deep/er/x.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b.rs"));
        assert!(glob_match("src/**/*.rs", "src/b.rs"));
        assert!(!glob_match("src/*.rs", "src/a/b.rs"));
        assert!(glob_match("Cargo.???l", "Cargo.toml"));
        assert!(!glob_match("src/auth/**", "src/billing/x.rs"));
    }

    #[test]
    fn repo_and_path_scope_selects_the_right_task() {
        let s = scope();
        assert!(applicability(&s, &ctx("acme/api", &["src/auth/login.rs"])).is_some());
        assert!(applicability(&s, &ctx("acme/web", &["src/auth/login.rs"])).is_none());
        assert!(applicability(&s, &ctx("acme/api", &["src/billing/x.rs"])).is_none());
        let a = applicability(&s, &ctx("acme/api.git/", &["./src/auth/x.rs"])).unwrap();
        assert!(a.repo_match && a.path_match);
    }

    #[test]
    fn no_repo_in_context_matches_only_global_lessons() {
        let none = TaskContext::default();
        assert!(applicability(&scope(), &none).is_none());
        let global = LessonScope {
            global: true,
            ..Default::default()
        };
        assert!(applicability(&global, &none).is_some());
    }

    #[test]
    fn dependency_outside_range_excludes() {
        let mut s = scope();
        s.dependencies = vec![DependencyScope {
            name: "react".into(),
            range: Some(">=18, <19".into()),
        }];
        let mut c = ctx("acme/api", &[]);
        c.dependencies.insert("react".into(), "18.2.0".into());
        assert!(applicability(&s, &c).unwrap().dependency_match);
        c.dependencies.insert("react".into(), "19.0.0".into());
        assert!(applicability(&s, &c).is_none());
    }

    #[test]
    fn more_specific_scopes_rank_higher() {
        let narrow = applicability(&scope(), &ctx("acme/api", &["src/auth/a.rs"])).unwrap();
        let broad_scope = LessonScope {
            repo: Some("acme/api".into()),
            ..Default::default()
        };
        let broad = applicability(&broad_scope, &ctx("acme/api", &["src/auth/a.rs"])).unwrap();
        assert!(narrow.specificity() > broad.specificity());
    }
}
