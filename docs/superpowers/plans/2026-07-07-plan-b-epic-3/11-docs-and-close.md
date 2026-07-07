# Task 11: Accept ADRs, update architecture docs, FOLLOWUPS lifecycle, epic close

**Files:**
- Modify (via tool): `docs/decisions/0033/0034/0035` → `accepted`
- Modify: `docs/reference/architecture/state-machine.md`, `docs/reference/architecture/orchestration.md`, `docs/reference/architecture/index.md`
- Modify: `docs/FOLLOWUPS.md`, `docs/followups/epic-3.md`, `docs/archive/followups-resolved.md`
- Create: `docs/superpowers/plans/2026-07-07-plan-b-epic-3/EPIC-3-CLOSE.md`

**Interfaces:**
- Consumes: everything landed in Tasks 01–10 (cite real commit SHAs — collect them with `git log --oneline` at execution time).
- Produces: a consistent written record; the epic-close artifact per 0019.

- [ ] **Step 1: Accept the three ADRs**

```bash
scripts/adr decide 0033 "Evidence-derived enums" "65k-run corpus + oEmbed probe evidence (n=36) made the speculative variant list unnecessary and showed message text is inverted on the dominant classes; inline write-off of the two probe-validated dead classes accepted by operator ruling 2026-07-07"
scripts/adr decide 0034 "Single operator-driven triage subcommand" "keeps the pipeline hot path network-pure; message-class fast path + oEmbed probe gave perfect separation; requeue-with-kind-write-back normalizes historical rows without wasted refetches"
scripts/adr decide 0035 "Cookies scoped to SensitiveLoginGated retries" "research-validity rationale (crime/policing content skews sensitive) justifies cookie support; retry-only scope caps account exposure at ~300 fetches"
scripts/adr validate
```

(Option text must match a `## Considered Options` bullet closely enough for `adg` to resolve it — check with `scripts/adr view 0033` first.)

- [ ] **Step 2: Update `state-machine.md`**

- Lifecycle-states section: `failed_retryable` is no longer "a sink"; it has two triage-driven exits. Replace the "Currently a sink … Epic 3 retry-policy charter" sentence with the shipped reality and the ADR 0034 reference.
- State-transition diagram: add edges `failed_retryable → failed_terminal` (label `triage_mark_terminal — triage verdict: dead`) and `failed_retryable → pending` (label `requeue_retryable — triage verdict: alive, attempt_count < cap`).
- Mutator table: add `triage_mark_terminal`, `requeue_retryable`, `list_failed_retryable` rows with their predicates; update `mark_terminal_failure`'s "no caller" note (first caller: pipeline write-off dispatch, Task 07 SHA).
- Failure-classification section: replace the "string-kind only" paragraph — kinds are now taxonomy tags from `src/failure.rs` (ADR 0033); note the two write-off classes and the `video_events` types `triaged_terminal` / `requeued`.

- [ ] **Step 3: Update `orchestration.md` + `index.md`**

- `orchestration.md`: failure-routing section now describes three-arm dispatch (`classify_fetch_phase` / `classify_transcribe_error` → Retryable / Unavailable / Bug), the T16 fetch cancellation wrap, and cookie-scoped fetch opts (one paragraph each; deepdive pointers to ADRs 0033/0035).
- `index.md` §4 ADR map: add rows for 0033, 0034, 0035 with one-line scopes.
- Skim both docs for now-false statements (search "sink", "no caller", "placeholder kind", "Fetch\"", "Epic 3"): fix each.

- [ ] **Step 4: FOLLOWUPS lifecycle (per 0020)**

Move to `docs/archive/followups-resolved.md` **with resolving commit SHAs** (from `git log --oneline`), and delete from `docs/followups/epic-3.md` + the scope index in `docs/FOLLOWUPS.md`:

- `From<RunError>` collapse (T6) — Task 02 SHA
- `status.code().unwrap_or(-1)` signal loss (T6) — Task 02 SHA
- `claim_next`/`mark_succeeded` `with_context` (T10) — Task 04 SHA
- `YtDlpFetcher::acquire` error mapping, findings 1–2 (T11) — Task 02 SHA; findings 3–4 remain: re-file finding 3 under Epic 5 (fetch hardening) and leave finding 4's Plan C routing — split the entry rather than archiving it whole
- `pipeline_fakes.rs` split + over-narration — Task 06 SHA
- Worker-level entry-point audit — Task 06 SHA (audit verdicts inline; note replacement candidates remain opportunistic)
- `From<AudioDecodeError>` → Bug (T5-Epic1) — Task 02 SHA
- `fetch_worker` cancellation latency (T16) — Task 07 SHA
- Plan-brief library-API drift — archive with the plan-directory commit SHA **only if** the checklist was demonstrably applied (the overview's "verified at plan-writing time" notes are the evidence); otherwise leave with a note
- Tier-5 failure-corpus entry — Task 03 SHA (fixtures committed); fold in the stale-claims corrections: stderr IS captured since 0021, share-link hypothesis refuted 2026-07-07 (56,600/56,620 share-form at 87.5% success; 10/10 share-form re-fetch OK)

- [ ] **Step 5: Write `EPIC-3-CLOSE.md`** (≤1 page, per 0019)

Contents: what landed (task → SHA table); the operator runbook for the production DB (`triage --dry-run` → review census → `triage` → `process --cookies-file …` → expected recovery ≈ +2,400 videos, 87.5% → ~91.5%); deferred items (worker-test replacement candidates, `run_serial` retirement → Epic 5; ffprobe-class investigation if any rows survive triage); pointer to Epic 4 sketch as next.

- [ ] **Step 6: Verify + commit**

```bash
scripts/adr validate
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1
git add docs/
git commit -m "docs: Epic 3 close — ADRs accepted, architecture docs updated, FOLLOWUPS archived with SHAs"
```

Then run `superpowers:verification-before-completion` and `superpowers:finishing-a-development-branch` (merge/PR decision belongs to the operator).
