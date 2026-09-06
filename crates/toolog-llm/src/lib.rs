//! A local model over the tool calls no rule matched (Phase 13, [ADR-0013]).
//!
//! Twelve rules find what someone thought to write a rule for. On the owner's
//! store that leaves **3,618 Bash commands — 77% — that no rule has ever
//! matched**, reported as nothing rather than as unexamined. This crate reads
//! them, on this machine, and says what each was trying to do.
//!
//! # Four things it is not
//!
//! - **It is not a network client.** [ADR-0008] is the tool's central claim and
//!   `toolog-cli/tests/egress.rs` enforces it structurally. A 3.1 GB fetch from
//!   Hugging Face is exactly the thing that test exists to forbid, so the user
//!   brings the `.gguf` and toolog is pointed at a path. That extends to the C++:
//!   llama.cpp has a `LLAMA_CURL` option for its own downloader that a manifest
//!   check cannot see, so the shipped binary is asserted to link no `libcurl` —
//!   `just verify-bundle`, and Phase 8's lesson that a config option is not a
//!   guarantee.
//! - **It is not a rule.** Nothing here changes a severity, a finding, or the
//!   risk summary's numbers, and no verdict is ever compiled into anything. A
//!   non-deterministic judge cannot be what an audit trail asserts.
//! - **It is not reproducible**, which is why a verdict is stored rather than
//!   recomputed ([ADR-0013]) and why it is keyed on a fingerprint of the model
//!   file *and* the prompt template.
//! - **It is not always right.** A small quantized model scores a benign `find`
//!   at 4 and a clever destructive one-liner at 2. That is the trade, it is
//!   stated in the README rather than in a footnote, and it is why the intent
//!   summary — useful even when the score is not — may be the half worth keeping.
//!
//! # The layout
//!
//! [`gguf`] refuses a file that is not a model, before any C++ touches it.
//! [`prompt`] is the versioned template and the delimited block the audited
//! command goes inside. [`verdict`] is the schema an answer has to satisfy.
//! [`engine`] is the one thread that owns every llama.cpp object.
//!
//! The first three are plain Rust and compile without the `inference` feature,
//! which is where most of what can go wrong lives and is what keeps this crate
//! checkable on a machine with no C++ toolchain.
//!
//! [ADR-0008]: ../../../docs/adr/0008-local-only-zero-egress.md
//! [ADR-0013]: ../../../docs/adr/0013-a-verdict-is-stored-not-recomputed.md

pub mod gguf;
pub mod prompt;
pub mod verdict;

#[cfg(feature = "inference")]
pub mod engine;

pub use gguf::{GgufError, ModelFile};
pub use prompt::Prompt;
pub use verdict::Verdict;

/// Whether this build can actually run a model.
///
/// The `inference` feature is on by default; a build without it exists so the
/// crate stays checkable where llama.cpp cannot be built, and so task 13.19's
/// fallback is a thing that has been compiled rather than a thing that was
/// imagined. The window reports it, because "no model configured" and "this
/// build cannot load one" are different answers to the same question.
#[must_use]
pub const fn built_with_inference() -> bool {
    cfg!(feature = "inference")
}

/// Look at a `.gguf` and say what it is: header first, then the hash.
///
/// The header read is cheap and catches the common mistake — a path that points
/// at an archive, a partial download, or nothing. The hash reads every byte and
/// is what a verdict is keyed on (task 13.14), so it is done second and only
/// once the file has proved it is a model.
pub fn inspect_model(path: &std::path::Path) -> Result<ModelFile, GgufError> {
    gguf::inspect(path)?.with_digest(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_build_reports_honestly_whether_it_can_load_a_model() {
        assert_eq!(super::built_with_inference(), cfg!(feature = "inference"));
    }
}
