# Task 05: requeue-failures contract ADR + ADR-0036 amendment (Phase 2)

**Files:**
- Create (via adg ONLY): new lean ADR — the `requeue-failures` operator override contract
- Modify (via write-lean-adr amendment workflow): `docs/decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md`

**Interfaces:**
- Consumes: spec §`requeue-failures` (rev 4 — its contract bullets are the binding content); Task 02's precedent for adg amendment flow.
- Produces: the accepted contract Task 06 implements verbatim.

**Semantics (binding — the record must carry ALL of this substance):**
- **Carve-out (verbatim target, per operator ruling 2026-07-30):** "ADR-0036 remains the normal retry authority. An operator may explicitly restore failed rows to pending after an external condition has materially changed. This is a forensic, default-deny override of eligibility, not an alternate classifier or retry scheduler; the subsequent fetch remains the liveness oracle."
- Selector grammar exactly as the spec: qualifying = repeatable `--error-kind <K>` (no comma-splitting; exact byte-equality match, no case folding), `--max-attempts <N>` (skip rows `attempt_count >= N`), `--older-than <DUR>`; modifiers (never satisfy default-deny) = `--max <N>`, `--dry-run`; bare invocation errors; `--all` = all retryables, conflicts with every qualifying selector; `--include-terminal` requires a qualifying terminal selector (`--include-terminal --all` and `--include-terminal --max N` rejected); retryable kinds match `last_retryable_kind`, terminal matching uses `terminal_reason`; `--max`/`--max-attempts` range-checked positive.
- Failure clock: `last_failure_at := MAX(video_events.at)` over allowlist `'failed_retryable','failed_terminal','retry_requeued','cookie_parked'`; administrative events (`'requeued'`, `'swept_stale'`, `'swept_terminal'`, `'claimed'`, `'succeeded'`) never reset it; one `now` per invocation; no qualifying events ⇒ no `--older-than` match; `videos.updated_at` untouched.
- Transition: eligible rows → `pending`, defensively clearing `claimed_by`/`claimed_at`; `attempt_count` never reset/decremented; `last_retryable_*`/terminal fields retained. One `operator_requeued` event per row: prior status, prior kind/reason, attempt count, attribution `operator:<hostname>-<pid>`.
- **Post-override arithmetic with example:** next claim bumps `A → A+1`; auto-requeue needs `A+1 < retries+1` ⇒ `--retries > A` strictly (`--retries = A` insufficient). Example: exhausted at `A=3` under `--retries 2` ⇒ one forced attempt unless `process --retries 4`+.
- Contract details the record fixes: single IMMEDIATE transaction (select → update → events, the `sweep_stale_claims` shape); deterministic `--max` ordering = `ORDER BY attempt_count ASC, video_id ASC` (mirrors 0036's claim-order family); dry-run = read-only SELECT, prints per-kind counts + total, writes nothing; zero matches ⇒ exit 0 with an explicit "0 rows matched" line; sequencing note ("bypasses the start-of-batch sweep; after the next ordinary claim — the command itself claims nothing — ordinary 0036 behavior resumes").
- **0036 amendment:** add the carve-out bullet referencing the new record; add the manual-SQL note (unsupported emergency repair unless it preserves the forensic event invariant). The blind-sweep/no-probe/claim-ordering decisions are untouched.

- [ ] **Step 1:** Load `write-adr:write-lean-adr`; author the new record via `adg lean new --from-stdin` (`applies_to`: `src/state/mod.rs`, `src/cli.rs`, `src/commands.rs`).
- [ ] **Step 2:** Amend ADR-0036 per the skill's amendment path.
- [ ] **Step 3:** `adg lean index --root .` + `adg lean check` on both — 0 failures.
- [ ] **Step 4: Verification** — full gate (docs-only; stays green).
- [ ] **Step 5: Commit**

```bash
git add docs/decisions/
git commit -m "docs(adr): requeue-failures — forensic default-deny eligibility override; 0036 amended with the carve-out and --retries > A arithmetic"
```
