# Phase 7 — Privacy, retention & integrity

**Goal:** earn the word "audit". Until this phase the tool records; after it, the record can be
trusted, bounded and defended.

**Depends on:** Phases 2, 3, 6. **Unblocks:** Phase 8.
**Governed by:** [ADR-0008](../adr/0008-local-only-zero-egress.md),
[ADR-0009](../adr/0009-correlate-on-tool-use-id.md).

## Tasks

- [ ] **7.1** **`toolog verify` — reconciliation.** Cross-check the lanes per ADR-0009:
  - OTEL-only calls → **rejected calls**, listed as such
  - Transcript-only calls → **collection gaps**, with the window in which the app was not running
  - A per-session completeness figure

  This is what lets the tool state its own completeness rather than assume it, and it is the
  feature that most distinguishes an audit tool from a log viewer.
- [ ] **7.2** **Secret redaction** at normalization: API keys, bearer tokens, `Authorization`
  headers, private keys, `.env` values, connection strings, AWS/GCP credentials. Pattern set as
  data, user-extensible.
- [ ] **7.3** **Decide, expose and document whether `raw_event` is redacted too.** Redacting the
  evidence store is defensible but lossy and irreversible; leaving it intact is defensible but
  keeps secrets on disk. This is the user's call, surfaced explicitly in Preferences with the
  trade-off stated — not a silent default.
- [ ] **7.4** Retention policy: age cap and size cap, with a **purge preview showing exactly what
  would be deleted** before anything is. `VACUUM` after purge.
- [ ] **7.5** Oversized result bodies stored by reference beyond a threshold, using the Phase 1.9
  measurements to set it.
- [ ] **7.6** **Integrity hash-chain** over `raw_event`: each row hashes its predecessor, making
  post-hoc tampering detectable. `toolog verify --chain` walks and reports.
  Document the honest limit: this detects modification of stored evidence, it does not prove the
  evidence was complete when written — that is what 7.1 is for.
- [ ] **7.7** **CI egress test** (ADR-0008): assert **no non-loopback socket is opened** during a
  full ingest plus UI-query run. The privacy guarantee must be a build failure when broken, not a
  convention.
- [ ] **7.8** Pause/resume capture from the tray; per-project exclusion list; delete-a-session
  removing raw and projected rows together (and breaking the hash chain visibly rather than
  silently re-chaining).
- [ ] **7.9** Evaluate **SQLCipher** for encryption at rest; decide and record as an ADR addendum.
  State the honest trade-off: the key must live locally, so this mainly defends the stolen-disk and
  backup cases, not a compromised logged-in machine.
- [ ] **7.10** Complete `PRIVACY.md`: exactly what is stored, where, what never leaves, and the one
  named exception (the opt-in update check, Phase 8).

## Exit criteria

- `toolog verify` reports 100% reconciliation on accepted calls with rejections listed separately.
- A deliberately introduced secret in a test transcript is redacted in the projection.
- `toolog verify --chain` detects a row edited directly with the `sqlite3` CLI.
- The CI egress test passes with zero non-loopback sockets, and fails if a stray HTTP call is added.
