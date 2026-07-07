# Task 08: Retirements (triage/probe/curl), ADR slate, docs, FOLLOWUPS, close-out

**Files:**
- Delete: `src/triage.rs`, `src/probe.rs`, `tests/triage.rs`
- Modify: `src/main.rs` (remove `mod probe; mod triage;` + the whole `Command::Triage` arm), `src/lib.rs` (remove `pub mod probe;` and `pub mod triage;` if present — check), `src/cli.rs` (remove `Command::Triage` variant + `parse_positive_rate`), `Cargo.toml` (remove the `[[test]] name = "triage"` block)
- Modify: `docs/operations/src-vm.md` (operate section), `docs/reference/architecture/state-machine.md` + `orchestration.md` + `index.md` (dispatch/retry sections + `uu-tiktok` naming sweep), `docs/FOLLOWUPS.md` + `docs/followups/epic-4.md` + `docs/archive/followups-resolved.md`
- Create: `docs/superpowers/plans/2026-07-07-plan-b-epic-4a/EPIC-4A-CLOSE.md`
- ADR work via `adg` / `scripts/adr` ONLY (never hand-edit `docs/decisions/`)

**Interfaces:** consumes everything; produces the epic's close-out. No new code interfaces.

- [ ] **Step 1: Delete the triage/probe surface**

```bash
git rm src/triage.rs src/probe.rs tests/triage.rs
```

Then compiler-driven cleanup: remove `mod probe;`/`mod triage;` from `src/main.rs` (and any `pub mod` in `src/lib.rs`), the full `Command::Triage { … }` arm in main, the `Triage` variant + `parse_positive_rate` + its `use` in `src/cli.rs`, and the `[[test]] name = "triage"` block in `Cargo.toml`. Verify:

```bash
grep -rn "probe\|triage\|curl" src/ Cargo.toml | grep -v -i "test-helpers"
```

Expected: no hits in production code (doc-comment mentions of the RETIRED probe are acceptable only in `src/classification.rs`'s evidence comments and `src/state/mod.rs` history notes — judge each hit; the binary must have zero probe/triage code paths and zero curl invocations).

- [ ] **Step 2: Verify + commit the code retirement**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green; test count DROPS by the deleted triage suite (state that count in the commit body).

```bash
git add -A
git commit -m "chore(epic-4a): retire triage subcommand, oEmbed probe, and the curl runtime dependency

The probe was Epic 3's validation instrument; its n=36 evidence session
plus the single full dry-run census (2026-07-07) calibrated the write-off
classes, and in-pipeline retry (fetch-as-oracle) now owns adjudication."
```

- [ ] **Step 3: ADR slate (each via `scripts/adr`; bodies via stdin)**

1. **New ADR — in-batch retry + claim ordering** (`scripts/adr new "In-batch capped retry with end-of-queue claim ordering; fetcher is the liveness oracle"`), body:

```markdown
## Context and Problem Statement

Epic 3 shipped operator-driven triage: an oEmbed probe adjudicated parked
failures and a manual subcommand requeued them. The 2026-07-07 census
(n=7,087) showed the probe re-confirming settled classes, the operator flow
added ceremony per batch, and dry-run + execute double-probed. The operator
ruled retry must be pipeline behavior.

## Considered Options

* Operator-driven probe triage (Epic 3 status quo, ADR 0034)
* In-batch capped retry: failure-time requeue to the end of the queue; the
  re-fetch itself adjudicates liveness (fetch-as-oracle)
* Automatic backoff/jitter retry inside the workers

## Decision Outcome

Chosen: in-batch capped retry. record_fetch_failure decides
requeue/exhaust/park in one transaction at failure time; claim ordering
becomes attempt_count ASC, first_seen_at ASC, video_id ASC (fresh work
drains before retries); --retries default 1 caps LIFETIME attempts at
retries+1 against attempt_count (bumped at claim time); --max-videos
counts every claim including retries; a start-of-batch sweep adjudicates
parked rows through the classification table so historical pools and
cross-batch stragglers ride the same mechanism. Dead classes self-classify
on re-fetch (write-off message → inline terminal), which the census showed
handles impure classes (NoPermission 25/452 alive) correctly where blanket
write-offs would discard recoverable videos. Supersedes 0034; the probe
retires with the census as its closing evidence.
```

Then `scripts/adr decide <new-id> 2` (option 2 = in-batch capped retry; adapt to the printed option list) and `scripts/adr supersede <new-id> 0034 "in-pipeline retry replaces operator triage; probe retired after its validation service"`.

2. **New ADR — classification config** (`scripts/adr new "Operator-editable TOML classification table with compiled evidence-derived default and batch provenance"`), body:

```markdown
## Context and Problem Statement

Epic 3 hardcoded yt-dlp stderr classification in src/failure.rs. yt-dlp
wording drifts and new message classes appear (status code 10240 emerged
as 606/606 dead at the 2026-07-07 census); responding must be an operator
table edit, not a code release.

## Considered Options

* Hardcoded classifier chain (Epic 3 status quo)
* Ordered TOML rule table: compiled default, file override, hard-fail
  validation, provenance snapshot per batch
* JSON config (zero new deps, no comments)

## Decision Outcome

Chosen: the TOML table. schema=1; ordered [[rule]] {pattern,label,
disposition ∈ retryable|terminal|requires-cookie}; first-match-wins;
explicit fallback (requires-cookie forbidden as fallback); exact
case-sensitive substrings; validation hard-fails at startup per 0022
philosophy. The compiled default is the evidence-derived census table
(only proven-pure classes are terminal: IpBlockedMessage, 10231, 10240;
NoPermission stays retryable at 25/452 alive). Config governs tool-output
interpretation only — structural errors stay code-mapped. The active
table's full TOML snapshots into batch_runs.policy_toml per run: a census
without its generating policy is not reproducible attrition documentation.
```

`scripts/adr decide <new-id> 2` (the TOML option).

3. **Comment on 0033** (`scripts/adr comment 0033 "…"`): "Epic 4a moved the write-off patterns into the classification table's compiled default (see the classification-config ADR). Evidence semantics unchanged; the misreading guard above still applies — IpBlockedMessage means VIDEO REMOVED. New at 4a: VideoNotAvailable10240 (606/606 probe-dead, census 2026-07-07) joined the terminal set; NoPermission stays retryable (25/452 alive)."

4. Run `scripts/adr validate` — must pass before any docs commit (the pre-commit hook enforces it anyway).

- [ ] **Step 4: Docs updates**

- `docs/operations/src-vm.md`: replace the "Operating (current, Epic 3 era…)" section body with the 4a flow — same pinned invocation plus `--retries` (default 1, note lifetime-cap semantics) and optional `--classification <file>`; remove the "do not run triage" bullet (the subcommand no longer exists) and replace with: "The pilot's parked rows are adjudicated automatically by the start-of-batch sweep on the first 4a run; expect the census to report ~3,915 swept_terminal + ~2,871 requeued + 301 parked_for_cookies (no cookies) on that first run."; add one line to Known VM facts: "The census persists in the state DB's batch_runs table with the active policy TOML — attrition documentation survives tmux."
- `docs/reference/architecture/state-machine.md`: update the failure-routing narrative — dispatch outcomes are now `record_fetch_failure` (pending | failed_retryable-exhausted | cookie-parked | stale) + inline terminal; the mutator table renames (`sweep_mark_terminal`, `sweep_requeue`, `record_fetch_failure`, `open_batch_run`/`close_batch_run`); event vocabulary additions (`retry_requeued`, `cookie_parked`, `swept_terminal`); claim ordering contract note. Fix `state-machine.md:151`'s `uu-tiktok migrate` → `ddp-transcribe migrate`.
- `docs/reference/architecture/orchestration.md`: replace the triage-flow paragraphs with the batch lifecycle (open → sweep → drain → close) and the retry dispatch; note the probe's retirement.
- `docs/reference/architecture/index.md`: Stage-5 text now describes automatic in-batch retry + sweep (no operator triage); ADR table rows for the two new ADRs + 0034 superseded; fix `index.md:44`'s `uu-tiktok ingest` and the four deepdive H1 titles (`uu-tiktok` → `ddp-transcribe`) — this closes the standing naming-sweep FOLLOWUPS item.
- `docs/FOLLOWUPS.md` + `docs/followups/epic-4.md` + `docs/archive/followups-resolved.md` per 0020: move to the archive (with this epic's resolving SHAs) the now-resolved entries — triage progress-output gap (triage deleted), census tag-annotation (census redesigned; labels documented in the policy file itself), config-echo papercut IF this epic touched it (it did not — leave it, re-target to 4b), `requeued` detail_json attempt-counts (superseded by `retry_requeued`'s detail carrying label+max_attempts — archive with note), architecture-doc naming sweep (done above). Keep: parse_watched_at (4b), operator-interface premise (mark "honored by Epic 4a" but leave as standing premise for 4b's status work).

- [ ] **Step 5: EPIC-4A-CLOSE.md**

Create `docs/superpowers/plans/2026-07-07-plan-b-epic-4a/EPIC-4A-CLOSE.md` following `docs/superpowers/plans/2026-07-07-plan-b-epic-3/EPIC-3-CLOSE.md`'s structure: task→commit(s) table (fill REAL SHAs from `git log --oneline`), verification line, and this operator runbook:

```markdown
## Operator runbook — first 4a batch (pilot DB, 7,087 parked rows)

1. Update the VM per docs/operations/src-vm.md (pull → build → sudo cp).
2. `ddp-transcribe --state-db ~/ddp-state/state.sqlite migrate` — v2→v3
   (batch_runs + index). Idempotent.
3. `ddp-transcribe --state-db ~/ddp-state/state.sqlite --transcripts
   ~/ddp-work/transcripts --whisper-model
   ~/ddp-work/models/ggml-large-v3-turbo-q5_0.bin process
   [--cookies-file ~/tiktok-cookies.txt]` — the sweep runs first
   (expect ~3,915 swept_terminal, ~2,871 requeued, 301 parked without
   cookies), then the drain retries them behind any fresh work; census
   prints at the end and persists to batch_runs.
4. `~/sync-to-storage.sh` after the batch (not while a transfer reads the
   volume).
5. Expected recovery ≈ +2,400 videos (census alive counts); corpus ≈
   91.5–92%. Exhausted/parked remainders are visible in failed_retryable
   by kind.
```

Plus a "Deferred to 4b" list (status subcommand renders batch_runs history; window/timezone; cookie-efficacy verdict from the first real cookie run).

- [ ] **Step 6: Full verification + commit + ledger**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` and `scripts/adr validate`.

```bash
git add -A
git commit -m "docs(epic-4a): close — ADR slate (retry+ordering, classification config, 0034 superseded), architecture docs, FOLLOWUPS moves, runbook"
```

(Two commits total for this task: code retirement in Step 2, docs/ADR close here — disclose the split in the report; it keeps the ADR-governed docs commit separable in review.)
