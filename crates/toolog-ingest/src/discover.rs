//! Finding transcripts on disk — and not finding the ones a user has excluded.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// `~/.claude/projects`, where Claude Code keeps session transcripts.
#[must_use]
pub fn projects_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|d| d.home_dir().join(".claude").join("projects"))
}

/// Projects the user has asked never to capture (task 7.8).
///
/// A process-wide policy, like redaction: which projects this machine records
/// does not vary by caller. Enforced at *discovery*, which is the only place it
/// can honestly be enforced — an excluded project's transcript is never opened,
/// so nothing from it is ever stored, and there is no evidence to purge later.
///
/// Matched on Claude Code's own directory encoding rather than on a decoded
/// path. `project_path_hint` is lossy — `/a/b/claude-code-tools-log` and
/// `/a/b/claude/code/tools/log` encode identically — so comparing decoded
/// paths would exclude the wrong project. Encoding the *exclusion* instead is
/// exact in the direction that matters. Two paths differing only by `-` versus
/// `/` still collide; nothing can tell them apart from the directory name
/// alone, and Claude Code has the same limitation.
static EXCLUDED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Replace the exclusion list. Empty means capture everything.
pub fn set_excluded(projects: Vec<String>) {
    if let Ok(mut list) = EXCLUDED.write() {
        *list = projects
            .into_iter()
            .map(|p| encode(p.trim_end_matches('/')))
            .filter(|p| !p.is_empty())
            .collect();
    }
}

/// The exclusion list as Claude Code would name those directories.
#[must_use]
pub fn excluded() -> Vec<String> {
    EXCLUDED.read().map_or_else(|_| Vec::new(), |l| l.clone())
}

/// `/Users/x/Projects/app` as Claude Code writes it: `-Users-x-Projects-app`.
fn encode(project_path: &str) -> String {
    project_path.replace('/', "-")
}

/// Whether a transcript belongs to an excluded project.
#[must_use]
pub fn is_excluded(transcript: &Path) -> bool {
    let Some(dir) = transcript
        .parent()
        .and_then(Path::file_name)
        .and_then(|d| d.to_str())
    else {
        return false;
    };
    EXCLUDED
        .read()
        .is_ok_and(|list| list.iter().any(|e| e == dir))
}

/// Every `*.jsonl` transcript under `root`, sorted for reproducible ordering.
///
/// Excluded projects are absent, which is what makes exclusion mean "never
/// captured" rather than "captured and then hidden".
#[must_use]
pub fn transcripts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") && !is_excluded(&path) {
                out.push(path);
            }
        }
    }

    out.sort();
    out
}

/// Best-effort project path from a transcript's directory name.
///
/// Claude Code encodes `/Users/x/Projects/app` as `-Users-x-Projects-app`, which
/// is lossy — a project whose own name contains a dash cannot be recovered. Use
/// it only as a fallback; records carry the real `cwd`, and that is what the
/// projector actually stores.
#[must_use]
pub fn project_path_hint(transcript: &Path) -> Option<String> {
    let dir = transcript.parent()?.file_name()?.to_str()?;
    dir.starts_with('-').then(|| dir.replace('-', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_transcripts_recursively_and_in_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("-Users-x-alpha");
        let b = tmp.path().join("-Users-x-beta");
        std::fs::create_dir_all(&a).expect("mkdir");
        std::fs::create_dir_all(&b).expect("mkdir");
        std::fs::write(a.join("s1.jsonl"), "{}").expect("write");
        std::fs::write(b.join("s2.jsonl"), "{}").expect("write");
        std::fs::write(a.join("notes.txt"), "ignored").expect("write");

        let found = transcripts(tmp.path());
        assert_eq!(found.len(), 2, "only .jsonl files: {found:?}");
        assert!(found[0] < found[1], "sorted for reproducible ordering");
    }

    #[test]
    fn missing_directories_yield_nothing_rather_than_failing() {
        assert!(transcripts(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn project_hint_decodes_the_encoded_directory_name() {
        let p = Path::new("/root/-Users-x-Projects-app/s.jsonl");
        assert_eq!(
            project_path_hint(p).as_deref(),
            Some("/Users/x/Projects/app")
        );
    }

    /// Documents the lossiness rather than pretending it away: this is why the
    /// projector uses `cwd` from the records instead.
    #[test]
    fn project_hint_is_lossy_for_names_containing_dashes() {
        let p = Path::new("/root/-Users-x-my-app/s.jsonl");
        assert_eq!(project_path_hint(p).as_deref(), Some("/Users/x/my/app"));
    }

    /// The exclusion policy is process-wide by design, so the tests that set
    /// it cannot run alongside one another: cargo runs them on threads of a
    /// single process, and whichever finishes first calls `set_excluded(vec![])`
    /// and clears the list out from under the others. That made this suite fail
    /// about one run in three, on a test that looks deterministic.
    ///
    /// Serialized rather than made per-caller: the policy really is global —
    /// see [`set_excluded`] — and a test-only knob would be testing something
    /// other than the thing that ships.
    static POLICY: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Hold the exclusion policy for the duration of one test.
    ///
    /// Recovers from poisoning: one panicking test should fail alone, not take
    /// every other test of this policy down with it.
    fn exclusively() -> std::sync::MutexGuard<'static, ()> {
        POLICY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Task 7.8: an excluded project is never opened, so nothing from it is
    /// ever stored — which is a stronger claim than hiding it from a view.
    #[test]
    fn an_excluded_project_is_not_discovered() {
        let _policy = exclusively();
        let tmp = tempfile::tempdir().expect("tempdir");
        let kept = tmp.path().join("-Users-x-Projects-keep");
        let dropped = tmp.path().join("-Users-x-Projects-secret");
        std::fs::create_dir_all(&kept).expect("mkdir");
        std::fs::create_dir_all(&dropped).expect("mkdir");
        std::fs::write(kept.join("a.jsonl"), "{}").expect("write");
        std::fs::write(dropped.join("b.jsonl"), "{}").expect("write");

        set_excluded(vec!["/Users/x/Projects/secret".to_string()]);
        let found = transcripts(tmp.path());
        set_excluded(Vec::new());

        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ends_with("a.jsonl"));
    }

    /// The reason exclusion matches the *encoded* directory name rather than a
    /// decoded path: decoding cannot tell these two apart.
    #[test]
    fn a_project_whose_name_contains_a_dash_is_matched_exactly() {
        let _policy = exclusively();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dashed = tmp.path().join("-Users-x-claude-code-tools-log");
        std::fs::create_dir_all(&dashed).expect("mkdir");
        std::fs::write(dashed.join("s.jsonl"), "{}").expect("write");

        // The lossy hint would call this `/Users/x/claude/code/tools/log`.
        assert_eq!(
            project_path_hint(&dashed.join("s.jsonl")).as_deref(),
            Some("/Users/x/claude/code/tools/log")
        );

        set_excluded(vec!["/Users/x/claude-code-tools-log".to_string()]);
        let found = transcripts(tmp.path());
        set_excluded(Vec::new());
        assert!(found.is_empty(), "exclusion must match the real path");
    }

    #[test]
    fn an_empty_exclusion_list_captures_everything() {
        let _policy = exclusively();
        set_excluded(Vec::new());
        assert!(excluded().is_empty());
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("-Users-x-anything");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("s.jsonl"), "{}").expect("write");
        assert_eq!(transcripts(tmp.path()).len(), 1);
    }
}
