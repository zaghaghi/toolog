# Fixtures

## `transcripts/`

**Synthetic**, not scrubbed recordings.

The corpus this parser was written against holds 226,192 string leaves totalling 37.3 MB of
free text — prompts, source code, client project names. Reliably redacting that for a public
repository is not achievable, and a single miss publishes someone's code.

These files instead reproduce the *structures* the real corpus exhibits, drawn from
`schema-manifest.json`. Between them they cover:

- all three `toolUseResult` shapes: object, bare string (always an error), and array
- `Bash`, `Read`, `Edit`, `Write`, `WebFetch`, `WebSearch`, `Grep`, an MCP tool and `Agent`
- a subagent: an `Agent` spawn, its `agentId`, and the sidechain calls it owns
- `agent-name` (a *session* label) and `relocated` records
- a record type and a tool this build has never heard of
- a line that is not JSON at all

They are the parser's contract. When Claude Code changes format, add a fixture rather than
patching a test.

## `schema-manifest.json`

Key names and value **types** extracted from a real corpus, with every value discarded.
21 record types across 12 Claude Code versions. Verified to contain no leaf outside the
vocabulary `string | bool | number | null | ...`, so it carries schema and no content.

Used to keep the synthetic fixtures faithful.

## What is not here

Real transcripts. `tests/real_corpus.rs` reads `~/.claude/projects` directly when it exists
and asserts on properties rather than values, so real data provides regression coverage
without ever being committed. `.gitignore` excludes `fixtures/raw/`.
