# Task 09: sync-IO audit + policy ADR + application (Phase 3)

**Files:**
- Create (via adg ONLY): new lean ADR — synchronous-IO-in-async-paths policy
- Modify: files the audit classifies as "move" (expected: `src/ingest.rs`, `src/output/artifacts.rs`, `src/fetcher/ytdlp.rs`, `src/pipeline/*.rs` — final list comes from the audit itself)
- Modify: `src/ingest.rs` `walk_recursive` polish, `src/output/artifacts.rs` `shard_distributes_uniformly` comment, `src/output/` `shard_dir` deletion (FOLLOWUPS bodies in `docs/followups/epic-5.md` — read the four relevant entries first)

**Interfaces:**
- Consumes: the post-Task-04 tree; `docs/followups/epic-5.md` bodies for the bundled polish items.
- Produces: the policy record later work cites; no public-surface changes.

**Semantics (binding):**
- **Audit first, policy second, edits third.** The audit inventories every `std::fs`/blocking call reachable from an async fn (`rg -n 'std::fs::|File::|fsync|sync_all' src/` cross-checked against async callers) and classifies each: (a) startup/CLI path, not under the runtime → stays sync; (b) bounded small blocking under tokio (e.g. rusqlite behind `&mut Store` — inherently sync, serialized by design) → stays sync WITH rationale; (c) potentially long blocking on the hot path (WAV decode, durable artifact writes + dir fsyncs, yt-dlp work-dir scans/removals) → `spawn_blocking` or `tokio::fs`, judged per site. The classification table goes in the ADR's Why/Guidance (or an appendix table the record points to).
- The ADR records the POLICY (when each treatment applies) — not a site-by-site changelog; review rejects future naked `std::fs` in async fns without a policy citation. ADR-0008's artifact ordering (write both artifacts before `mark_succeeded`) and its lock-free `write_artifacts_durable` contract are untouched — any `spawn_blocking` wrapping preserves ordering and error propagation exactly.
- Apply ONLY class-(c) moves. Each move is behavior-preserving: same errors, same ordering, same counters; suite must not change assertions.
- Bundle polish (per their FOLLOWUPS bodies, binding content lives there): `walk_recursive` polish; `shard_distributes_uniformly` rationale comment refresh; delete unused `shard_dir` + its allow.
- **PR #23 ingest-ledger hardening** executes here per its Task-01 matrix rulings (the body routes it to "ingest/sync-IO sweep"): basename-only key collision, 1s-resolution fingerprint change detector, and the missing mid-tx rollback test — implement exactly what each ruling says (fix vs accepted-archive), TDD for any behavior change.

- [ ] **Step 1: Audit.** Produce the classification table (path:line, class, rationale) — goes into the ADR draft.
- [ ] **Step 2: ADR** via `write-adr:write-lean-adr` / `adg lean new --from-stdin` (`applies_to`: the audited files). `adg lean index` + `check` — 0 failures.
- [ ] **Step 3: Failing-test check.** Class-(c) moves are behavior-preserving, so no new tests; the existing suites are the harness. Run the focused suites for each file you touch BEFORE editing (record counts), then after — identical results required.
- [ ] **Step 4: Apply moves + bundle polish.** Delete `shard_dir` and its `#[allow(dead_code)]` (ADR-0002); comment refreshes per the bodies.
- [ ] **Step 5: Full gate** (incl. release build).
- [ ] **Step 6: Commit**

```bash
git add docs/decisions/ src/
git commit -m "feat(io): sync-IO policy ADR + hot-path spawn_blocking/tokio::fs moves; walk_recursive and shard polish; shard_dir deleted"
```
