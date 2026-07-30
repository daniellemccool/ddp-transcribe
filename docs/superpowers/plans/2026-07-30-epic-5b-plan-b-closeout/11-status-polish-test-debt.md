# Task 11: status polish + closed-reply logging (Phase 3)

**Files:**
- Modify: `src/status.rs` (Epic 4b polish list), `src/pipeline/pipelined.rs` (closed-reply logging — per the Task-01 matrix ruling), test fixtures the 4b body names
- Test: the status/verify suites the body names (`tests/batch_census.rs` / status tests)

**Interfaces:**
- Consumes: FOLLOWUPS bodies — "Epic 4b final review: status polish + test-debt bundle" (in `docs/followups/epic-5.md`) and "T5-Epic1: worker-side closed-reply path silently swallows error"; the Task-01 matrix row for closed-reply (records whether the transcribe work item carries a video/request id, and the operator's ruling).
- Produces: no public-surface changes.

**Semantics (binding — full item text lives in the bodies; read them first):**
- 4b bundle: `render_event_detail_inline` non-string fallback; the missing test fixtures the body names; `run_verify` `e.ok()` miscount fix; `--respondent-id` typo must error instead of silently zero-filling; the mid-file `use` moves to the header. Each is small; TDD where an observable behavior changes (`run_verify` miscount, `--respondent-id`), suite-preservation for pure moves.
- Closed-reply logging (T5-Epic1): the worker's send-on-closed-channel path currently drops the error silently; log it with enough identity to act on. Execute per the matrix row: if the work item carries a video id, include it (`tracing::warn!(video_id, "transcribe reply channel closed; result dropped")`); if the matrix recorded that no id is available and the operator ruled to add one, thread the id per that ruling; if ruled log-without-id, do that. Never invent scope beyond the ruling.

- [ ] **Step 1: Read the two bodies + the matrix row.**
- [ ] **Step 2: Failing tests** for `run_verify` miscount and `--respondent-id` (per the body's described repro), RED → implement → GREEN; remaining items with suite-preservation evidence.
- [ ] **Step 3: Full gate** (incl. release build).
- [ ] **Step 4: Commit**

```bash
git add src/status.rs src/pipeline/pipelined.rs tests/
git commit -m "fix(status): 4b polish bundle — verify miscount, respondent-id validation, event-detail fallback, fixtures; closed-reply drop is logged with identity"
```
