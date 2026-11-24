//! Minimal dependency-version ranges for lesson scope: comma-separated
//! comparators over dotted numeric versions (`>=18, <19`, `^1.4`, `~2.1`,
//! `=3.0.1`, `*`). Pre-release/build suffixes (`-rc.1`, `+sha`) are
//! ignored for comparison.
//!
//! shortcut: no pre-release ordering or `||` alternatives; upgrade to the
//! `semver` crate if lesson scopes start needing them.

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version([u64; 3]);

impl Version {
    pub fn parse(text: &str) -> Result<Self, String> {
        let core = text
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or_default();
        if core.is_empty() {
            return Err(format!("empty version in {text:?}"));
        }
        let mut parts = [0u64; 3];
        for (i, part) in core.split('.').enumerate() {
            if i >= 3 {
                return Err(format!("version {text:?} has more than three parts"));
            }
            parts[i] = part
                .parse()
                .map_err(|_| format!("version {text:?} isn't dotted numbers"))?;
        }
        Ok(Version(parts))
    }

    /// How many dotted parts were written, so `^1` and `^1.2` can bump the
    /// right component.
    fn written_parts(text: &str) -> usize {
        text.trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRange(Vec<(Op, Version)>);

impl VersionRange {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut bounds = Vec::new();
        for raw in text.split(',') {
            let item = raw.trim();
            if item.is_empty() {
                return Err(format!("empty comparator in {text:?}"));
            }
            if item == "*" {
                continue;
            }
            bounds.extend(Self::parse_comparator(item)?);
        }
        Ok(VersionRange(bounds))
    }

    fn parse_comparator(item: &str) -> Result<Vec<(Op, Version)>, String> {
        let (op, rest) = [
            (">=", Some(Op::Ge)),
            ("<=", Some(Op::Le)),
            (">", Some(Op::Gt)),
            ("<", Some(Op::Lt)),
            ("=", Some(Op::Eq)),
            ("^", None),
            ("~", None),
        ]
        .into_iter()
        .find_map(|(prefix, op)| {
            item.strip_prefix(prefix)
                .map(|rest| (Some((prefix, op)), rest))
        })
        .unwrap_or((None, item));
        let version = Version::parse(rest)?;
        Ok(match op {
            Some((_, Some(op))) => vec![(op, version)],
            // Bare version: exact match.
            None => vec![(Op::Eq, version)],
            Some((prefix, None)) => {
                let [major, minor, _] = version.0;
                let parts = Version::written_parts(rest);
                let upper = if prefix == "^" {
                    // Caret: don't change the left-most non-zero component.
                    match (major, minor) {
                        (0, 0) if parts >= 3 => Version([0, 0, version.0[2] + 1]),
                        (0, _) if parts >= 2 => Version([0, minor + 1, 0]),
                        _ => Version([major + 1, 0, 0]),
                    }
                } else if parts >= 2 {
                    // Tilde: patch-level changes only.
                    Version([major, minor + 1, 0])
                } else {
                    Version([major + 1, 0, 0])
                };
                vec![(Op::Ge, version), (Op::Lt, upper)]
            }
        })
    }

    pub fn contains(&self, version: Version) -> bool {
        self.0.iter().all(|(op, bound)| {
            let ord = version.cmp(bound);
            match op {
                Op::Eq => ord == Ordering::Equal,
                Op::Gt => ord == Ordering::Greater,
                Op::Ge => ord != Ordering::Less,
                Op::Lt => ord == Ordering::Less,
                Op::Le => ord != Ordering::Greater,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str) -> VersionRange {
        VersionRange::parse(text).unwrap()
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn comparators_bound_both_ends() {
        let range = r(">=18, <19");
        assert!(range.contains(v("18.0.0")));
        assert!(range.contains(v("18.2.1")));
        assert!(!range.contains(v("19.0.0")));
        assert!(!range.contains(v("17.9")));
    }

    #[test]
    fn caret_and_tilde_follow_the_usual_rules() {
        assert!(r("^1.4").contains(v("1.9.0")));
        assert!(!r("^1.4").contains(v("2.0.0")));
        assert!(r("^0.3").contains(v("0.3.9")));
        assert!(!r("^0.3").contains(v("0.4.0")));
        assert!(r("~2.1").contains(v("2.1.7")));
        assert!(!r("~2.1").contains(v("2.2.0")));
    }

    #[test]
    fn bare_version_is_exact_and_star_is_anything() {
        assert!(r("3.0.1").contains(v("v3.0.1")));
        assert!(!r("3.0.1").contains(v("3.0.2")));
        assert!(r("*").contains(v("42")));
    }

    #[test]
    fn suffixes_are_ignored_and_garbage_is_rejected() {
        assert_eq!(v("1.2.3-rc.1+abc"), v("1.2.3"));
        assert!(VersionRange::parse(">= banana").is_err());
        assert!(VersionRange::parse("1.2.3.4").is_err());
        assert!(VersionRange::parse(">=1,").is_err());
    }
}
