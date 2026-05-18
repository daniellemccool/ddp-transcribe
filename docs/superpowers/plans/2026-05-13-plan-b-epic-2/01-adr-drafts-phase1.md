# Task 1 — Draft + decide AD0021, AD0022, AD0023; audit Open Questions FOLLOWUPS

**Goal:** Land three Phase-1 ADRs in `decided` state via the `adg` tool so subsequent tasks can reference them. Also audit the three "Open Questions" entries in `docs/followups/epic-2.md` (T8 lang_probs second-state, T9 non-finite f32 rejection, T13 cfg-gated backend assertion) against shipped Epic 1 `src/` code; archive any that are confirmed-already-shipped.

**ADRs touched:** AD0021 (drafts), AD0022 (drafts), AD0023 (drafts).

**Files:**
- Create: `docs/decisions/AD0021-schema-version-policy-hard-fail-and-migrate-subcommand.md`
- Create: `docs/decisions/AD0022-minimum-mutator-signatures-kind-message-result-usize.md`
- Create: `docs/decisions/AD0023-stale-claim-sweep-no-validation-no-attempt-bump-30-min-default.md`
- Modify: `docs/followups/epic-2.md` (archive the three Open Questions if confirmed)
- Modify (if any archived): `docs/archive/followups-resolved.md` (append-only)

**Pre-reqs:** branch `feat/plan-b-epic-2` checked out from `main`. ADR tooling is project-local; see the `using-adg` skill if unfamiliar.

---

- [ ] **Step 1: Branch off main**

```bash
git switch -c feat/plan-b-epic-2
```

Expected: clean branch creation. If the branch already exists from a prior session, `git switch feat/plan-b-epic-2` and continue.

- [ ] **Step 2: Draft AD0021 (schema-version policy)**

Title encodes the decision tersely (project convention; see `using-adg` skill for the title-quality rule).

```bash
adg add --model docs/decisions \
  --id 0021 \
  --title "Schema-version policy: hard-fail at Store::open, migrate via dedicated CLI subcommand"
```

Then fill the body via `scripts/adr-fill 0021`. The script opens `$EDITOR`; provide the body content below (paste into the editor):

```markdown
## Context and Problem Statement

Plan B Epic 2 introduces four new nullable columns and bumps SCHEMA_VERSION "1" → "2". Plan A's `Store::open` records the version on first run via `INSERT OR IGNORE INTO meta` but never reads it back; opening an older DB silently runs against newer code, with whatever-happens-happens semantics on missing columns. FOLLOWUPS T7 (`Store::open` schema-version not read) tracks the gap. Epic 2 needs a policy that (a) refuses to open a DB at the wrong version, (b) gives the operator a clear migration path, (c) preserves Plan A bake data, and (d) sets a precedent for Epic 3+ schema bumps.

## Considered Options

1. Hard-fail at Store::open + dedicated `migrate` CLI subcommand
2. Auto-migrate on open (silently apply ALTER TABLE if version mismatch)
3. Log-and-warn on mismatch but continue
4. Wipe-and-re-ingest (delete DB on mismatch; force operator to re-ingest)

## Decision Drivers

- Operational visibility (operator must know what happened)
- Preserves bake data and donor-watched-video history
- Doesn't tempt silent drift in production
- Sets a precedent that scales to Epic 3+ schema bumps

## Decision Outcome

Option 1: hard-fail at `Store::open` via typed `SchemaVersionMismatch { expected, found }` error containing operator-readable instruction directing them to the new `migrate` subcommand. The `migrate` subcommand opens the DB raw (bypassing the version check), runs `ALTER TABLE videos ADD COLUMN ... NULL` × 4 + `UPDATE meta SET value='2'` inside one transaction, and exits. Idempotent on already-v2 DBs (no-op if already migrated).

Auto-migrate (2) was rejected for silent-drift risk: the operator wouldn't know a migration ran, which is operationally invisible. Log-and-warn (3) was rejected for the same reason — operators ignore warnings. Wipe-and-re-ingest (4) was rejected because it loses Epic 1's bake artifacts and donor history (which has no source-of-truth restore path).

## Consequences

- Every Epic 2+ schema bump becomes a new ALTER block in the `migrate` subcommand (idempotent layering).
- Pre-Epic-2 DBs require one operator action (`uu-tiktok migrate`) before `process` works.
- Tests cover both directions: opening a v1 DB without migration fails with the typed error; opening a v2 DB succeeds; running `migrate` on a v2 DB is a no-op.
```

Then decide it:

```bash
adg decide --model docs/decisions --id 0021
```

Expected: status flips to `decided`; `adg validate --model docs/decisions` passes.

- [ ] **Step 3: Draft AD0022 (mutator signatures)**

```bash
adg add --model docs/decisions \
  --id 0022 \
  --title "Minimum mutator signatures: (kind: &str, message: &str) returning Result<usize> per AD0006"
```

Then `scripts/adr-fill 0022` with body:

```markdown
## Context and Problem Statement

Epic 2's state machine adds two failure-classification mutators (`mark_retryable_failure`, `mark_terminal_failure`) and uses them from the serial loop (Phase 1) and orchestrator (Phase 2). Epic 3 will introduce typed-enum failure classification (`RetryableKind`, `UnavailableReason`, `ClassifiedFailure`). Question: what signature should Epic 2's mutators use today, given Epic 3 will refine them?

## Considered Options

1. `(kind: &str, message: &str) -> Result<usize>` — string-typed kind, AD0006-conformant return
2. Typed enum today (front-load Epic 3's `ClassifiedFailure`)
3. Free-form `&serde_json::Value` payload (let callers store whatever)

## Decision Drivers

- AD0006 conformance (row-change count return)
- Compose cleanly with Epic 3's typed enums
- Don't lock Epic 3's design in
- Minimum surface area today

## Decision Outcome

Option 1: `(video_id: &str, kind: &str, message: &str) -> Result<usize>`. Plus the same `WHERE status='in_progress' AND claimed_by = ?` predicate as the tightened `mark_succeeded` (so all three mutators have symmetric stale-claim semantics).

Epic 3 introduces typed enums via `ClassifiedFailure` and a wider signature; Epic 2's `(kind, msg)` surface composes cleanly on top — Epic 3's dispatcher just calls `mark_retryable_failure(id, &kind.tag(), &kind.message())` or similar. No structural "context" / extra-field hooks today — Epic 3 introduces structure when it has a structural reason.

Option 2 was rejected because it pre-decides Epic 3's design before the failure-mode catalog is empirically grounded. Option 3 was rejected because untyped payloads invite drift (every caller defines its own schema).

## Consequences

- Phase 1's classifier wiring (T9) emits string kinds like `"FetchOrTranscribe"` until Epic 3 lands typed enums.
- Phase 2's fetch worker reuses the same string-kind path.
- Epic 3's first task is the enum + classifier dispatcher; signature broadens at that point with `succeeds: ["AD0022"]` if the change is structural.
```

Decide:

```bash
adg decide --model docs/decisions --id 0022
```

- [ ] **Step 4: Draft AD0023 (stale-claim sweep)**

```bash
adg add --model docs/decisions \
  --id 0023 \
  --title "Stale-claim sweep: no validation, no attempt_count bump, 30-min default threshold"
```

Then `scripts/adr-fill 0023` with body:

```markdown
## Context and Problem Statement

Process crashes mid-batch leave rows in `status='in_progress'` with valid `claimed_by`/`claimed_at`. Plan A had no recovery; Plan B Epic 2 introduces a `sweep_stale_claims(threshold)` mutator that flips stale rows back to `pending`. Open design questions: (a) should the sweep validate that no artifacts exist before reverting? (b) should it bump `attempt_count`? (c) what threshold default?

## Considered Options

1. No validation, no attempt_count bump, 30-min default
2. Validate artifacts present → mark_succeeded; else revert (validate-and-mark-succeeded)
3. Bump attempt_count on every sweep (count sweeps as retries)
4. Configurable threshold with conservative default (1 hour)

## Decision Drivers

- AD0008 invariant ("in_progress + complete artifacts" is an accepted intermediate state)
- Don't corrupt Epic 3's retry-policy semantics
- Prevent stealing from healthy peers in any future multi-instance scenario
- Bake worst-case fetch is ~25s; threshold must be well above that

## Decision Outcome

Option 1: no artifact validation; no `attempt_count` bump; default threshold 30 minutes.

Artifact validation (2) is deferred to Plan C if measured to matter — the sweep happens at process startup, and Phase 2's coordinated-shutdown drill shows the redo cost on a kill -KILL is one re-fetch + one re-transcribe per in-flight row. Empirically negligible against the throughput delta from N=3 vs N=1.

`attempt_count` bump (3) was rejected because mixing operator-recovery semantics (sweep) with application-retry semantics (Epic 3's classifier-driven retry) would corrupt Epic 3's retry-policy logic. The sweep is "this row was abandoned by a crashed process," not "this row failed and is being retried." `attempt_count` stays the count of `claim_next` invocations.

30-min default (1) is conservative against bake worst-case (~25s end-to-end per video, single-state). Far above worst-case prevents stealing from healthy peers in any future multi-instance scenario. The threshold is `--stale-claim-threshold` flag-tunable (T11) so operators can tighten for testing.

## Consequences

- A sweep on a v2 DB with stale claims emits exactly one log line per recovered row (no event row inserted in `video_events` — the sweep is an operator-recovery action, not an application event).
- Plan C may add `--validate-artifacts-on-sweep` and `--bump-attempts-on-sweep` flags if measurement supports them; AD0023 records that those are deferred-by-design.
- The redo cost of a kill -KILL mid-batch is bounded by the threshold + the per-video budget (~30min + ~7s = ~30min worst-case wallclock until full recovery).
```

Decide:

```bash
adg decide --model docs/decisions --id 0023
```

- [ ] **Step 5: Validate the ADR set**

```bash
adg validate --model docs/decisions
```

Expected: clean. The pre-commit hook in `.githooks/pre-commit` runs this automatically; ensure `git config core.hooksPath .githooks` is set (one-time per fresh clone).

- [ ] **Step 6: Audit FOLLOWUPS Open Questions (T8 lang_probs, T9 non-finite f32, T13 cfg-gated backend)**

The spec at line 226–234 names three FOLLOWUPS entries marked "Status unverified against shipped Epic 1 code — confirm before archiving." Audit each:

**T8 lang_probs second-state:** check `src/transcribe.rs` for second-state allocation:

```bash
grep -n 'lang_state\|create_state' src/transcribe.rs
```

Expected: if a second state is created and used for lang_probs (gated on `PerCallConfig::compute_lang_probs`), the FOLLOWUPS entry is shipped — archive it.

**T9 non-finite f32 rejection:** check `extract_segments` (in `src/transcribe.rs`):

```bash
grep -n 'is_finite\|non.finite\|TokenRaw\|SegmentRaw' src/transcribe.rs
```

Expected: if `extract_segments` (or equivalent) rejects non-finite values when constructing `TokenRaw` / `SegmentRaw`, the FOLLOWUPS entry is shipped — archive it.

**T13 cfg-gated backend assertion:** check `WhisperEngine::new` for the cuda-feature-gated `BackendMismatch` error:

```bash
grep -n 'BackendMismatch\|cfg(feature' src/transcribe.rs
```

Expected: if `WhisperInitError::BackendMismatch` is produced under `#[cfg(feature = "cuda")]` when whisper.cpp's backend isn't GPU, the FOLLOWUPS entry is shipped — archive it.

- [ ] **Step 7: Archive confirmed Open Questions**

For each entry confirmed shipped in Step 6, move the body from `docs/followups/epic-2.md` to `docs/archive/followups-resolved.md` (append-only). Format:

```markdown
## <Entry title> [archived 2026-05-14, confirmed shipped in <Epic 1 commit SHA>]

<original body>

**Resolution:** confirmed against `src/transcribe.rs` at commit <SHA>; the
shipped Epic 1 code already implements the behavior described.
```

Update `docs/FOLLOWUPS.md`'s scope-index by removing the corresponding lines under the Epic 2 group.

If any entry is NOT confirmed (the code doesn't actually do what the FOLLOWUPS entry expected), leave it in `docs/followups/epic-2.md` with a note `**Audit (2026-05-14): NOT confirmed against shipped Epic 1 code. Re-investigate during <task>.**` — this is the AD0020 unverified-hypothesis discipline applied retroactively.

- [ ] **Step 8: cargo fmt + clippy (defensive — no code changes this task, but the pre-commit hook will run)**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: no changes; clippy clean (no code changed).

- [ ] **Step 9: Commit**

```bash
git add docs/decisions/AD0021-*.md docs/decisions/AD0022-*.md docs/decisions/AD0023-*.md \
        docs/FOLLOWUPS.md docs/followups/epic-2.md docs/archive/followups-resolved.md
git commit -m "$(cat <<'EOF'
docs(decisions): land AD0021–AD0023 (Phase 1 ADRs) + audit Epic 1 FOLLOWUPS Open Questions

AD0021 — Schema-version policy: hard-fail at Store::open via typed
SchemaVersionMismatch error; dedicated `migrate` CLI subcommand runs
ALTER TABLE + UPDATE meta in one transaction. Preserves Plan A bake
data; sets a precedent for Epic 3+ schema changes.

AD0022 — Minimum mutator signatures: (video_id, kind: &str, message: &str)
returning Result<usize> per AD0006. Epic 3 introduces typed enums via
ClassifiedFailure; Epic 2's surface composes cleanly. No extra "context"
field — Epic 3 introduces structure when it has a structural reason.

AD0023 — Stale-claim sweep: no validation, no attempt_count bump, 30-min
default. Validate-and-mark-succeeded deferred to Plan C if measured to
matter. attempt_count stays count of claim_next invocations; sweep is
operator-recovery, not application-retry.

FOLLOWUPS audit (Open Questions, spec line 226):
- T8 lang_probs second-state: <result>
- T9 non-finite f32 rejection: <result>
- T13 cfg-gated backend assertion: <result>

(Replace <result> with: "confirmed shipped — archived to
docs/archive/followups-resolved.md" or "NOT confirmed — left in
docs/followups/epic-2.md with audit note.")

Refs: AD0006, AD0008, AD0020, AD0021, AD0022, AD0023

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `adg validate --model docs/decisions` clean
- [ ] All three ADRs are `status: decided`
- [ ] FOLLOWUPS audit: each Open Question entry has an explicit disposition (archived or left with audit note)
- [ ] `docs/FOLLOWUPS.md` scope-index reflects archived entries (lines removed)
- [ ] Commit message names all three ADRs and the audit outcomes
