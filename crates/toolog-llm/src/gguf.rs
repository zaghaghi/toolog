//! Reading enough of a GGUF file to refuse one that is not a model (task 13.3).
//!
//! A user points at a path. The file at the end of it is 3 GB of *something*,
//! and handing it to `llama.cpp` to find out is how a wrong path becomes a
//! segfault rather than a sentence. So the header is read here first, in plain
//! Rust: the magic, the version, the architecture and the parameter count.
//!
//! Deliberately **not** `llama_cpp_2::gguf`. This has to work in a build
//! without the `inference` feature — a machine with no C++ toolchain can still
//! be told that the file it was given is a tarball — and it has to be the thing
//! that runs *before* any C++ touches the file, which a wrapper over that same
//! C++ cannot be.
//!
//! Only the metadata is read. The tensor data — all 3 GB of it — is never
//! mapped, so this is a few hundred kilobytes of reading whatever the file's
//! size.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

/// `GGUF`, little-endian, at byte zero.
const MAGIC: [u8; 4] = *b"GGUF";

/// The versions this reader understands. v1 laid out its lengths differently
/// and no model anyone ships is still on it; refusing it is more honest than
/// guessing.
const SUPPORTED_VERSIONS: &[u32] = &[2, 3];

/// A metadata value's type tag, from the GGUF specification.
mod kind {
    pub(super) const UINT8: u32 = 0;
    pub(super) const INT8: u32 = 1;
    pub(super) const UINT16: u32 = 2;
    pub(super) const INT16: u32 = 3;
    pub(super) const UINT32: u32 = 4;
    pub(super) const INT32: u32 = 5;
    pub(super) const FLOAT32: u32 = 6;
    pub(super) const BOOL: u32 = 7;
    pub(super) const STRING: u32 = 8;
    pub(super) const ARRAY: u32 = 9;
    pub(super) const UINT64: u32 = 10;
    pub(super) const INT64: u32 = 11;
    pub(super) const FLOAT64: u32 = 12;
}

/// What is wrong with the file, in words a person can act on.
///
/// Each variant names the thing that was actually observed rather than "invalid
/// model": a user who pointed at the wrong file wants to be told which wrong
/// file it was.
#[derive(Debug, thiserror::Error)]
pub enum GgufError {
    #[error("{path}: cannot be read: {source}")]
    Unreadable {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{path}: not a GGUF model — it starts with {found}, and a GGUF file starts with `GGUF`"
    )]
    NotGguf { path: String, found: String },
    #[error("{path}: GGUF version {version}, and this build reads versions 2 and 3")]
    UnsupportedVersion { path: String, version: u32 },
    #[error("{path}: the GGUF header is truncated or malformed ({what})")]
    Malformed { path: String, what: String },
}

/// What a GGUF file says about itself.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "unused/")]
pub struct ModelFile {
    /// The path as given, so a report can name it.
    pub path: String,
    /// Size on disk, for the Status card's "3.1 GB".
    pub size_bytes: i64,
    pub gguf_version: u32,
    /// `general.architecture` — `gemma4`, `llama`, `qwen3`.
    pub architecture: Option<String>,
    /// `general.name`, when the file carries one.
    pub name: Option<String>,
    /// Summed from the tensor shapes, not read from a key.
    ///
    /// `general.parameter_count` is optional and frequently absent; the tensor
    /// table is neither, and summing it is exact. A file claiming to be a model
    /// and holding no tensors is reported as zero rather than as a model.
    pub parameters: i64,
    pub tensors: i64,
    /// SHA-256 of the whole file. **This is the model's identity** (task 13.14):
    /// two files called `gemma.gguf` are not the same model, and a verdict that
    /// cannot name what produced it is not evidence of anything.
    ///
    /// `None` until [`ModelFile::with_digest`] has done the read — it is three
    /// gigabytes of hashing, which the header inspection deliberately is not.
    pub sha256: Option<String>,
}

impl ModelFile {
    /// A short line for the Status card: `gemma4, 4.6B parameters, 3.1 GB`.
    #[must_use]
    pub fn describe(&self) -> String {
        let arch = self
            .architecture
            .as_deref()
            .unwrap_or("unknown architecture");
        format!(
            "{arch}, {} parameters, {}",
            human_count(self.parameters),
            human_bytes(self.size_bytes)
        )
    }

    /// Hash the file and record it. Reads every byte, so it is the slow half.
    pub fn with_digest(mut self, path: &Path) -> Result<Self, GgufError> {
        self.sha256 = Some(sha256_file(path)?);
        Ok(self)
    }
}

/// Round a parameter count the way model cards do: `4.6B`, `270M`.
///
/// The casts are lossy above 2^53, which is four thousand times more parameters
/// than any model that exists; the rounding this does is coarser than the
/// precision it gives up.
#[expect(
    clippy::cast_precision_loss,
    reason = "a parameter count large enough to lose precision here would not fit \
              on any machine that could load the file"
)]
fn human_count(n: i64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.0}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}K", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// Round a byte count. Powers of 1024, which is what Finder shows a `.gguf` as.
#[expect(
    clippy::cast_precision_loss,
    reason = "the same: a file of 2^53 bytes is nine petabytes"
)]
fn human_bytes(n: i64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// SHA-256 of a file, read in one megabyte at a time.
pub fn sha256_file(path: &Path) -> Result<String, GgufError> {
    let file = File::open(path).map_err(|source| GgufError::Unreadable {
        path: path.display().to_string(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|source| GgufError::Unreadable {
                path: path.display().to_string(),
                source,
            })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Read a GGUF file's header, or say what it is instead.
///
/// Never reads the tensor *data*, only the table describing it.
pub fn inspect(path: &Path) -> Result<ModelFile, GgufError> {
    let display = path.display().to_string();
    let unreadable = |source| GgufError::Unreadable {
        path: display.clone(),
        source,
    };
    let malformed = |what: &str| GgufError::Malformed {
        path: display.clone(),
        what: what.to_string(),
    };

    let file = File::open(path).map_err(unreadable)?;
    let size_bytes = i64::try_from(file.metadata().map_err(unreadable)?.len()).unwrap_or(i64::MAX);
    let mut r = Reader {
        inner: BufReader::new(file),
        path: &display,
    };

    let magic = r.array::<4>()?;
    if magic != MAGIC {
        return Err(GgufError::NotGguf {
            path: display,
            found: describe_magic(magic),
        });
    }

    let gguf_version = r.u32()?;
    if !SUPPORTED_VERSIONS.contains(&gguf_version) {
        return Err(GgufError::UnsupportedVersion {
            path: display,
            version: gguf_version,
        });
    }

    let tensor_count = r.u64()?;
    let kv_count = r.u64()?;
    // A header claiming millions of keys is a corrupt file, and believing it
    // means allocating from a number an attacker controls.
    if kv_count > 1_000_000 || tensor_count > 1_000_000 {
        return Err(malformed(&format!(
            "it declares {kv_count} metadata keys and {tensor_count} tensors"
        )));
    }

    let mut architecture = None;
    let mut name = None;
    for _ in 0..kv_count {
        let key = r.string()?;
        match key.as_str() {
            "general.architecture" => architecture = r.value_as_string()?,
            "general.name" => name = r.value_as_string()?,
            _ => r.skip_value()?,
        }
    }

    // The tensor table: name, shape, type, offset. Summing the shapes is the
    // parameter count, which is a fact about the file rather than a claim it
    // makes about itself.
    let mut parameters: i64 = 0;
    for _ in 0..tensor_count {
        let _name = r.string()?;
        let n_dims = r.u32()?;
        if n_dims > 8 {
            return Err(malformed(&format!("a tensor claims {n_dims} dimensions")));
        }
        let mut elements: i64 = 1;
        for _ in 0..n_dims {
            let dim = i64::try_from(r.u64()?).unwrap_or(i64::MAX);
            elements = elements.saturating_mul(dim);
        }
        let _kind = r.u32()?;
        let _offset = r.u64()?;
        parameters = parameters.saturating_add(elements);
    }

    Ok(ModelFile {
        path: display,
        size_bytes,
        gguf_version,
        architecture,
        name,
        parameters,
        tensors: i64::try_from(tensor_count).unwrap_or(i64::MAX),
        sha256: None,
    })
}

/// Name what the file starts with, for the error that says it is not a model.
///
/// The formats a person actually mistakes for a model get named; anything else
/// is reported as its bytes rather than guessed at.
fn describe_magic(magic: [u8; 4]) -> String {
    match &magic {
        b"\x1f\x8b\x08\x00" | [0x1f, 0x8b, ..] => "a gzip archive".to_string(),
        b"PK\x03\x04" => "a zip archive".to_string(),
        b"\x89PNG" => "a PNG image".to_string(),
        b"\x7fELF" => "an ELF binary".to_string(),
        b"ggml" | b"ggjt" => "a pre-GGUF ggml model, which llama.cpp no longer loads".to_string(),
        _ => {
            let printable: String = magic
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() {
                        char::from(b)
                    } else {
                        '.'
                    }
                })
                .collect();
            format!("`{printable}` ({magic:02x?})")
        }
    }
}

/// A little-endian reader that turns every short read into one error type.
struct Reader<'a> {
    inner: BufReader<File>,
    path: &'a str,
}

impl Reader<'_> {
    fn malformed(&self, what: &str) -> GgufError {
        GgufError::Malformed {
            path: self.path.to_string(),
            what: what.to_string(),
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        let mut buf = [0u8; N];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| self.malformed("it ends part-way through the header"))?;
        Ok(buf)
    }

    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.array::<4>()?))
    }

    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.array::<8>()?))
    }

    /// A length-prefixed UTF-8 string.
    fn string(&mut self) -> Result<String, GgufError> {
        let len = self.u64()?;
        // Long enough for any real key or tensor name, short enough that a
        // corrupt length cannot ask for a gigabyte.
        if len > 1 << 20 {
            return Err(self.malformed(&format!("a string claims to be {len} bytes long")));
        }
        let mut buf = vec![0u8; usize::try_from(len).unwrap_or(0)];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| self.malformed("it ends part-way through a string"))?;
        String::from_utf8(buf).map_err(|_| self.malformed("a metadata string is not UTF-8"))
    }

    /// Read one metadata value and render it as text, when it is one worth
    /// showing. Arrays and floats are skipped rather than stringified.
    fn value_as_string(&mut self) -> Result<Option<String>, GgufError> {
        let kind = self.u32()?;
        if kind == kind::STRING {
            return Ok(Some(self.string()?));
        }
        self.skip_value_of(kind)?;
        Ok(None)
    }

    fn skip_value(&mut self) -> Result<(), GgufError> {
        let kind = self.u32()?;
        self.skip_value_of(kind)
    }

    fn skip_value_of(&mut self, kind: u32) -> Result<(), GgufError> {
        let width = match kind {
            kind::UINT8 | kind::INT8 | kind::BOOL => 1,
            kind::UINT16 | kind::INT16 => 2,
            kind::UINT32 | kind::INT32 | kind::FLOAT32 => 4,
            kind::UINT64 | kind::INT64 | kind::FLOAT64 => 8,
            kind::STRING => {
                self.string()?;
                return Ok(());
            }
            kind::ARRAY => {
                let inner = self.u32()?;
                let len = self.u64()?;
                if inner == kind::STRING {
                    // A tokenizer's vocabulary lives here: hundreds of thousands
                    // of strings, each read and dropped.
                    for _ in 0..len {
                        self.string()?;
                    }
                    return Ok(());
                }
                let width = self.width_of(inner)?;
                self.skip(len.saturating_mul(width))?;
                return Ok(());
            }
            other => {
                return Err(self.malformed(&format!("metadata type {other} is not a GGUF type")));
            }
        };
        self.skip(width)
    }

    fn width_of(&self, kind: u32) -> Result<u64, GgufError> {
        Ok(match kind {
            kind::UINT8 | kind::INT8 | kind::BOOL => 1,
            kind::UINT16 | kind::INT16 => 2,
            kind::UINT32 | kind::INT32 | kind::FLOAT32 => 4,
            kind::UINT64 | kind::INT64 | kind::FLOAT64 => 8,
            other => {
                return Err(self.malformed(&format!(
                    "an array declares element type {other}, which is not a fixed-width GGUF type"
                )));
            }
        })
    }

    fn skip(&mut self, bytes: u64) -> Result<(), GgufError> {
        self.inner
            .seek(SeekFrom::Current(i64::try_from(bytes).unwrap_or(i64::MAX)))
            .map_err(|_| self.malformed("it ends part-way through a metadata value"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Build a GGUF file byte by byte, so the reader is tested against the
    /// format rather than against one file that happens to be on this machine.
    struct Builder {
        bytes: Vec<u8>,
        kv: Vec<u8>,
        kv_count: u64,
        tensors: Vec<u8>,
        tensor_count: u64,
    }

    impl Builder {
        fn new(version: u32) -> Self {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&MAGIC);
            bytes.extend_from_slice(&version.to_le_bytes());
            Self {
                bytes,
                kv: Vec::new(),
                kv_count: 0,
                tensors: Vec::new(),
                tensor_count: 0,
            }
        }

        fn string(out: &mut Vec<u8>, s: &str) {
            out.extend_from_slice(&(s.len() as u64).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }

        fn kv_string(mut self, key: &str, value: &str) -> Self {
            Self::string(&mut self.kv, key);
            self.kv.extend_from_slice(&kind::STRING.to_le_bytes());
            Self::string(&mut self.kv, value);
            self.kv_count += 1;
            self
        }

        fn kv_u32(mut self, key: &str, value: u32) -> Self {
            Self::string(&mut self.kv, key);
            self.kv.extend_from_slice(&kind::UINT32.to_le_bytes());
            self.kv.extend_from_slice(&value.to_le_bytes());
            self.kv_count += 1;
            self
        }

        fn kv_string_array(mut self, key: &str, values: &[&str]) -> Self {
            Self::string(&mut self.kv, key);
            self.kv.extend_from_slice(&kind::ARRAY.to_le_bytes());
            self.kv.extend_from_slice(&kind::STRING.to_le_bytes());
            self.kv
                .extend_from_slice(&(values.len() as u64).to_le_bytes());
            for v in values {
                Self::string(&mut self.kv, v);
            }
            self.kv_count += 1;
            self
        }

        fn kv_u32_array(mut self, key: &str, values: &[u32]) -> Self {
            Self::string(&mut self.kv, key);
            self.kv.extend_from_slice(&kind::ARRAY.to_le_bytes());
            self.kv.extend_from_slice(&kind::UINT32.to_le_bytes());
            self.kv
                .extend_from_slice(&(values.len() as u64).to_le_bytes());
            for v in values {
                self.kv.extend_from_slice(&v.to_le_bytes());
            }
            self.kv_count += 1;
            self
        }

        fn tensor(mut self, name: &str, dims: &[u64]) -> Self {
            Self::string(&mut self.tensors, name);
            self.tensors.extend_from_slice(
                &u32::try_from(dims.len())
                    .expect("a test tensor with few dimensions")
                    .to_le_bytes(),
            );
            for d in dims {
                self.tensors.extend_from_slice(&d.to_le_bytes());
            }
            self.tensors.extend_from_slice(&0u32.to_le_bytes()); // type
            self.tensors.extend_from_slice(&0u64.to_le_bytes()); // offset
            self.tensor_count += 1;
            self
        }

        fn write(self, dir: &Path, name: &str) -> std::path::PathBuf {
            let mut bytes = self.bytes;
            bytes.extend_from_slice(&self.tensor_count.to_le_bytes());
            bytes.extend_from_slice(&self.kv_count.to_le_bytes());
            bytes.extend_from_slice(&self.kv);
            bytes.extend_from_slice(&self.tensors);
            let path = dir.join(name);
            std::fs::File::create(&path)
                .expect("create")
                .write_all(&bytes)
                .expect("write");
            path
        }
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn a_well_formed_header_reports_what_it_holds() {
        let dir = tempdir();
        let path = Builder::new(3)
            .kv_string("general.architecture", "gemma4")
            .kv_string("general.name", "Gemma 4 E2B")
            .kv_u32("gemma4.block_count", 30)
            // The two shapes that dominate a real file, and the two the skipper
            // has to get right or every later offset is wrong.
            .kv_string_array("tokenizer.ggml.tokens", &["<pad>", "<bos>", "hello"])
            .kv_u32_array("tokenizer.ggml.token_type", &[3, 3, 1])
            .tensor("token_embd.weight", &[2048, 262_144])
            .tensor("output_norm.weight", &[2048])
            .write(dir.path(), "model.gguf");

        let info = inspect(&path).expect("a GGUF file");
        assert_eq!(info.architecture.as_deref(), Some("gemma4"));
        assert_eq!(info.name.as_deref(), Some("Gemma 4 E2B"));
        assert_eq!(info.gguf_version, 3);
        assert_eq!(info.tensors, 2);
        assert_eq!(
            info.parameters,
            2048 * 262_144 + 2048,
            "the parameter count is summed from the tensor shapes, not read from a key"
        );
        assert!(
            info.describe().starts_with("gemma4, 537M parameters,"),
            "{}",
            info.describe()
        );
    }

    /// Task 13.3's actual requirement: a sentence, not a segfault.
    #[test]
    fn a_file_that_is_not_a_model_is_named_for_what_it_is() {
        let dir = tempdir();
        let cases: [(&str, &[u8], &str); 4] = [
            ("archive.tar.gz", &[0x1f, 0x8b, 0x08, 0x00, 0x00], "gzip"),
            ("bundle.zip", b"PK\x03\x04....", "zip"),
            ("icon.png", b"\x89PNG\r\n\x1a\n", "PNG"),
            ("notes.txt", b"hello, this is not a model", "`hell`"),
        ];
        for (name, bytes, expected) in cases {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).expect("write");
            let error = inspect(&path).expect_err("not a model");
            let message = error.to_string();
            assert!(
                matches!(error, GgufError::NotGguf { .. }),
                "{name}: {message}"
            );
            assert!(
                message.contains(expected),
                "{name} should be named as {expected}: {message}"
            );
        }
    }

    #[test]
    fn a_pre_gguf_model_is_told_apart_from_junk() {
        let dir = tempdir();
        let path = dir.path().join("old.bin");
        std::fs::write(&path, b"ggml\x00\x00\x00\x00").expect("write");
        let message = inspect(&path).expect_err("refused").to_string();
        assert!(message.contains("pre-GGUF"), "{message}");
    }

    #[test]
    fn a_version_this_build_does_not_read_is_refused_by_number() {
        let dir = tempdir();
        let path = Builder::new(1).write(dir.path(), "v1.gguf");
        match inspect(&path) {
            Err(GgufError::UnsupportedVersion { version, .. }) => assert_eq!(version, 1),
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    /// A truncated file must not be believed, and must not allocate from the
    /// length it claims.
    #[test]
    fn a_truncated_header_is_malformed_rather_than_a_panic() {
        let dir = tempdir();
        let full = Builder::new(3)
            .kv_string("general.architecture", "gemma4")
            .tensor("token_embd.weight", &[2048, 1024])
            .write(dir.path(), "full.gguf");
        let bytes = std::fs::read(&full).expect("read");

        for cut in [4, 8, 16, 24, 32, bytes.len() - 4] {
            let path = dir.path().join(format!("cut{cut}.gguf"));
            std::fs::write(&path, &bytes[..cut]).expect("write");
            let error = inspect(&path).expect_err("truncated");
            assert!(
                matches!(error, GgufError::Malformed { .. }),
                "cut at {cut}: {error}"
            );
        }
    }

    #[test]
    fn a_header_claiming_impossible_counts_is_refused_before_allocating() {
        let dir = tempdir();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&u64::MAX.to_le_bytes()); // tensors
        bytes.extend_from_slice(&u64::MAX.to_le_bytes()); // kv
        let path = dir.path().join("liar.gguf");
        std::fs::write(&path, &bytes).expect("write");
        let message = inspect(&path).expect_err("refused").to_string();
        assert!(message.contains("declares"), "{message}");
    }

    #[test]
    fn a_missing_file_says_so_rather_than_claiming_a_bad_format() {
        let error = inspect(Path::new("/nowhere/at/all.gguf")).expect_err("missing");
        assert!(matches!(error, GgufError::Unreadable { .. }), "{error}");
    }

    /// Task 13.14: the identity is the content, not the name.
    #[test]
    fn two_files_with_the_same_name_hash_differently() {
        let dir = tempdir();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"one").expect("write");
        std::fs::write(&b, b"two").expect("write");
        assert_ne!(
            sha256_file(&a).expect("hash"),
            sha256_file(&b).expect("hash")
        );
        // Against a known vector, so this is testing SHA-256 and not itself.
        assert_eq!(
            sha256_file(&a).expect("hash"),
            "7692c3ad3540bb803c020b3aee66cd8887123234ea0c6e7143c0add73ff431ed"
        );
    }

    #[test]
    fn byte_counts_read_the_way_finder_shows_them() {
        assert_eq!(human_bytes(512), "512 bytes");
        assert_eq!(human_bytes(3_350_000_000), "3.1 GB");
        assert_eq!(human_count(4_630_000_000), "4.6B");
        assert_eq!(human_count(270_000_000), "270M");
    }
}
