//! `z3rno lesson ...`: author, review and recall lessons in a local
//! embedded store.
//!
//! Lessons travel as Markdown files with TOML front matter (`+++`), so a
//! human can review and edit them in any editor and keep them in git:
//!
//! ```text
//! +++
//! title = "Session tokens expire in UTC"
//! [scope]
//! repo = "acme/api"
//! paths = ["src/auth/**"]
//! +++
//! ## What was tried
//! ...
//! ## What went wrong
//! ...
//! ## Fix
//! ...
//! ```
//!
//! Commands that change what agents will be told (confirm, reject, retire,
//! reinstate, edit, supersede, delete, adding or importing as confirmed)
//! need a human: an interactive terminal, or an explicit
//! `--yes-i-am-human`. Without either, `add`/`import` still work but record
//! the lesson as an unconfirmed candidate proposed by an agent.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use z3rno_engine::lessons::{
    render_for_agent, Actor, Evidence, Lesson, LessonDraft, LessonFilter, LessonQuery, LessonScope,
    LessonStatus, Lessons, TaskContext, Transition, DEFAULT_MAX_CHARS,
};
use z3rno_engine::MemoryEngine;
use z3rno_hash_embed::hash_embed;

use crate::{DEFAULT_DB_PATH, DEFAULT_TENANT};

#[derive(Args)]
pub struct LessonArgs {
    #[arg(long, global = true)]
    tenant: Option<String>,

    /// SQLite file holding the lessons.
    #[arg(long, global = true, default_value = DEFAULT_DB_PATH)]
    path: String,

    /// Confirms a human is running this command when stdin isn't a
    /// terminal (scripts, editors). Agents must not pass it.
    #[arg(long, global = true)]
    yes_i_am_human: bool,

    #[command(subcommand)]
    command: LessonCommand,
}

#[derive(Subcommand)]
enum LessonCommand {
    /// Add a lesson from a Markdown file (`-` reads stdin).
    Add {
        file: PathBuf,
        /// Store it already confirmed (human only). Default: candidate.
        #[arg(long)]
        confirm: bool,
    },
    /// List lessons.
    List {
        #[arg(long)]
        status: Option<LessonStatus>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Candidates waiting for review.
    Inbox {
        #[arg(long)]
        repo: Option<String>,
    },
    /// Print one lesson as Markdown (the format `edit` and `import` read).
    Show {
        id: Uuid,
        #[arg(long)]
        json: bool,
        /// Also print its review history.
        #[arg(long)]
        history: bool,
    },
    /// Replace a lesson's content from a Markdown file (human only).
    Edit {
        id: Uuid,
        file: PathBuf,
        #[arg(long)]
        note: Option<String>,
    },
    /// Confirm a candidate (human only).
    Confirm {
        id: Uuid,
        #[arg(long)]
        note: Option<String>,
    },
    /// Reject a candidate as wrong (human only, terminal).
    Reject {
        id: Uuid,
        #[arg(long)]
        note: Option<String>,
    },
    /// Retire a lesson that no longer applies (human only).
    Retire {
        id: Uuid,
        #[arg(long)]
        note: Option<String>,
    },
    /// Bring a retired lesson back (human only).
    Reinstate {
        id: Uuid,
        #[arg(long)]
        note: Option<String>,
    },
    /// Retire OLD in favour of NEW (human only).
    Supersede {
        old: Uuid,
        #[arg(long)]
        by: Uuid,
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete one lesson, or every lesson scoped to a repo (human only).
    Delete {
        #[arg(required_unless_present = "repo", conflicts_with = "repo")]
        id: Option<Uuid>,
        #[arg(long)]
        repo: Option<String>,
    },
    /// Write lessons as Markdown files into a directory.
    Export {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        status: Option<LessonStatus>,
        #[arg(long)]
        repo: Option<String>,
    },
    /// Import Markdown lesson files as new lessons. They come in as
    /// candidates unless `--confirm` is given by a human.
    Import {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        confirm: bool,
    },
    /// Print the lessons that apply to a task, as a delimited block ready
    /// to paste into an agent's context. Prints nothing when none apply.
    Recall {
        #[arg(long)]
        repo: Option<String>,
        /// Paths the task touches (repeatable).
        #[arg(long = "file")]
        files: Vec<String>,
        #[arg(long = "lang")]
        languages: Vec<String>,
        /// Installed dependency as name=version (repeatable).
        #[arg(long = "dep", value_parser = parse_dep)]
        dependencies: Vec<(String, String)>,
        #[arg(long)]
        tool: Option<String>,
        /// What the task is about; improves ranking.
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        include_candidates: bool,
        #[arg(long)]
        include_global: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = DEFAULT_MAX_CHARS)]
        max_chars: usize,
        /// Print ranked results as JSON instead of the rendered block.
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(args: LessonArgs) -> anyhow::Result<()> {
    let tenant = args.tenant.unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let engine = MemoryEngine::embedded(&args.path)?;
    let lessons = engine.lessons(&tenant);
    let gate = HumanGate {
        flag: args.yes_i_am_human,
        terminal: std::io::stdin().is_terminal(),
    };

    match args.command {
        LessonCommand::Add { file, confirm } => {
            let doc = LessonDoc::parse(&read_input(&file)?)?;
            let lesson = create(&lessons, &gate, doc.draft, confirm).await?;
            println!("{}  {}", lesson.id, lesson.status.as_str());
        }
        LessonCommand::List { status, repo, json } => {
            let found = lessons.list(&LessonFilter { status, repo }).await?;
            print_list(&found, json)?;
        }
        LessonCommand::Inbox { repo } => {
            let filter = LessonFilter {
                status: Some(LessonStatus::Candidate),
                repo,
            };
            let found = lessons.list(&filter).await?;
            if found.is_empty() {
                println!("No candidates waiting for review.");
            }
            print_list(&found, false)?;
        }
        LessonCommand::Show { id, json, history } => {
            let lesson = lessons.get(id).await?.context("no lesson with that id")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&lesson)?);
            } else {
                print!("{}", LessonDoc::from_lesson(&lesson).to_markdown()?);
            }
            if history {
                for event in lessons.history(id).await? {
                    println!("{}", serde_json::to_string(&event)?);
                }
            }
        }
        LessonCommand::Edit { id, file, note } => {
            let actor = gate.require("edit")?;
            let draft = lessons.prepare(LessonDoc::parse(&read_input(&file)?)?.draft)?;
            let embedding = hash_embed(&draft.searchable_text());
            let lesson = lessons
                .edit(&actor, id, draft, Some(embedding), note)
                .await?;
            println!("{}  edited", lesson.id);
        }
        LessonCommand::Confirm { id, note } => {
            transition(&lessons, &gate, id, Transition::Confirm, note).await?
        }
        LessonCommand::Reject { id, note } => {
            transition(&lessons, &gate, id, Transition::Reject, note).await?
        }
        LessonCommand::Retire { id, note } => {
            transition(&lessons, &gate, id, Transition::Retire, note).await?
        }
        LessonCommand::Reinstate { id, note } => {
            transition(&lessons, &gate, id, Transition::Reinstate, note).await?
        }
        LessonCommand::Supersede { old, by, note } => {
            let actor = gate.require("supersede")?;
            lessons.supersede(&actor, old, by, note).await?;
            println!("{old}  retired, superseded by {by}");
        }
        LessonCommand::Delete { id, repo } => {
            let actor = gate.require("delete")?;
            match (id, repo) {
                (Some(id), _) => match lessons.delete(&actor, id).await? {
                    Some(proof) => println!("{id}  deleted (audit event {})", proof.audit_event_id),
                    None => println!("No lesson with id {id}."),
                },
                (None, Some(repo)) => {
                    let deleted = lessons.delete_by_repo(&actor, &repo).await?;
                    println!("Deleted {} lesson(s) scoped to {repo}.", deleted.len());
                }
                (None, None) => unreachable!("clap requires an id or --repo"),
            }
        }
        LessonCommand::Export { out, status, repo } => {
            std::fs::create_dir_all(&out)?;
            let found = lessons.list(&LessonFilter { status, repo }).await?;
            for lesson in &found {
                // Redact again on the way out: patterns may have been added
                // since the lesson was stored.
                let mut doc = LessonDoc::from_lesson(lesson);
                doc.draft = lessons.prepare(doc.draft)?;
                let path = out.join(format!("{}.md", lesson.id));
                std::fs::write(&path, doc.to_markdown()?)?;
            }
            println!("Exported {} lesson(s) to {}.", found.len(), out.display());
        }
        LessonCommand::Import { files, confirm } => {
            // Parse, validate and check the human gate first, so a bad file
            // or a missing confirmation imports nothing.
            if confirm {
                gate.require("confirm")?;
            }
            let mut plan = Vec::new();
            for file in &files {
                let doc = LessonDoc::parse(&read_input(file)?)
                    .with_context(|| file.display().to_string())?;
                if matches!(
                    doc.status,
                    Some(LessonStatus::Retired | LessonStatus::Rejected)
                ) {
                    eprintln!("skipping {}: status is not live", file.display());
                    continue;
                }
                let mut draft = lessons
                    .prepare(doc.draft)
                    .with_context(|| file.display().to_string())?;
                if !draft.derived_from.is_empty() {
                    // Those ids belong to the store the file came from.
                    eprintln!("{}: dropping derived_from links", file.display());
                    draft.derived_from.clear();
                }
                // `--confirm` never promotes a file that was exported as a candidate.
                let confirm = confirm && doc.status != Some(LessonStatus::Candidate);
                plan.push((file, draft, confirm));
            }
            for (file, draft, confirm) in plan {
                let lesson = create(&lessons, &gate, draft, confirm).await?;
                println!(
                    "{}  {}  {}",
                    lesson.id,
                    lesson.status.as_str(),
                    file.display()
                );
            }
        }
        LessonCommand::Recall {
            repo,
            files,
            languages,
            dependencies,
            tool,
            text,
            include_candidates,
            include_global,
            limit,
            max_chars,
            json,
        } => {
            let query = LessonQuery {
                context: TaskContext {
                    repo,
                    paths: files,
                    languages,
                    dependencies: dependencies.into_iter().collect::<BTreeMap<_, _>>(),
                    tool,
                },
                embedding: text.as_deref().map(hash_embed),
                text,
                include_candidates,
                include_global,
                limit,
            };
            let recalled = lessons.recall(&query).await?;
            if json {
                let rows: Vec<_> = recalled
                    .iter()
                    .map(|r| serde_json::json!({"lesson": r.lesson, "score": r.score, "warnings": r.warnings}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
                return Ok(());
            }
            let rendered = render_for_agent(&recalled, max_chars, chrono::Utc::now());
            lessons.mark_shown(&rendered.included).await?;
            if !rendered.text.is_empty() {
                println!("{}", rendered.text);
            }
            if rendered.omitted > 0 {
                eprintln!(
                    "{} lesson(s) left out to stay within --max-chars.",
                    rendered.omitted
                );
            }
        }
    }
    Ok(())
}

/// Decides who is running a command. The CLI can't authenticate anyone;
/// it asks for an interactive terminal or an explicit claim, so an agent
/// driving it non-interactively can't confirm its own lessons by accident.
struct HumanGate {
    flag: bool,
    terminal: bool,
}

impl HumanGate {
    fn is_human(&self) -> bool {
        self.flag || self.terminal
    }

    fn require(&self, action: &str) -> anyhow::Result<Actor> {
        if !self.is_human() {
            bail!(
                "{action} needs a human: run it from an interactive terminal, or pass \
                 --yes-i-am-human if you are one"
            );
        }
        Ok(Actor::human(operator_name()))
    }

    /// A human when the gate passes, otherwise an agent: either way the
    /// recorded origin reflects how the command was run.
    fn actor(&self) -> Actor {
        if self.is_human() {
            Actor::human(operator_name())
        } else {
            Actor::agent("z3rno-cli", None)
        }
    }
}

fn operator_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn create(
    lessons: &Lessons<'_>,
    gate: &HumanGate,
    draft: LessonDraft,
    confirm: bool,
) -> anyhow::Result<Lesson> {
    let (actor, status) = if confirm {
        (gate.require("confirm")?, LessonStatus::Confirmed)
    } else {
        (gate.actor(), LessonStatus::Candidate)
    };
    // Embed what will be stored (after redaction), not the raw input.
    let draft = lessons.prepare(draft)?;
    let embedding = hash_embed(&draft.searchable_text());
    Ok(lessons
        .create(&actor, draft, status, Some(embedding))
        .await?)
}

async fn transition(
    lessons: &Lessons<'_>,
    gate: &HumanGate,
    id: Uuid,
    transition: Transition,
    note: Option<String>,
) -> anyhow::Result<()> {
    let actor = gate.require("changing a lesson's status")?;
    let lesson = lessons.transition(&actor, id, transition, note).await?;
    println!("{}  {}", lesson.id, lesson.status.as_str());
    Ok(())
}

fn print_list(lessons: &[Lesson], json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(lessons)?);
        return Ok(());
    }
    for l in lessons {
        let scope = l.draft.scope.repo.as_deref().unwrap_or("(global)");
        println!(
            "{}  {:<9}  {}  {}",
            l.id,
            l.status.as_str(),
            scope,
            l.draft.title
        );
    }
    Ok(())
}

fn read_input(path: &Path) -> anyhow::Result<String> {
    if path == Path::new("-") {
        return Ok(std::io::read_to_string(std::io::stdin())?);
    }
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn parse_dep(raw: &str) -> Result<(String, String), String> {
    raw.split_once('=')
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .filter(|(n, v)| !n.is_empty() && !v.is_empty())
        .ok_or_else(|| format!("expected name=version, got {raw:?}"))
}

/// Front matter of a lesson file. `id`/`status` are written on export for
/// reference and ignored on import (imports are always new lessons).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<LessonStatus>,
    title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    derived_from: Vec<Uuid>,
    scope: LessonScope,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<Evidence>,
}

struct LessonDoc {
    id: Option<Uuid>,
    status: Option<LessonStatus>,
    draft: LessonDraft,
}

const SECTIONS: [&str; 3] = ["What was tried", "What went wrong", "Fix"];

impl LessonDoc {
    fn from_lesson(lesson: &Lesson) -> Self {
        Self {
            id: Some(lesson.id),
            status: Some(lesson.status),
            draft: lesson.draft.clone(),
        }
    }

    fn to_markdown(&self) -> anyhow::Result<String> {
        let d = &self.draft;
        let front = FrontMatter {
            id: self.id,
            status: self.status,
            title: d.title.clone(),
            derived_from: d.derived_from.clone(),
            scope: d.scope.clone(),
            evidence: d.evidence.clone(),
        };
        let mut out = format!("+++\n{}+++\n", toml::to_string(&front)?);
        for (heading, body) in SECTIONS
            .iter()
            .zip([&d.what_was_tried, &d.what_went_wrong, &d.fix])
        {
            out.push_str(&format!("\n## {heading}\n\n{}\n", body.trim()));
        }
        Ok(out)
    }

    fn parse(text: &str) -> anyhow::Result<Self> {
        let rest = text
            .trim_start_matches('\u{feff}')
            .trim_start()
            .strip_prefix("+++")
            .context("a lesson file starts with a +++ TOML front matter block")?;
        let (front, body) = rest
            .split_once("\n+++")
            .context("the front matter block isn't closed with +++")?;
        let front: FrontMatter = toml::from_str(front).context("invalid front matter")?;

        let mut sections: [Option<String>; 3] = Default::default();
        let mut current: Option<usize> = None;
        for line in body.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                let index = SECTIONS
                    .iter()
                    .position(|s| s.eq_ignore_ascii_case(heading.trim()))
                    .with_context(|| {
                        format!("unknown section {heading:?}; expected one of {SECTIONS:?}")
                    })?;
                if sections[index].is_some() {
                    bail!("section {:?} appears twice", SECTIONS[index]);
                }
                sections[index] = Some(String::new());
                current = Some(index);
            } else if let Some(i) = current {
                let s = sections[i].as_mut().expect("current section exists");
                s.push_str(line);
                s.push('\n');
            } else if !line.trim().is_empty() {
                bail!("text before the first ## section: {line:?}");
            }
        }
        let [tried, wrong, fix] = sections.map(|s| s.unwrap_or_default().trim().to_string());
        Ok(Self {
            id: front.id,
            status: front.status,
            draft: LessonDraft {
                title: front.title,
                what_was_tried: tried,
                what_went_wrong: wrong,
                fix,
                evidence: front.evidence,
                scope: front.scope,
                derived_from: front.derived_from,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3rno_engine::lessons::model::{DependencyScope, EvidenceKind};

    const SAMPLE: &str = r#"+++
title = "Session tokens expire in UTC"

[scope]
repo = "acme/api"
paths = ["src/auth/**"]

[[evidence]]
kind = "failing_test"
text = "auth::expiry_test failed at 23:30 local"
+++

## What was tried
Compared expiry against local time.

## What went wrong
Tokens expired an hour early after DST.

## Fix
Compare in UTC.
"#;

    #[test]
    fn parses_a_lesson_file() {
        let doc = LessonDoc::parse(SAMPLE).unwrap();
        let d = doc.draft;
        assert_eq!(d.title, "Session tokens expire in UTC");
        assert_eq!(d.scope.repo.as_deref(), Some("acme/api"));
        assert_eq!(d.evidence[0].kind, EvidenceKind::FailingTest);
        assert_eq!(d.what_went_wrong, "Tokens expired an hour early after DST.");
        assert_eq!(d.fix, "Compare in UTC.");
        d.validate().unwrap();
    }

    #[test]
    fn markdown_round_trips_with_every_scope_field() {
        let mut doc = LessonDoc::parse(SAMPLE).unwrap();
        doc.id = Some(Uuid::new_v4());
        doc.status = Some(LessonStatus::Confirmed);
        doc.draft.scope.dependencies = vec![DependencyScope {
            name: "react".into(),
            range: Some(">=18, <19".into()),
        }];
        doc.draft.scope.tool = Some("cargo".into());
        doc.draft.what_was_tried = "line one\n\nline two".into();
        let text = doc.to_markdown().unwrap();
        let back = LessonDoc::parse(&text).unwrap();
        assert_eq!(back.draft, doc.draft);
        assert_eq!(back.id, doc.id);
        assert_eq!(back.status, doc.status);
    }

    #[test]
    fn malformed_files_are_rejected() {
        assert!(LessonDoc::parse("no front matter").is_err());
        assert!(LessonDoc::parse("+++\ntitle = \"x\"\n").is_err());
        assert!(LessonDoc::parse(&SAMPLE.replace("## Fix", "## Notes")).is_err());
        assert!(LessonDoc::parse(&SAMPLE.replace("title =", "titel =")).is_err());
        assert!(LessonDoc::parse(&SAMPLE.replace("## Fix", "## What went wrong")).is_err());
    }

    #[test]
    fn the_gate_needs_a_terminal_or_the_flag() {
        let agent = HumanGate {
            flag: false,
            terminal: false,
        };
        assert!(agent.require("confirm").is_err());
        assert!(!agent.actor().is_human());
        for gate in [
            HumanGate {
                flag: true,
                terminal: false,
            },
            HumanGate {
                flag: false,
                terminal: true,
            },
        ] {
            assert!(gate.require("confirm").unwrap().is_human());
        }
    }

    #[test]
    fn dependency_flags_parse() {
        assert_eq!(
            parse_dep("react=18.2.0").unwrap(),
            ("react".into(), "18.2.0".into())
        );
        assert!(parse_dep("react").is_err());
        assert!(parse_dep("=1").is_err());
    }

    #[tokio::test]
    async fn non_interactive_add_is_a_candidate_and_cannot_confirm() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let engine = MemoryEngine::embedded(db.path()).unwrap();
        let lessons = engine.lessons("t");
        let agent = HumanGate {
            flag: false,
            terminal: false,
        };
        let draft = LessonDoc::parse(SAMPLE).unwrap().draft;
        assert!(create(&lessons, &agent, draft.clone(), true).await.is_err());
        let lesson = create(&lessons, &agent, draft, false).await.unwrap();
        assert_eq!(lesson.status, LessonStatus::Candidate);
        assert!(!lesson.origin.is_human());
        assert!(
            transition(&lessons, &agent, lesson.id, Transition::Confirm, None)
                .await
                .is_err()
        );
    }
}
