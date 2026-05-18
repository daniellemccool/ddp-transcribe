# Task 20 — A10 bake: N=3 vs N=1 throughput; coordinated-shutdown drill

**Goal:** Operational validation on the SRC A10 workspace. Two measurements: (a) throughput comparison N=3 vs N=1 (serial) against the `news_orgs` fixture; capture wallclock + per-stage breakdown; (b) coordinated-shutdown drill: `kill -KILL` the running process mid-batch, restart, confirm the sweep recovers in-progress rows. Append findings to `docs/SRC-BAKE-NOTES.md`.

**ADRs touched:** 0027 (validates the N=3 default).

**Files:**
- Modify: `docs/SRC-BAKE-NOTES.md` (append a Phase 2 section)
- No source changes.

**Pre-reqs:** Phase 2 implementation complete (T12–T19). A10 workspace SSH access. `news_orgs` fixture present.

---

- [ ] **Step 1: Build the binary on the A10 workspace**

```bash
ssh <a10-workspace>
cd /path/to/uu-tiktok
git checkout feat/plan-b-epic-2
cargo build --release --features cuda
```

Expected: release binary at `./target/release/uu-tiktok`.

If the build fails on CUDA: capture the error in SRC-BAKE-NOTES and stop. The CUDA build dynamic is the same as Epic 1's bake (T13 of Plan B Epic 1 covers operator runbook).

- [ ] **Step 2: Reset DB to a known state**

```bash
rm -f /path/to/state.sqlite
./target/release/uu-tiktok init
./target/release/uu-tiktok ingest
```

Confirm the `news_orgs` fixture's videos are upserted (`sqlite3 state.sqlite 'SELECT COUNT(*) FROM videos WHERE status="pending"'` should match the fixture's video count, typically 8–20 depending on which subset was selected for the bake).

- [ ] **Step 3: Bake run — N=1 (serial baseline)**

```bash
time UU_TIKTOK_DOWNLOAD_WORKERS=1 ./target/release/uu-tiktok \
    --whisper-model models/ggml-large-v3-turbo-q5_0.bin \
    --compute-lang-probs \
    process
```

Capture:
- Total wallclock
- Per-video timing from `tracing::info!` logs (claim → wav acquired → transcribed → succeeded)
- Final `process complete` stats line (claimed/succeeded/failed)

- [ ] **Step 4: Reset and bake run — N=3 (default)**

```bash
sqlite3 state.sqlite "UPDATE videos SET status='pending', claimed_by=NULL, claimed_at=NULL, succeeded_at=NULL, duration_s=NULL, language_detected=NULL, fetcher=NULL, transcript_source=NULL WHERE status='succeeded'"
rm -rf transcripts/

time ./target/release/uu-tiktok \
    --whisper-model models/ggml-large-v3-turbo-q5_0.bin \
    --compute-lang-probs \
    process
```

(The `UPDATE` is the operator's "reset" gesture for re-running the bake on the same fixture; T20 doesn't introduce a `reset` CLI — that's Epic 5.)

Capture the same metrics. Compute the speedup ratio (N=1 wallclock / N=3 wallclock); expected ~3.5×.

- [ ] **Step 5: Bake run — N=5 (sanity check, marginal returns)**

```bash
sqlite3 state.sqlite "UPDATE videos SET status='pending', ..."  # same reset
rm -rf transcripts/

time UU_TIKTOK_DOWNLOAD_WORKERS=5 ./target/release/uu-tiktok \
    --whisper-model models/ggml-large-v3-turbo-q5_0.bin \
    --compute-lang-probs \
    process
```

Capture wallclock. Confirm marginal-or-no improvement vs N=3 (per 0027 throughput math). If N=5 substantially outperforms N=3, that's a finding to record — 0027's default may need revision. Document either way.

- [ ] **Step 6: Coordinated-shutdown drill**

The drill: start `process` with N=3, kill -KILL one of the workers mid-batch, then restart `process` and confirm the sweep recovers in-progress rows.

```bash
# Reset state
sqlite3 state.sqlite "UPDATE videos SET status='pending', ..."
rm -rf transcripts/

# Start a long-running process in the background (use N=1 so a single
# kill is observable; or N=3 and kill the binary entirely).
./target/release/uu-tiktok \
    --whisper-model models/ggml-large-v3-turbo-q5_0.bin \
    process &
PID=$!
sleep 5  # let it claim a row and start fetching

# Kill -9 the whole process tree.
kill -KILL $PID
wait

# Inspect: there should be in_progress rows with non-NULL claimed_at.
sqlite3 state.sqlite "SELECT video_id, status, claimed_at FROM videos WHERE status='in_progress'"
```

Expected: one or more in_progress rows with `claimed_at` populated.

```bash
# Restart with a SHORT threshold so the sweep fires immediately on
# rows that look "stale" even though they're only seconds old.
./target/release/uu-tiktok \
    --whisper-model models/ggml-large-v3-turbo-q5_0.bin \
    --stale-claim-threshold 1s \
    process
```

Expected log line: `sweep_stale_claims recovered abandoned rows` with `recovered > 0`. The orchestrator then re-claims those rows (now `pending` again) and processes them to success.

Final DB state: all rows either `succeeded` or (if a row failed transiently) `failed_retryable`.

- [ ] **Step 7: Append findings to `docs/SRC-BAKE-NOTES.md`**

Add a "Plan B Epic 2 bake" section at the bottom (matching the format of the existing Epic 1 bake section):

```markdown
## Plan B Epic 2 — Pipelined orchestrator (2026-XX-XX, A10 workspace)

### Throughput comparison (large-v3-turbo-q5_0, `news_orgs` fixture, n=<N>)

| Topology | Wallclock | Avg s/video | Speedup vs serial |
|----------|-----------|-------------|-------------------|
| N=1 (serial) | <Xs> | <Xs> | 1.00× |
| **N=3 (default)** | **<Xs>** | **<Xs>** | **<X.XX×>** |
| N=5 | <Xs> | <Xs> | <X.XX×> |

**Finding 1**: N=3 vs N=1 speedup is <X.XX×> (predicted ~3.5×; <within | exceeds | below expectation>).
**Finding 2**: N=5 vs N=3 delta is <X%>; <consistent with curve-flattening prediction in 0027 | suggests revisiting the default>.

### Coordinated-shutdown drill

- Started `process` with N=3; let it claim 2 rows.
- `kill -KILL` the binary mid-batch.
- Pre-restart DB state: 2 rows `in_progress` with non-NULL claimed_at.
- Restart with `--stale-claim-threshold 1s`.
- Observed log line: `sweep_stale_claims recovered abandoned rows recovered=2`.
- Recovered rows re-claimed and processed to success.
- Final DB state: all rows `succeeded`.

**Finding 3**: 0025 shutdown order + 0024 sweep work as designed; no manual operator intervention needed beyond restarting `process`.

### Resource envelope

- Peak memory at N=3: <X MB> (Mutex<Store> contention not measured; observed under typical bake load no per-worker stalls).
- Peak channel depth observed: <X> (predicted ≤6 at N=3 + capacity 2).
- ffmpeg subprocess concurrency: <X> at peak (predicted ≤6).
```

Fill `<X>` placeholders with measured values.

- [ ] **Step 8: Commit the bake notes**

```bash
git add docs/SRC-BAKE-NOTES.md
git commit -m "$(cat <<'EOF'
docs(bake): Plan B Epic 2 throughput + coordinated-shutdown findings on A10

Throughput (large-v3-turbo-q5_0, news_orgs fixture):
- N=1: <wallclock>s, 1.00×
- N=3 (default): <wallclock>s, <speedup>×
- N=5: <wallclock>s, <speedup>×

N=3 vs N=1 speedup matches 0027's ~3.5× prediction within <margin>%.
N=5 shows <marginal | substantial> improvement vs N=3 — 0027's
default remains correct / needs revision per Finding 2.

Coordinated-shutdown drill: kill -KILL mid-batch → restart with short
stale-claim threshold → sweep recovers in-progress rows → rerun
succeeds. 0025 shutdown order + 0024 sweep both validated.

Refs: 0024, 0025, 0027

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 9: FOLLOWUPS resolution sweep**

Move resolved entries from `docs/followups/epic-2.md` to `docs/archive/followups-resolved.md` with the resolving commit SHAs. Per 0020:

- `Store::open` schema-version not read → T2/T3 (record SHAs)
- `concurrent_claim_serializes_via_begin_immediate` doesn't race → T10
- `mark_succeeded` predicate + missing round-trip → T5
- `claim_next` polling semantics → T12 (0026)
- `process::run` unbounded → T14
- `ring_buffer_tail` misnamed → T14
- WhisperEngine teardown can hang → T18 (0025)
- Config dead fields (whisper_use_gpu/whisper_threads) → T18

Update `docs/FOLLOWUPS.md` scope-index to remove the corresponding lines under "Epic 2".

```bash
git add docs/FOLLOWUPS.md docs/followups/epic-2.md docs/archive/followups-resolved.md
git commit -m "$(cat <<'EOF'
docs(followups): Epic 2 resolution sweep — archive entries resolved by T2-T18

Per 0020, move resolved FOLLOWUPS entries from docs/followups/epic-2.md
to docs/archive/followups-resolved.md with resolving commit SHAs. Update
the scope-index in docs/FOLLOWUPS.md.

Entries archived:
- T7: Store::open schema-version not read → T2/T3
- T10: concurrent_claim doesn't race → T10
- T10: mark_succeeded predicate + round-trip → T5
- T10: claim_next polling semantics → 0026
- T6: process::run unbounded → T14
- T6: ring_buffer_tail misnamed → T14
- T5: WhisperEngine teardown → 0025
- (Config dead fields → T18)

Refs: 0020

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 10: Mark Epic 2 complete**

Optional close-out artifact (mirroring Epic 1's): consider a brief `docs/superpowers/plans/2026-05-13-plan-b-epic-2/EPIC-2-CLOSE.md` that summarizes what landed, the bake findings, and the entry point for Epic 3 (failure classification).

```bash
git push origin feat/plan-b-epic-2
gh pr create --base main --head feat/plan-b-epic-2 --title "Plan B Epic 2 — state machine + pipelined orchestrator" --body "$(cat <<'EOF'
## Summary

- Phase 1: state machine on serial loop (schema-version policy, migrate
  subcommand, retryable/terminal mutator surface, stale-claim sweep,
  classifier wiring into run_serial).
- Phase 2: pipelined orchestrator (N=3 fetch + 1 transcribe over bounded
  mpsc, JoinSet + CancellationToken supervision, load-bearing shutdown
  ORDER per 0025).
- Seven new ADRs: 0022–0027.
- Bake findings appended to docs/SRC-BAKE-NOTES.md (N=3 throughput,
  coordinated-shutdown drill).
- FOLLOWUPS Epic 2 entries resolved and archived.

## Test plan

- [ ] cargo test --features test-helpers (all green)
- [ ] cargo test --features cuda,test-helpers e2e_real_tools -- --ignored (A10)
- [ ] Manual A10 bake reproduces SRC-BAKE-NOTES numbers
- [ ] migrate subcommand brings a v1 DB to v2 cleanly

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-check

- [ ] N=3 wallclock ≤ 1/3 of N=1 wallclock (Finding 1 within tolerance)
- [ ] Coordinated-shutdown drill: sweep recovers `recovered ≥ 1` rows; restart succeeds
- [ ] No row left in `in_progress` after the sweep+restart cycle
- [ ] `docs/SRC-BAKE-NOTES.md` has a Plan B Epic 2 section with all `<X>` placeholders filled
- [ ] FOLLOWUPS Epic 2 group is empty (all entries archived) OR clearly notes which entries roll forward to Epic 3
- [ ] PR opened against `main` (operator decides when to merge)
