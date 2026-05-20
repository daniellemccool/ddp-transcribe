# Task 5 — Populate `state-machine.md`

**Goal:** Replace every `(TBD)` in `docs/reference/architecture/state-machine.md` with real content covering schema, lifecycle states, claim contention, the stale-claim sweep, mutator contracts, schema-version policy, crash-recovery durability, and failure classification. Includes one ASCII state-transition diagram. Also: replace the `<sha>` placeholder in the in-flight stamp with the current short commit hash.

**ADRs referenced:** 0006, 0008, 0022, 0023, 0024, 0026.

**Files:**
- Modify: `docs/reference/architecture/state-machine.md`
- Read: `src/state/` (mod.rs, store.rs, schema.rs, migrations if present), `Cargo.toml` (verify rusqlite version + `bundled` feature)

**Pre-reqs:** T01, T02 complete. The two stable deepdive files (`data-input.md`, `transcription.md`) ideally complete first so the in-flight content reflects the most recent state machine code; if T03/T04 not done yet, this task can still proceed but the writer should be aware that the deepdives may cross-reference state-machine.md when they're written.

---

- [ ] **Step 1: Survey source files**

```bash
ls src/state/ && wc -l src/state/*.rs
grep -n "^fn\|^pub fn\|^impl\|^struct\|^enum\|CREATE TABLE\|ALTER TABLE\|SCHEMA_VERSION" src/state/*.rs
```

Note:
- Schema files (likely `src/state/schema.rs` or migrations).
- The status / lifecycle-state column values (look at the `CREATE TABLE` and any enum definition).
- Public mutator signatures on `Store`.
- The `BEGIN IMMEDIATE` pattern (where in `claim_next` it appears).
- The stale-sweep function and its parameters.

Read further to understand:
- Lifecycle state values — typically `pending`, `in_progress`, `succeeded`, `retryable_failure`, `terminal_failure`. Verify against code.
- Schema version constant (`SCHEMA_VERSION`).
- Migration story (probably a CLI subcommand per ADR 0022).

- [ ] **Step 2: Replace the in-flight stamp's `<sha>` placeholder**

Get the current short commit hash:

```bash
git rev-parse --short HEAD
```

Edit the in-flight stamp at the top of `state-machine.md` — replace `<sha>` with that short hash. Keep the `\`` backticks around it. Example: if the hash is `8554c42`, the stamp becomes:

```markdown
> ⚠ **As of commit `8554c42`.** This subsystem is being reshaped in
> [Plan B Epic 2](../../superpowers/plans/2026-05-13-plan-b-epic-2/).
> Expect revision at epic close — names, contracts, and topology may move.
```

- [ ] **Step 3: Write `## Schema and lifecycle states` intro**

Replace the `## Schema and lifecycle states` `(TBD)` with a 3-sentence intro:

```markdown
## Schema and lifecycle states

The state machine is a single sqlite database accessed via the `Store` type in `src/state/`. Each watched video produces one row in the primary `videos` table; the row's `status` column tracks where it sits in the lifecycle. Schema version is recorded in a `meta` table; mismatches fail closed at `Store::open` per [ADR 0022](../../decisions/0022-schema-version-policy-hard-fail-at-store-open-migrate-via-dedicated-cli-subcommand.md) — there is no in-process auto-migrate.
```

- [ ] **Step 4: Write `### Schema overview`**

Describe the tables and their key columns. Don't reproduce the full DDL — point at `src/state/schema.rs` (or wherever the schema lives). Cover:
- The primary `videos` table and the columns that matter to the lifecycle: `video_id` (PK), `status`, `claimed_by`, `claimed_at`, `attempt_count`, the four failure-classification columns added by Plan B Epic 2 (`last_retryable_kind`, `last_retryable_message`, `terminal_reason`, `terminal_message`).
- The `meta` table holding `schema_version`.
- Any other tables (verify by reading the schema source).

For the `videos` row example, give a tabular shape — column name + brief description + nullability. Verify the column names against `src/state/schema.rs`.

- [ ] **Step 5: Write `### Lifecycle states`**

List the status values with one-sentence descriptions. The list likely is:
- `pending` — newly ingested or just released by stale sweep / retry; eligible for claim.
- `in_progress` — currently claimed by a worker; not eligible for claim until the worker finishes or the sweep recovers it.
- `succeeded` — terminal-success; artifact on disk, never re-attempted.
- `retryable_failure` — recoverable failure; the failure mutator records the kind/message and (depending on retry policy) the row returns to `pending` on next pass.
- `terminal_failure` — non-recoverable failure (e.g., video deleted upstream); not re-attempted.

Verify against `src/state/` — if the actual state names differ, use the real ones.

- [ ] **Step 6: Write `### State-transition diagram`**

Add an ASCII diagram covering all transitions. Template (verify each edge against the code; remove edges that don't exist; add any that do):

```markdown
### State-transition diagram

```
                              +------------+
              ingest          |            |  claim_next (BEGIN IMMEDIATE)
              ------->        |  pending   | <-----+
                              |            |       |
                              +-----+------+       |
                                    |              |
                                    | claim taken  |
                                    v              |
                              +------------+       |
                              | in_progress| ------+
                              |            | <----- stale sweep
                              +-----+------+        (per 0024)
                                    |
                  mark_succeeded    |    mark_retryable_failure
                       /            |             \
                      v             |              v
              +-----------+         |    +-------------------+
              | succeeded |         |    | retryable_failure |
              +-----------+         |    +-------------------+
                                    |             |
                                    |             | next pass
                                    |             | (retry policy)
                                    |             v
                                    |        (back to pending)
                                    |
                              mark_terminal_failure
                                    |
                                    v
                            +-------------------+
                            | terminal_failure  |
                            +-------------------+
```
```

The textbox shape is illustrative; alignment doesn't need to be precise. If the actual retry mechanism is something else (e.g., a separate retry column, or `retryable_failure` is itself eligible for `claim_next`), redraw the edges accurately and cite `src/state/store.rs:N`.

- [ ] **Step 7: Write `## Claim contention`**

Describe how multiple workers (whether the current single transcribe worker + future expansions, or the n=3 fetch workers all calling `claim_next` per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md)) avoid double-claiming the same row.

Cover:
- The `BEGIN IMMEDIATE` transaction per [ADR 0026](../../decisions/0026-claim-contention-no-polling-for-plan-b-batch-drain-on-claim-next-none.md) — sqlite serializes writers, so the SELECT-FOR-CLAIM + UPDATE pair runs atomically.
- The `claim_next` return type — `Option<Claim>`. `None` signals "no claimable rows right now"; per ADR 0026, the orchestrator does *not* poll. It drains the batch.
- The `claimed_by` field — likely a UUID or PID identifying the worker. Cite where it's set.

Redirect the rationale (why no polling) to ADR 0026.

- [ ] **Step 8: Write `## Stale-claim sweep`**

Per [ADR 0024](../../decisions/0024-stale-claim-sweep-no-validation-no-attempt-count-bump-30-min-default-threshold.md), `Store::sweep_stale_claims(threshold)` looks for `in_progress` rows whose `claimed_at` is older than `threshold` and resets them to `pending`. Cover:
- When the sweep runs (top of `run_serial` and `run_pipelined` per Epic 2 wiring; verify).
- The default threshold (30 minutes).
- What the sweep does *not* do: no validation that the worker actually died; no `attempt_count` bump (per 0024).
- The motivating failure mode: a kill -9 leaves a claim row stuck in `in_progress`; without the sweep, that row blocks all future progress.

Redirect to ADR 0024 for rationale.

- [ ] **Step 9: Write `## Mutator contracts`**

Per [ADR 0006](../../decisions/0006-store-mutators-return-result-of-usize-for-row-change-count.md) and [ADR 0023](../../decisions/0023-minimum-mutator-signatures-kind-str-message-str-returning-result-usize-per-0006.md), all `Store` mutators follow a common shape:
- Return `Result<usize>` — the inner `usize` is the row-change count.
- For mutators that operate on a specific row, the contract is "return 0 if the row didn't match the WHERE predicate" — this is how stale-claim handling surfaces (e.g., `mark_succeeded` returns 0 if the claim has already been swept).
- For mutators that record failures, the signature is `(kind: &str, message: &str)`.

List the mutators currently defined (verify against `src/state/store.rs`):
- `claim_next` — special; not a row-change mutator, returns `Option<Claim>`.
- `mark_succeeded(video_id, claimed_by)` — per ADR 0008, the WHERE predicate requires the claim to still be live.
- `mark_retryable_failure(video_id, kind, message)` — per ADR 0023.
- `mark_terminal_failure(video_id, reason, message)` — per ADR 0023.
- `sweep_stale_claims(threshold)` — per ADR 0024.

Redirect signature rationale to ADRs 0006 and 0023.

- [ ] **Step 10: Write `## Schema-version policy`**

One paragraph. The `Store::open` constructor reads the `schema_version` row from `meta` and compares against the binary's `SCHEMA_VERSION` constant. On mismatch, it returns a typed `SchemaVersionMismatch { expected, found }` error whose `Display` impl tells the operator to run `uu-tiktok migrate`. Migration runs in a dedicated CLI subcommand that bypasses the version check, applies the migration SQL in one transaction, and bumps `meta.schema_version`. Per [ADR 0022](../../decisions/0022-schema-version-policy-hard-fail-at-store-open-migrate-via-dedicated-cli-subcommand.md), there is no in-process auto-migrate — fail closed, ask for human action.

- [ ] **Step 11: Write `## Crash-recovery durability`**

Per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md), the durability ordering is: write transcript artifact to disk → only then call `mark_succeeded`. Effects:
- A crash between artifact-write and `mark_succeeded` leaves the row in `in_progress`. Stale sweep eventually recovers it; the next attempt finds the artifact already on disk and re-writes it (idempotent overwrite).
- A crash before artifact-write leaves the row in `in_progress` with no artifact; the next attempt re-runs the full fetch+transcribe.
- A crash after `mark_succeeded` is fully durable — both the artifact and the state row are committed.

Cross-link: full discussion of artifact-side enforcement in [`transcription.md`](transcription.md). Redirect crash-recovery rationale to ADR 0008.

- [ ] **Step 12: Write `## Failure classification`**

Describe how failures are classified into retryable vs terminal at the state-machine surface. Cover:
- The two mutators: `mark_retryable_failure` and `mark_terminal_failure`.
- The columns each writes (verify against schema): `last_retryable_kind`, `last_retryable_message`, `terminal_reason`, `terminal_message`.
- The current classifier: as of Plan B Epic 2 (in-flight), the classifier is string-kind only; rich classification is planned for Epic 3. Cross-link to `orchestration.md` for the caller's perspective.

- [ ] **Step 13: Write ADRs section**

```markdown
## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0006 | `Store` mutators return `Result<usize>` | Mutator return contract. |
| 0008 | Artifact-before-mark_succeeded | Durability ordering. Cross-cuts transcription. |
| 0022 | Schema-version policy | `Store::open` hard-fail + `migrate` subcommand. |
| 0023 | Minimum mutator signatures | `mark_retryable_failure` / `mark_terminal_failure` signatures. |
| 0024 | Stale-claim sweep | `sweep_stale_claims` semantics. |
| 0026 | Claim contention via `BEGIN IMMEDIATE` | `claim_next` serialization. |
```

- [ ] **Step 14: Verify, lint, commit**

```bash
grep -n "(TBD\|<sha>" docs/reference/architecture/state-machine.md
wc -l docs/reference/architecture/state-machine.md
```

Expected: no `(TBD)` and no `<sha>` placeholders remain; line count 160-230 (spec budget ~190).

```bash
grep -oP '\(\.\./\.\./decisions/\K[^)]+' docs/reference/architecture/state-machine.md | sort -u | while read f; do
  test -f "docs/decisions/$f" && echo "OK ADR: $f" || echo "MISSING ADR: $f"
done
```

Expected: every line `OK`.

```bash
git add docs/reference/architecture/state-machine.md
git commit -m "$(cat <<'EOF'
docs(reference): populate architecture/state-machine.md

Schema + lifecycle states + ASCII state-transition diagram, claim
contention per 0026, stale sweep per 0024, mutator contracts per 0006/
0023, schema-version policy per 0022, durability ordering per 0008,
failure classification surface.

In-flight stamp pinned to current commit; will revise at Epic 2 close.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
