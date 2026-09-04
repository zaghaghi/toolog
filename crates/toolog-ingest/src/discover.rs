//! Finding transcripts on disk.

use std::path::{Path, PathBuf};

/// `~/.claude/projects`, where Claude Code keeps session transcripts.
#[must_use]
pub fn projects_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|d| d.home_dir().join(".claude").join("projects"))
}

/// Every `*.jsonl` transcript under `root`, sorted for reproducible ordering.
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
            } else if path.extension().is_some_and(|e| e == "jsonl") {
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
}
