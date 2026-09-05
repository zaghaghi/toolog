//! Secret redaction at normalization (task 7.2).
//!
//! Claude Code runs shell commands, and shell commands carry keys. A tool whose
//! whole purpose is to keep a durable record of what ran will, left alone,
//! keep a durable record of every credential that went past. This is the part
//! that stops it.
//!
//! # Where it applies, and where it deliberately does not
//!
//! **The projection is redacted. The evidence is not**, unless the user asks
//! for it (task 7.3, the `redact_evidence` preference). That split follows from
//! [ADR-0004]: `raw_event` is the thing every other table is derived from, so
//! redacting it is irreversible in a way redacting a projection is not — a
//! pattern that turns out to be wrong can be fixed and the projection rebuilt,
//! but only if the original is still there. The cost is stated rather than
//! hidden: with the default, secrets remain in the evidence store on disk.
//!
//! # Over-redaction is the safe direction
//!
//! A row reading `[redacted: env-assignment]` where a variable was merely
//! *named* like a secret loses a little fidelity. A row printing a live key
//! loses the key. The patterns lean towards the first, and because the evidence
//! is intact by default, nothing is lost that cannot be recovered.
//!
//! # Patterns are data
//!
//! Same shape as [`crate::rules`]: a built-in TOML set, and a user file whose
//! ids replace built-ins. A pattern that does not compile is skipped with a
//! warning — a user's typo in a regex must not stop capture, because a tool
//! that stops recording when misconfigured records nothing at all.
//!
//! [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md

use std::borrow::Cow;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// The patterns shipped with the application.
const BUILT_IN: &str = include_str!("redact/default.toml");

/// One pattern, as written in the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub id: String,
    pub title: String,
    pub regex: String,
    /// Which capture group to replace. 0 — the default — is the whole match.
    #[serde(default)]
    pub group: usize,
}

#[derive(Debug, Deserialize)]
struct PatternFile {
    #[serde(default)]
    pattern: Vec<Pattern>,
}

/// A compiled pattern, and the id that names it in the replacement.
#[derive(Debug)]
struct Compiled {
    id: String,
    regex: Regex,
    group: usize,
}

/// The pattern set in force.
#[derive(Debug, Default)]
pub struct Redactor {
    compiled: Vec<Compiled>,
    /// The patterns as written, for the preferences pane.
    declared: Vec<Pattern>,
    /// Patterns that would not compile, by id — reported, never fatal.
    broken: Vec<String>,
}

impl Redactor {
    /// Whether anything at all is being redacted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }

    /// How many patterns are in force.
    #[must_use]
    pub fn len(&self) -> usize {
        self.compiled.len()
    }

    /// The patterns as written, in file order.
    #[must_use]
    pub fn patterns(&self) -> &[Pattern] {
        &self.declared
    }

    /// Ids of patterns whose regex did not compile.
    #[must_use]
    pub fn broken(&self) -> &[String] {
        &self.broken
    }

    /// Redact one string, borrowing it unchanged when nothing matched.
    ///
    /// Every pattern is matched against the **original** input and the
    /// replacements are made in a single pass. Applying them one after another
    /// looked simpler and was wrong: the first replacement leaves
    /// `password=[redacted: password-assignment]` in the text, and the next
    /// pattern reads `[redacted:` as the value of `password=` and redacts it
    /// again. One pass over the original cannot cascade.
    ///
    /// Where two patterns match overlapping text the earlier one wins, so the
    /// order in the file is a precedence order and the specific patterns are
    /// written above the general ones.
    #[must_use]
    pub fn text<'a>(&self, input: &'a str) -> Cow<'a, str> {
        let mut spans: Vec<(usize, usize, &str)> = Vec::new();

        // Markers already in the text are protected, which is what makes this
        // idempotent — and it has to be, because with `redact_evidence` on the
        // projector reads bodies that were redacted on the way in.
        for existing in marker().find_iter(input) {
            spans.push((existing.start(), existing.end(), ""));
        }

        for pattern in &self.compiled {
            for caps in pattern.regex.captures_iter(input) {
                // A pattern naming a group that did not participate falls back
                // to the whole match rather than silently redacting nothing.
                let Some(target) = caps.get(pattern.group).or_else(|| caps.get(0)) else {
                    continue;
                };
                let overlaps = spans
                    .iter()
                    .any(|(start, end, _)| target.start() < *end && *start < target.end());
                if !overlaps {
                    spans.push((target.start(), target.end(), &pattern.id));
                }
            }
        }

        // Only the spans this call is replacing; the protected ones stay as is.
        spans.retain(|(_, _, id)| !id.is_empty());
        if spans.is_empty() {
            return Cow::Borrowed(input);
        }
        spans.sort_unstable_by_key(|(start, _, _)| *start);

        let mut out = String::with_capacity(input.len());
        let mut last = 0;
        for (start, end, id) in spans {
            out.push_str(&input[last..start]);
            out.push_str("[redacted: ");
            out.push_str(id);
            out.push(']');
            last = end;
        }
        out.push_str(&input[last..]);
        Cow::Owned(out)
    }

    /// Redact every string inside a JSON value, keys included in the walk but
    /// never rewritten — a key is a name, and renaming it would change what the
    /// record says was asked for.
    #[must_use]
    pub fn json(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s).into_owned()),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.json(v)).collect()),
            Value::Object(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), self.json(v))).collect())
            }
            other => other.clone(),
        }
    }

    /// [`Redactor::text`] over an optional string, in place.
    pub fn in_place(&self, field: &mut Option<String>) {
        if let Some(text) = field
            && let Cow::Owned(redacted) = self.text(text)
        {
            *field = Some(redacted);
        }
    }
}

/// Matches a replacement this module already made.
///
/// Used to protect it from being matched again — see [`Redactor::text`].
fn marker() -> &'static Regex {
    static MARKER: OnceLock<Regex> = OnceLock::new();
    MARKER.get_or_init(|| {
        Regex::new(r"\[redacted: [a-z0-9-]+\]").expect("a literal pattern, checked by tests")
    })
}

/// Every pattern in force: the built-in set, plus the user's file.
///
/// A user pattern with the same `id` as a built-in **replaces** it, which is how
/// one is switched off (give it a regex that matches nothing) or retuned.
pub fn load(user: Option<&str>) -> Result<Redactor> {
    let mut declared = parse(BUILT_IN)?;
    if let Some(text) = user {
        for pattern in parse(text)? {
            match declared.iter_mut().find(|p| p.id == pattern.id) {
                Some(existing) => *existing = pattern,
                None => declared.push(pattern),
            }
        }
    }
    Ok(compile(declared))
}

/// Nothing redacted. For the preference that turns it off, and for tests that
/// are about something else.
#[must_use]
pub fn none() -> Redactor {
    Redactor::default()
}

fn parse(text: &str) -> Result<Vec<Pattern>> {
    let file: PatternFile = toml::from_str(text).map_err(|e| Error::Rules(e.to_string()))?;
    Ok(file.pattern)
}

fn compile(declared: Vec<Pattern>) -> Redactor {
    let mut compiled = Vec::with_capacity(declared.len());
    let mut broken = Vec::new();

    for pattern in &declared {
        match Regex::new(&pattern.regex) {
            Ok(regex) => compiled.push(Compiled {
                id: pattern.id.clone(),
                regex,
                group: pattern.group,
            }),
            Err(e) => {
                // Never fatal: a tool that stops recording when a pattern has a
                // typo in it records nothing, which is the worse failure.
                tracing::warn!(id = %pattern.id, error = %e, "redaction pattern skipped");
                broken.push(pattern.id.clone());
            }
        }
    }

    Redactor {
        compiled,
        declared,
        broken,
    }
}

/// The process's redactor.
///
/// A process-wide policy rather than a parameter threaded through every
/// normalizer, because that is what it is: which secrets this machine hides
/// does not vary call by call. The application installs it at startup;
/// everything else — tests included — gets the built-in set, so a developer's
/// own `redaction.toml` cannot change what the test suite sees.
static ACTIVE: OnceLock<Redactor> = OnceLock::new();

/// Install the process's redactor. The first call wins; later ones are ignored.
///
/// Returns whether this call was the one that set it, so a caller that cares
/// can say so rather than assume.
pub fn install(redactor: Redactor) -> bool {
    ACTIVE.set(redactor).is_ok()
}

/// Whether the **evidence** is redacted too (task 7.3).
///
/// Off by default, and the default is the interesting half of the decision.
/// [ADR-0004] makes `raw_event` the thing every other table is rebuilt from: a
/// pattern that turns out to be wrong can be fixed and the projection rebuilt,
/// but only if the original is still there. Redacting the evidence gives that
/// up permanently, in exchange for not keeping secrets on disk at all.
///
/// Neither answer is right for everyone, so it is a preference with the
/// trade-off written next to it, not a silent default. It is also
/// **forward-only**: turning it on redacts what arrives next and cannot reach
/// back into records already stored — doing that would mean rewriting the
/// evidence store, which is what breaks the integrity chain.
///
/// An atomic rather than a `OnceLock` because a user can change their mind
/// while the process is running.
///
/// [ADR-0004]: ../../../docs/adr/0004-store-raw-project-normalized.md
static REDACT_EVIDENCE: AtomicBool = AtomicBool::new(false);

/// Turn evidence redaction on or off. See [`REDACT_EVIDENCE`].
pub fn set_evidence_redaction(on: bool) {
    REDACT_EVIDENCE.store(on, Ordering::Relaxed);
}

/// Whether records are redacted before they are stored as evidence.
#[must_use]
pub fn evidence_redaction() -> bool {
    REDACT_EVIDENCE.load(Ordering::Relaxed)
}

/// A record's body as it should be stored: redacted only if asked for.
///
/// Borrowed when nothing changes, which is the ordinary case in both settings.
#[must_use]
pub fn evidence(body: &str) -> Cow<'_, str> {
    if evidence_redaction() {
        active().text(body)
    } else {
        Cow::Borrowed(body)
    }
}

/// The redactor in force, defaulting to the built-in patterns.
pub fn active() -> &'static Redactor {
    ACTIVE.get_or_init(|| {
        load(None).unwrap_or_else(|e| {
            tracing::error!(error = %e, "built-in redaction patterns did not parse");
            Redactor::default()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin() -> Redactor {
        load(None).expect("built-in patterns")
    }

    #[test]
    fn every_built_in_pattern_compiles() {
        let redactor = builtin();
        assert!(
            redactor.broken().is_empty(),
            "these shipped patterns do not compile: {:?}",
            redactor.broken()
        );
        assert!(
            redactor.len() >= 12,
            "the shipped set is {}",
            redactor.len()
        );
    }

    #[test]
    fn a_whole_value_credential_is_replaced_entirely() {
        let r = builtin();
        assert_eq!(
            r.text("aws s3 ls --profile AKIAIOSFODNN7EXAMPLE"),
            "aws s3 ls --profile [redacted: aws-access-key-id]"
        );
        assert!(
            r.text("curl -H 'X: ghp_abcdefghijklmnopqrstuvwxyz0123456789'")
                .contains("[redacted: github-token]")
        );
    }

    /// The point of capture groups: the row still reads as the command it was.
    #[test]
    fn a_value_in_context_loses_the_value_and_keeps_the_name() {
        let r = builtin();
        assert_eq!(
            r.text("psql 'host=db password=hunter2xyz dbname=app'"),
            "psql 'host=db password=[redacted: password-assignment] dbname=app'"
        );
        assert_eq!(
            r.text("export GITHUB_TOKEN=abcd1234efgh5678"),
            "export GITHUB_TOKEN=[redacted: env-assignment]"
        );
        assert_eq!(
            r.text("curl -H 'Authorization: Bearer abcdefghijklmnop'"),
            "curl -H 'Authorization: Bearer [redacted: bearer-token]'"
        );
    }

    #[test]
    fn a_private_key_leaves_its_markers_so_the_row_says_what_was_there() {
        let r = builtin();
        let pem =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nabc\n-----END RSA PRIVATE KEY-----";
        let redacted = r.text(pem);
        assert!(redacted.contains("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(redacted.contains("[redacted: private-key-block]"));
        assert!(!redacted.contains("MIIEowIBAAKCAQEA"));
    }

    #[test]
    fn an_ordinary_command_is_untouched_and_not_even_copied() {
        let r = builtin();
        let plain = "cargo test --workspace && git status";
        assert!(
            matches!(r.text(plain), Cow::Borrowed(_)),
            "no match must not allocate"
        );
    }

    #[test]
    fn redacting_twice_changes_nothing_the_second_time() {
        let r = builtin();
        let once = r.text("export API_KEY=supersecretvalue").into_owned();
        let twice = r.text(&once).into_owned();
        assert_eq!(once, twice, "the marker must not itself be redactable");
    }

    #[test]
    fn json_values_are_redacted_and_keys_are_left_alone() {
        let r = builtin();
        let input = serde_json::json!({
            "command": "curl -H 'Authorization: Bearer abcdefghijklmnop'",
            "AWS_SECRET_ACCESS_KEY": "not a value we match on its own",
            "nested": [{"token": "ghp_abcdefghijklmnopqrstuvwxyz0123456789"}],
            "count": 3,
        });
        let out = r.json(&input);

        assert!(out["command"].as_str().expect("str").contains("[redacted:"));
        assert!(
            out.get("AWS_SECRET_ACCESS_KEY").is_some(),
            "a key is a name; renaming it would change what was asked for"
        );
        assert!(
            out["nested"][0]["token"]
                .as_str()
                .expect("str")
                .starts_with("[redacted:")
        );
        assert_eq!(out["count"], 3, "non-strings pass through");
    }

    #[test]
    fn a_user_pattern_replaces_a_built_in_by_id() {
        let user = r#"
            [[pattern]]
            id = "aws-access-key-id"
            title = "off"
            regex = 'this-will-never-appear-anywhere'
        "#;
        let r = load(Some(user)).expect("load");
        assert_eq!(
            r.text("AKIAIOSFODNN7EXAMPLE"),
            "AKIAIOSFODNN7EXAMPLE",
            "a built-in switched off by a user pattern that matches nothing"
        );
        assert_eq!(r.len(), builtin().len(), "replaced, not added");
    }

    #[test]
    fn a_user_pattern_with_a_new_id_is_added() {
        let user = r#"
            [[pattern]]
            id = "internal-ticket"
            title = "our ticket ids"
            regex = 'ACME-[0-9]{4,}'
        "#;
        let r = load(Some(user)).expect("load");
        assert_eq!(r.len(), builtin().len() + 1);
        assert_eq!(r.text("see ACME-12345"), "see [redacted: internal-ticket]");
    }

    /// A typo in a user's file must not stop capture.
    #[test]
    fn a_pattern_that_does_not_compile_is_skipped_not_fatal() {
        let user = r#"
            [[pattern]]
            id = "broken"
            title = "unbalanced"
            regex = '([unclosed'
        "#;
        let r = load(Some(user)).expect("a bad regex is not a load failure");
        assert_eq!(r.broken(), ["broken"]);
        assert_eq!(r.len(), builtin().len(), "the rest still work");
        assert!(r.text("AKIAIOSFODNN7EXAMPLE").contains("[redacted:"));
    }

    /// Every one of these was a false positive measured against the owner's
    /// real store (`cargo run --example measure_redaction`). They are here so
    /// that tightening one pattern cannot loosen another back onto them.
    #[test]
    fn the_shapes_that_are_not_secrets_are_left_alone() {
        let r = builtin();
        for ordinary in [
            // A command substitution is an expression, not a literal.
            "TOKEN=$(curl -s -X POST http://localhost:3000/host/login)",
            "psql \"host=db password=$PGPASSWORD\"",
            // Source code in a heredoc: an assignment with spaces around it.
            "let token = issue_jwt(host_id_for(&email), \"host\", None, HOST_TTL_SECS);",
            // Type declarations, not values.
            "pub struct LoginReq { pub email: String, pub password: String }",
            "export async function login(email: string, password: string): Promise<void> {",
            // Prose about an API, with no token in it.
            "The QR is sent as `Authorization: Bearer`. Design tokens live in src/index.css",
            // A React component about password fields.
            "autoComplete={isNewPassword ? \"new-password\" : \"current-password\"}",
            "{mode === \"reset\" ? \"New password\" : \"Password\"}",
        ] {
            assert_eq!(
                r.text(ordinary),
                ordinary,
                "redacted something that is not a secret"
            );
        }
    }

    /// And the shapes that *are*, so tightening never goes too far.
    #[test]
    fn the_shapes_that_are_secrets_are_still_caught() {
        let r = builtin();
        for (input, expected) in [
            (
                "cd apps/api && JWT_SECRET=devsecret cargo run",
                "env-assignment",
            ),
            ("psql 'host=db password=hunter2xyz'", "password-assignment"),
            (r#"{"password": "hunter2xyz"}"#, "password-field"),
            ("password: \"hunter2xyz\"", "password-field"),
            (
                "curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9abcdefghij'",
                "bearer-token",
            ),
            (
                r#"{"token":"eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJjNmJkNTllMiJ9.abcdefghijk"}"#,
                "jwt",
            ),
            (
                "access_key_id: \"ASIAY5KEW2HNAWNUAHNM\"",
                "aws-access-key-id",
            ),
        ] {
            let redacted = r.text(input);
            assert!(
                redacted.contains(&format!("[redacted: {expected}]")),
                "{input}\n  → {redacted}\n  expected {expected}"
            );
        }
    }

    #[test]
    fn a_malformed_file_is_an_error_rather_than_a_panic() {
        assert!(load(Some("[[pattern]]\nid = ")).is_err());
    }

    /// Task 7.3's switch, and the default that matters most.
    ///
    /// This toggles process-wide state and puts it back. Nothing else in this
    /// binary is affected by a momentary overlap: the bodies the other tests
    /// store — `{"a":1}` and the like — match no pattern in either setting.
    #[test]
    fn the_evidence_is_not_redacted_unless_it_is_asked_for() {
        // The process default. Asserted rather than assumed, because "off by
        // default" is the whole claim PRIVACY.md makes about this.
        assert!(!evidence_redaction());
        assert_eq!(
            evidence("export API_KEY=supersecretvalue"),
            "export API_KEY=supersecretvalue"
        );

        set_evidence_redaction(true);
        assert!(
            evidence("export API_KEY=supersecretvalue").contains("[redacted:"),
            "with the switch on, a record is redacted before it is stored"
        );
        set_evidence_redaction(false);
        assert!(!evidence_redaction(), "and back, for the tests that follow");
    }

    #[test]
    fn the_empty_redactor_changes_nothing() {
        let r = none();
        assert!(r.is_empty());
        assert_eq!(
            r.text("export API_KEY=secretvalue"),
            "export API_KEY=secretvalue"
        );
    }
}
