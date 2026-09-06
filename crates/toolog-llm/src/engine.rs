//! One thread that owns every llama.cpp object (tasks 13.5, 13.6).
//!
//! `LlamaModel`, `LlamaContext` and `LlamaSampler` are not `Sync`, so they
//! cannot be shared and cannot be handed to a thread pool. The shape that works
//! is a dedicated worker reached over a channel with oneshot replies — the one
//! `project-birthday/src-tauri/src/inference.rs` arrived at, and there is no
//! reason to rediscover it.
//!
//! # Which thread this is not
//!
//! It is **not** the resident process's writer, and **not** either read
//! connection ([ADR-0007]). It computes and hands verdicts to the writer the way
//! a risk review hands it sightings (task 12.3). Inference holds the CPU and the
//! GPU for a second at a time; a store that could not be read while it ran would
//! be a worse tool than one that never had a model.
//!
//! # Two things measured rather than assumed
//!
//! **The prompt prefix is prefilled once and kept.** The instructions are 461
//! tokens and identical on every call. Re-tokenizing and re-decoding them per
//! call cost 22% of the wall clock over the owner's store (1,377 ms → 1,070 ms
//! mean). So the prefix is decoded at load, and each call clears the KV cache
//! from the end of the prefix rather than from zero. The cost is that the
//! command is tokenized as its own sequence, which is a *different tokenization*
//! from the whole prompt at once — and that changes verdicts at the margin. It
//! is one more reason a verdict is a judgement rather than a derivation.
//!
//! **The sampler must be accepted exactly once per token.** `llama_sampler_sample`
//! already calls `llama_sampler_accept` internally. Calling `accept` again — the
//! obvious thing to write, and what a working example without a grammar does —
//! advances the grammar twice per token until no stack survives, at which point
//! llama.cpp reaches `GGML_ASSERT(!stacks.empty())` and **aborts the process**.
//! Not an error, not a panic: `abort(3)`. That is why the loop below does not
//! call `accept`, and why the comment saying so is longer than the line it is
//! about.
//!
//! [ADR-0007]: ../../../docs/adr/0007-single-resident-process.md

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, SyncSender, sync_channel};
use std::time::Instant;

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use crate::prompt::Prompt;
use crate::verdict::{self, Verdict};

/// The context window.
///
/// The prompt is 461 tokens of instructions plus a command capped at 2,000
/// characters, and the answer is bounded by the grammar. 2,048 covers that with
/// room, and a smaller context is a smaller KV cache — 518 MiB of Metal buffer
/// at this size, which is the number that decides whether this runs comfortably
/// beside a browser.
const N_CTX: u32 = 2048;

/// Tokens per prefill batch, at llama.cpp's default `n_batch`.
const PREFILL_CHUNK: usize = 512;

/// The most tokens one answer may take.
///
/// The grammar bounds the summary at 200 characters, so a well-behaved answer is
/// about 45 tokens. This is the backstop for one that is not, and it is a
/// *bound on the work*, not on the schema — an answer cut off here fails
/// validation and is recorded as failed, which is the honest outcome.
const MAX_ANSWER_TOKENS: i32 = 220;

/// How many requests may wait before the caller is made to wait.
///
/// Small on purpose. The queue is not where a backfill lives — that reads its
/// next batch from the store, which is a queue that survives a restart. This is
/// only the hand-off, and a deep one would just mean more work to throw away
/// when the model is unset.
const QUEUE_DEPTH: usize = 32;

/// What went wrong, in the caller's terms rather than llama.cpp's.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("no model is loaded")]
    NotLoaded,
    #[error("the inference worker has stopped")]
    WorkerGone,
    #[error("{0}")]
    Load(String),
    #[error("{0}")]
    Generate(String),
}

/// One request and where its answer goes.
enum Job {
    Analyze {
        command: String,
        reply: Sender<Result<Answer, EngineError>>,
    },
    /// Put down the model and stop. Sent when the model is unset (task 13.7).
    Stop,
}

/// What the model said about one command, and how long it took.
#[derive(Debug, Clone)]
pub struct Answer {
    /// `Ok` when the schema accepted it, `Err` with the reason when it did not.
    /// A rejected answer is a *result*, not an error: task 13.10 records it.
    pub verdict: Result<Verdict, String>,
    /// Wall clock for this call, including prefill.
    pub ms: i64,
    /// The raw text, kept only for the failure path — a stored reason nobody can
    /// reproduce is not worth the column.
    pub raw: String,
}

/// A handle on the worker. Cloneable, `Send`, and holds no llama.cpp object.
#[derive(Debug, Clone)]
pub struct Engine {
    tx: SyncSender<Job>,
    /// What was loaded, so the window can say so without asking the worker.
    loaded: LoadedModel,
}

/// The model this engine is running, as facts a UI can render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    pub path: PathBuf,
    /// SHA-256 of the file — the model half of the verdict key (task 13.14).
    pub fingerprint: String,
    /// Milliseconds `LlamaModel::load_from_file` took.
    pub load_ms: i64,
    /// Tokens in the cached prompt prefix.
    pub prefix_tokens: i64,
}

impl Engine {
    /// Load a model and start its worker, or say why not.
    ///
    /// Blocks until the model is loaded, because every caller wants to know
    /// whether it worked: the Status card is reporting on a file the user just
    /// chose, and a background load would mean the card says "loading…" while
    /// the real answer is "that is a tarball".
    ///
    /// `fingerprint` is passed in rather than computed here — the caller has
    /// already hashed the file to decide whether anything changed, and hashing
    /// 3.1 GB twice to reach the same answer is not free.
    pub fn start(path: &Path, fingerprint: &str, prompt: Prompt) -> Result<Self, EngineError> {
        let (tx, rx) = sync_channel::<Job>(QUEUE_DEPTH);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(i64, i64), String>>();

        let owned = path.to_path_buf();
        std::thread::Builder::new()
            .name("toolog-llm".into())
            .spawn(move || worker(&owned, &prompt, &rx, &ready_tx))
            .map_err(|e| EngineError::Load(format!("could not start the worker thread: {e}")))?;

        let (load_ms, prefix_tokens) = ready_rx
            .recv()
            .map_err(|_| EngineError::WorkerGone)?
            .map_err(EngineError::Load)?;

        Ok(Self {
            tx,
            loaded: LoadedModel {
                path: path.to_path_buf(),
                fingerprint: fingerprint.to_string(),
                load_ms,
                prefix_tokens,
            },
        })
    }

    /// What is loaded.
    #[must_use]
    pub fn loaded(&self) -> &LoadedModel {
        &self.loaded
    }

    /// Analyse one command, waiting for the answer.
    ///
    /// Blocking, and the callers are the backfill loop and the live queue —
    /// both already on their own threads. Making this async would put a runtime
    /// between two blocking things and buy nothing.
    pub fn analyze(&self, command: &str) -> Result<Answer, EngineError> {
        let (reply, answers) = std::sync::mpsc::channel();
        self.tx
            .send(Job::Analyze {
                command: command.to_string(),
                reply,
            })
            .map_err(|_| EngineError::WorkerGone)?;
        answers.recv().map_err(|_| EngineError::WorkerGone)?
    }

    /// Put the model down. Idempotent; a worker already gone is not an error.
    pub fn stop(&self) {
        let _ = self.tx.try_send(Job::Stop);
    }
}

/// The thread. Owns the backend, the model, the context and the batch, and
/// nothing outside it ever sees them.
fn worker(
    path: &Path,
    prompt: &Prompt,
    rx: &Receiver<Job>,
    ready: &Sender<Result<(i64, i64), String>>,
) {
    let started = Instant::now();

    let backend = match LlamaBackend::init() {
        Ok(b) => b,
        Err(e) => {
            let _ = ready.send(Err(format!("llama.cpp would not start: {e}")));
            return;
        }
    };

    // `n_gpu_layers = 999` means "all of them". On a Mac that is Metal; on a
    // machine without a GPU backend llama.cpp falls back to the CPU rather than
    // failing, which is the behaviour we want from a number that means "as much
    // as you can".
    let params = LlamaModelParams::default().with_n_gpu_layers(999);
    let model = match LlamaModel::load_from_file(&backend, path, &params) {
        Ok(m) => m,
        Err(e) => {
            let _ = ready.send(Err(format!("{}: {e}", path.display())));
            return;
        }
    };
    let load_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);

    let ctx_params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(N_CTX));
    let mut ctx = match model.new_context(&backend, ctx_params) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(format!("could not open a context: {e}")));
            return;
        }
    };

    // The grammar is compiled once here rather than per call, so a grammar that
    // does not parse is reported at load — before any verdict has been recorded
    // against it — instead of at the first command.
    if LlamaSampler::grammar(&model, crate::prompt::GRAMMAR, crate::prompt::GRAMMAR_ROOT).is_err() {
        let _ = ready.send(Err(
            "the built-in answer grammar did not compile, which is a bug in this build".to_string(),
        ));
        return;
    }

    // The instructions, prefilled once and never again.
    let prefix = match model.str_to_token(prompt.prefix(), AddBos::Always) {
        Ok(t) => t,
        Err(e) => {
            let _ = ready.send(Err(format!("the prompt would not tokenize: {e}")));
            return;
        }
    };
    let n_prefix = i32::try_from(prefix.len()).unwrap_or(i32::MAX);
    let mut batch = LlamaBatch::new(PREFILL_CHUNK, 1);
    if let Err(e) = feed(&mut ctx, &mut batch, &prefix, 0, false) {
        let _ = ready.send(Err(format!("the prompt would not prefill: {e}")));
        return;
    }

    if ready.send(Ok((load_ms, i64::from(n_prefix)))).is_err() {
        // Whoever asked for the model has gone. Nothing to serve.
        return;
    }
    tracing::info!(
        path = %path.display(),
        load_ms,
        prefix_tokens = n_prefix,
        "local model ready"
    );

    while let Ok(job) = rx.recv() {
        match job {
            Job::Stop => break,
            Job::Analyze { command, reply } => {
                let started = Instant::now();
                let outcome = one(&model, &mut ctx, &mut batch, prompt, n_prefix, &command);
                let ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
                let answer = match outcome {
                    Ok(raw) => Ok(Answer {
                        verdict: verdict::parse(&raw).map_err(|e| e.to_string()),
                        ms,
                        raw,
                    }),
                    Err(e) => Err(EngineError::Generate(e)),
                };
                // A caller that stopped waiting is not a failure; the next job
                // is what matters.
                let _ = reply.send(answer);
            }
        }
    }
    tracing::info!("local model unloaded");
}

/// Decode `tokens` at positions `at..`, asking for logits only where generation
/// will start.
fn feed(
    ctx: &mut LlamaContext,
    batch: &mut LlamaBatch,
    tokens: &[LlamaToken],
    at: i32,
    logits_on_last: bool,
) -> Result<(), String> {
    let n = tokens.len();
    let mut i = 0;
    while i < n {
        let end = (i + PREFILL_CHUNK).min(n);
        batch.clear();
        for (j, token) in tokens.iter().enumerate().take(end).skip(i) {
            let last = j == n - 1;
            let position = at
                .checked_add(i32::try_from(j).map_err(|_| "a prompt too long to index")?)
                .ok_or("a prompt too long for this context")?;
            batch
                .add(*token, position, &[0], logits_on_last && last)
                .map_err(|e| e.to_string())?;
        }
        ctx.decode(batch).map_err(|e| e.to_string())?;
        i = end;
    }
    Ok(())
}

/// One command in, the model's raw text out.
fn one(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    batch: &mut LlamaBatch,
    prompt: &Prompt,
    n_prefix: i32,
    command: &str,
) -> Result<String, String> {
    // Everything after the cached instructions goes; the instructions stay.
    let keep = u32::try_from(n_prefix).map_err(|_| "a negative prefix length")?;
    ctx.clear_kv_cache_seq(Some(0), Some(keep), None)
        .map_err(|e| e.to_string())?;

    // `AddBos::Never`: the prefix already carries it, and a second one mid-
    // sequence is a token the model has never seen there.
    let tail = model
        .str_to_token(
            &format!("{}{}", prompt.body(command), prompt.suffix()),
            AddBos::Never,
        )
        .map_err(|e| e.to_string())?;
    let n_tail = i32::try_from(tail.len()).map_err(|_| "a command too long to index")?;
    if n_prefix.saturating_add(n_tail) >= i32::try_from(N_CTX).unwrap_or(i32::MAX) {
        return Err("the command does not fit in the context window".to_string());
    }
    feed(ctx, batch, &tail, n_prefix, true)?;

    // Greedy under the grammar. Not a temperature: two runs of the same
    // question should differ as little as this can make them, and every source
    // of variance that can be removed makes the stored verdict a more honest
    // record of what the model thinks.
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::grammar(model, crate::prompt::GRAMMAR, crate::prompt::GRAMMAR_ROOT)
            .map_err(|e| e.to_string())?,
        LlamaSampler::greedy(),
    ]);

    let mut decoder = encoding_rs_of(model);
    let mut out = String::new();
    let mut n_past = n_prefix + n_tail;
    let limit = (n_past + MAX_ANSWER_TOKENS).min(i32::try_from(N_CTX).unwrap_or(i32::MAX) - 1);

    while n_past < limit {
        // `sample` accepts the token into the sampler chain itself. Do **not**
        // call `accept` here: a second accept advances the grammar past the
        // token that was actually emitted, and two or three tokens later
        // llama.cpp aborts the process on `GGML_ASSERT(!stacks.empty())`.
        let token = sampler.sample(ctx, -1);
        if model.is_eog_token(token) {
            break;
        }
        out.push_str(
            &model
                .token_to_piece(token, &mut decoder, false, None)
                .map_err(|e| e.to_string())?,
        );
        // The grammar cannot emit anything after the closing brace, so waiting
        // for an end-of-generation token here would be waiting for a model that
        // has already finished.
        if out.trim_end().ends_with('}') {
            break;
        }
        batch.clear();
        batch
            .add(token, n_past, &[0], true)
            .map_err(|e| e.to_string())?;
        n_past += 1;
        ctx.decode(batch).map_err(|e| e.to_string())?;
    }

    Ok(out)
}

/// A fresh incremental UTF-8 decoder.
///
/// A token is not a character: a multi-byte one arrives in pieces, and decoding
/// each piece on its own turns an accented path into replacement characters.
fn encoding_rs_of(_model: &LlamaModel) -> encoding_rs::Decoder {
    encoding_rs::UTF_8.new_decoder()
}
