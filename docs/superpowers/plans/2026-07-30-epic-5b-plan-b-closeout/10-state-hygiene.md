# Task 10: state/mod.rs hygiene bundle + `updated_at` documentation (Phase 3)

**Files:**
- Modify: `src/state/mod.rs` (the whole bundle), `tests/state_sweep.rs` / `tests/state_claims.rs` (assertion additions named below)

**Interfaces:**
- Consumes: FOLLOWUPS bodies in `docs/followups/epic-5.md` — the "state/mod.rs hygiene bundle" entry (Epic 3 final review) plus the four one-liner entries (`pragma_string`, `read_meta`, `conn`/`conn_mut`, `updated_at`); run AFTER Task 06 (both touch state/mod.rs).
- Produces: no public-surface changes beyond visibility lowering; doc-comment contract for `updated_at`.

**Semantics (binding — each item's full text lives in its FOLLOWUPS body; read them first):**
- Post-4a sweep-mutator renames; bare `tx.commit()?` sites gain `.context(...)` phrasing matching the file's style; the attempt_count==2 test assertion and the sweep capped-requeue no-event assertion land in the named test files; defensive `claimed_by`/`claimed_at` clearing where the body prescribes it (pure hygiene — claim/status SEMANTICS untouched; ADR-0023/0024-as-amended still bind).
- `Store::conn_mut` deleted (with its allow); `Store::conn`'s comment refreshed (it gained a real consumer in 5a); `pragma_string` lowered to `pub(crate)` (test reaches it via `test-helpers` per ADR-0005 if needed); `read_meta` switches to `OptionalExtension` per its body.
- **`upsert_metadata_raw` claim-guard** executes here per its Task-01 matrix ruling: if ruled "resolve", add the claim guard its FOLLOWUPS body describes (TDD); if ruled "Plan C", no code change (Task 13 re-routes the entry).
- **`updated_at` contract (operator-ruled, spec §Phase 0):** document on `UPSERT_VIDEO_SQL` + the mutators: `updated_at` records **lifecycle-mutation time; a no-op ingest is clock-neutral** (INSERT OR IGNORE leaves it untouched — the existing row-count contract per ADR-0006 stands). Verify which status mutators bump it today; where a lifecycle transition does NOT bump it, fix to match the documented contract; add one test pinning "re-ingest of an existing row does not change `updated_at`". No rename (optional per ruling — do not rename).

- [ ] **Step 1: Read the FOLLOWUPS bodies**; list each sub-item in the report as done/deviated.
- [ ] **Step 2: Failing tests first** for the testable items (attempt_count assertion, capped-requeue no-event, updated_at re-ingest neutrality) — `cargo test --test state_sweep --features test-helpers -- --test-threads=1` etc., confirm RED where behavior changes, then implement.
- [ ] **Step 3: Mechanical hygiene items** (renames, contexts, deletions, visibility) — suite-preservation evidence.
- [ ] **Step 4: Full gate** (incl. release build).
- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs tests/
git commit -m "refactor(state): Epic 3/4 hygiene bundle — renames, tx contexts, conn_mut deleted, pragma_string pub(crate), OptionalExtension; updated_at documented as lifecycle-mutation time"
```
