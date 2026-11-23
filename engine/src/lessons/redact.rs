//! Secret redaction applied to every lesson text field before it is stored
//! (and again by exporters). Lessons are built from error output, logs and
//! diffs — exactly where tokens leak — so this runs on every create and
//! edit, whichever interface the lesson came through.
//!
//! Pattern-based: it catches the common credential shapes below, not every
//! possible secret. Treat it as a safety net, not a guarantee.

use std::sync::LazyLock;

use regex::Regex;

use super::model::LessonDraft;

pub const REDACTED: &str = "[REDACTED]";

/// Whole-match patterns: the entire match is replaced.
static WHOLE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // PEM private key blocks (any key type), across lines.
        r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
        // AWS access key ids.
        r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        // GitHub tokens (classic and fine-grained).
        r"\bgh[pousr]_[A-Za-z0-9]{30,}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{40,}\b",
        // Anthropic / OpenAI-style API keys.
        r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_\-]{20,}",
        // Slack tokens.
        r"\bxox[abprs]-[A-Za-z0-9\-]{10,}",
        // Google API keys.
        r"\bAIza[0-9A-Za-z_\-]{35}\b",
        // Stripe live/test secret keys.
        r"\b[rs]k_(?:live|test)_[A-Za-z0-9]{16,}\b",
        // JWTs.
        r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        // Credentials embedded in URLs: scheme://user:pass@host
        r"(?i)\b[a-z][a-z0-9+.\-]*://[^\s/:@]+:[^\s/@]+@",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("static redaction pattern"))
    .collect()
});

/// `key = value` / `key: value` / `Bearer value`: keep the label, redact
/// the value (capture group `v`).
static LABELLED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r#"(?i)\b(?:api[_\-]?key|secret|token|password|passwd|pwd|access[_\-]?key|client[_\-]?secret)\b["']?\s*[:=]\s*["']?(?P<v>[^\s"',;]{6,})"#,
        r"(?i)\bbearer\s+(?P<v>[A-Za-z0-9._\-~+/]{16,}=*)",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("static redaction pattern"))
    .collect()
});

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b").expect("email pattern")
});

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Redactor {
    /// Also redact email addresses (off by default: commit authors and
    /// maintainers are often useful context).
    pub redact_emails: bool,
}

impl Redactor {
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for re in WHOLE.iter() {
            out = re.replace_all(&out, REDACTED).into_owned();
        }
        for re in LABELLED.iter() {
            out = re
                .replace_all(&out, |caps: &regex::Captures<'_>| {
                    let whole = caps.get(0).expect("match");
                    let value = caps.name("v").expect("value group");
                    let start = value.start() - whole.start();
                    format!("{}{REDACTED}", &whole.as_str()[..start])
                })
                .into_owned();
        }
        if self.redact_emails {
            out = EMAIL.replace_all(&out, REDACTED).into_owned();
        }
        out
    }

    /// Redacts every free-text field of a draft in place. Scope values
    /// (repo, globs, dependency names) are identifiers, not free text, and
    /// are left alone.
    pub fn redact_draft(&self, draft: &mut LessonDraft) {
        for field in [
            &mut draft.title,
            &mut draft.what_was_tried,
            &mut draft.what_went_wrong,
            &mut draft.fix,
        ] {
            *field = self.redact(field);
        }
        for e in &mut draft.evidence {
            e.text = self.redact(&e.text);
            if let Some(r) = &mut e.reference {
                *r = self.redact(r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str) -> String {
        Redactor::default().redact(text)
    }

    // Fake secrets, assembled at runtime so the source never contains a
    // string that secret scanners would flag.
    fn fake(prefix: &str, body_len: usize) -> String {
        format!(
            "{prefix}{}",
            &"a1B2".repeat(body_len.div_ceil(4))[..body_len]
        )
    }

    #[test]
    fn known_token_shapes_are_redacted() {
        let samples = [
            fake("AKIA", 16).to_uppercase(),
            fake("ghp_", 36),
            fake("github_pat_", 60),
            fake("sk-ant-", 40),
            fake("sk-proj-", 40),
            fake("xoxb-", 30),
            fake("AIza", 35),
            fake("sk_live_", 24),
            format!("{}.{}.{}", fake("eyJ", 20), fake("eyJ", 20), fake("", 20)),
        ];
        for secret in samples {
            let out = r(&format!("error: auth failed with {secret} at line 3"));
            assert!(!out.contains(&secret), "leaked {secret}: {out}");
            assert!(out.contains(REDACTED), "{out}");
            assert!(out.starts_with("error: auth failed with"), "{out}");
        }
    }

    #[test]
    fn private_key_blocks_are_removed_whole() {
        let body = fake("", 64);
        let text = format!(
            "before\n-----BEGIN RSA PRIVATE KEY-----\n{body}\n-----END RSA PRIVATE KEY-----\nafter"
        );
        let out = r(&text);
        assert!(!out.contains(&body));
        assert_eq!(out, format!("before\n{REDACTED}\nafter"));
    }

    #[test]
    fn labelled_values_keep_their_label() {
        let secret = fake("", 24);
        assert_eq!(
            r(&format!("API_KEY={secret}")),
            format!("API_KEY={REDACTED}")
        );
        assert_eq!(
            r(&format!("password: \"{secret}\"")),
            format!("password: \"{REDACTED}\"")
        );
        assert_eq!(
            r(&format!("Authorization: Bearer {secret}")),
            format!("Authorization: Bearer {REDACTED}")
        );
    }

    #[test]
    fn url_credentials_are_redacted() {
        let out = r("postgres://admin:hunter2pass@db.internal:5432/app");
        assert!(!out.contains("hunter2pass"), "{out}");
        assert!(out.contains("db.internal:5432/app"));
    }

    #[test]
    fn ordinary_text_is_untouched_and_emails_are_opt_in() {
        let text = "token expiry test flaked; see src/auth/token.rs:42 (ana@example.com)";
        assert_eq!(r(text), text);
        let out = Redactor {
            redact_emails: true,
        }
        .redact(text);
        assert!(!out.contains("ana@example.com"));
    }
}
