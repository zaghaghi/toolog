//! A streaming JSONL reader that tolerates being read mid-write.
//!
//! Claude Code appends to transcripts while we read them, so the last line of a
//! file is routinely a fragment. Yielding it would store a truncated record as
//! evidence, and evidence is the one thing that must not be wrong.
//!
//! The reader therefore yields only lines terminated by a newline, and reports
//! the offset just past the last complete one so a tail can resume there.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

/// One complete line and where it started.
#[derive(Debug, Clone)]
pub struct Line {
    /// Byte offset of the line's first character.
    pub offset: i64,
    pub text: String,
}

/// What a read consumed.
#[derive(Debug, Clone, Default)]
pub struct ReadOutcome {
    /// Offset just past the last complete line — where to resume.
    pub next_offset: i64,
    /// Complete lines yielded.
    pub complete: usize,
    /// Whether the file ended with an unterminated fragment.
    pub trailing_partial: bool,
}

/// Read complete lines from `from` onward, calling `f` for each.
///
/// Blank lines are skipped but still advance the offset. A trailing fragment is
/// never yielded, and `next_offset` stops before it so the next pass picks it up
/// once it has been finished.
pub fn read_from<R: Read + Seek>(
    reader: R,
    from: i64,
    f: &mut dyn FnMut(&Line),
) -> std::io::Result<ReadOutcome> {
    let mut reader = BufReader::new(reader);
    reader.seek(SeekFrom::Start(u64::try_from(from).unwrap_or(0)))?;

    let mut outcome = ReadOutcome {
        next_offset: from,
        ..ReadOutcome::default()
    };
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }

        let terminated = buf.last() == Some(&b'\n');
        if !terminated {
            // A fragment. Leave next_offset before it; it will be complete on a
            // later pass.
            outcome.trailing_partial = true;
            break;
        }

        let offset = outcome.next_offset;
        outcome.next_offset += i64::try_from(n).unwrap_or(0);

        let text = String::from_utf8_lossy(&buf[..n - 1]);
        let text = text.trim_end_matches('\r');
        if !text.trim().is_empty() {
            outcome.complete += 1;
            f(&Line {
                offset,
                text: text.to_string(),
            });
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn collect(data: &str, from: i64) -> (Vec<Line>, ReadOutcome) {
        let mut out = Vec::new();
        let outcome = read_from(Cursor::new(data.as_bytes().to_vec()), from, &mut |l| {
            out.push(l.clone());
        })
        .expect("read");
        (out, outcome)
    }

    #[test]
    fn reads_complete_lines_with_offsets() {
        let (lines, outcome) = collect("{\"a\":1}\n{\"b\":2}\n", 0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].offset, 0);
        assert_eq!(lines[1].offset, 8);
        assert_eq!(outcome.next_offset, 16);
        assert!(!outcome.trailing_partial);
    }

    /// The property the whole module exists for.
    #[test]
    fn a_trailing_fragment_is_never_yielded() {
        let (lines, outcome) = collect("{\"a\":1}\n{\"partial\":", 0);
        assert_eq!(lines.len(), 1, "only the terminated line");
        assert!(outcome.trailing_partial);
        assert_eq!(
            outcome.next_offset, 8,
            "resume at the fragment, not past it"
        );
    }

    #[test]
    fn resuming_at_the_fragment_yields_it_once_complete() {
        let (_, first) = collect("{\"a\":1}\n{\"b\"", 0);
        let (lines, second) = collect("{\"a\":1}\n{\"b\":2}\n", first.next_offset);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "{\"b\":2}");
        assert_eq!(second.next_offset, 16);
    }

    #[test]
    fn blank_lines_advance_the_offset_without_being_yielded() {
        let (lines, outcome) = collect("{\"a\":1}\n\n{\"b\":2}\n", 0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].offset, 9);
        assert_eq!(outcome.next_offset, 17);
    }

    #[test]
    fn handles_crlf_and_invalid_utf8_without_failing() {
        let (lines, _) = collect("{\"a\":1}\r\n", 0);
        assert_eq!(lines[0].text, "{\"a\":1}");

        let mut raw = b"{\"a\":\"".to_vec();
        raw.extend_from_slice(&[0xff, 0xfe]);
        raw.extend_from_slice(b"\"}\n");
        let mut out = Vec::new();
        read_from(Cursor::new(raw), 0, &mut |l| out.push(l.clone())).expect("lossy read");
        assert_eq!(out.len(), 1, "invalid UTF-8 is replaced, not fatal");
    }

    #[test]
    fn reading_past_the_end_yields_nothing() {
        let (lines, outcome) = collect("{\"a\":1}\n", 99);
        assert!(lines.is_empty());
        assert_eq!(outcome.next_offset, 99);
    }
}
