# Task 14 — Bounded `process::run` capture: verify perf-tweaks' 0021 covers Epic 2's needs

**Goal:** Confirm that perf-tweaks' `0021` (bounded subprocess output capture via streaming `VecDeque<u8>`) — already merged into `main` and inherited via the Epic 2 rebase — fully covers what Epic 2 originally planned for this slot. **This is a verify-only checkpoint task.** No Epic 2 source changes are expected; no Epic 2 ADR is authored here.

**ADRs touched:** 0021 (inherited from perf-tweaks; no Epic 2 ADR).

**Files:**
- None modified in the expected case. (If verification turns up a gap, that gap becomes its own follow-on commit, but the brief assumes the perf-tweaks implementation is complete per its commit body `9e84b54`.)

**Pre-reqs:** T12 closed (Phase 2 ADRs landed). Perf-tweaks merged on `main` (commit `d03173d`); Epic 2's rebase picked it up. T13 (tokio-util dep) does not block this verify.

**Background:** the original Epic 2 brief for this slot drafted its own bounded-capture ADR (was 0026 in the pre-rebase numbering) and prescribed implementing streaming-bounded `VecDeque<u8>` in `src/process.rs`, symmetric `stdout_capture_bytes` on `CommandSpec`, and a rename `ring_buffer_tail` → `tail_excerpt`. Perf-tweaks shipped all of that — and went one step further by *removing* `ring_buffer_tail` entirely (capture is bounded by construction in the streaming reader; the post-hoc tail-slicing helper became dead weight). Per perf-tweaks' commit body: *"Plan B Epic 2's T13 anticipated this work; it can now absorb only symmetric stdout policy decisions … on top of 0021 without re-authoring the design."* Empirically there are no leftover symmetric-policy decisions to make: `CommandOutcome::stdout` is `Option<Vec<u8>>` (opt-in via `stdout_capture_bytes`), and `fetcher/ytdlp.rs` already sets `stdout_capture_bytes: 0`. Nothing for Epic 2 to add.

---

- [ ] **Step 1: Confirm the streaming-bounded `VecDeque<u8>` pattern is in place**

```bash
grep -n 'VecDeque<u8>\|stdout_capture_bytes\|stderr_capture_bytes' src/process.rs | head -20
```

Expected: matches showing `pub stdout_capture_bytes: usize` and `pub stderr_capture_bytes: usize` on `CommandSpec`, plus a `VecDeque<u8>` type used in the streaming reader. If any of these are absent, perf-tweaks didn't ship what its commit claimed — stop and surface to the operator before proceeding.

- [ ] **Step 2: Confirm `ring_buffer_tail` is gone**

```bash
grep -rn 'ring_buffer_tail' src/ tests/
```

Expected: empty. Perf-tweaks removed the helper outright (not renamed) because the streaming reader bounds capture by construction. If any reference survives in `src/` or `tests/`, file a follow-on issue.

- [ ] **Step 3: Run the bounded-capture test suite**

```bash
cargo test --features test-helpers --test process_bounded_capture
```

Expected: 5 tests pass (per perf-tweaks' commit body): `stderr_excerpt_preserves_tail_when_subprocess_overflows_cap`, `stdout_is_none_when_capture_bytes_zero`, `stdout_is_some_when_capture_bytes_nonzero`, `exit_code_passes_through_bounded_capture`, `read_bounded_peak_len_never_exceeds_cap`.

- [ ] **Step 4: Confirm the call-site convention**

```bash
grep -nA1 'CommandSpec {' src/fetcher/ytdlp.rs | head -20
```

Expected: `stdout_capture_bytes: 0` (yt-dlp writes audio to a file; stdout was already unused). Any value other than 0 here would mean perf-tweaks made a different per-tool policy choice — read its commit message for the rationale and confirm Epic 2 has no reason to override.

- [ ] **Step 5: Record T14 closure in `PHASE-2-CLOSE.md`**

This task produces no source commit. Note in `PHASE-2-CLOSE.md` (or whatever close-out artifact the Phase 2 controller writes per 0019) that T14 verified perf-tweaks' `0021` implementation cleared the slot, and no Epic 2 ADR was authored. Cite perf-tweaks' commits `9e84b54` (impl) and `118d7e2` (ADR) for traceability.

---

## Self-check

- [ ] `src/process.rs` has streaming-bounded `VecDeque<u8>` capture with `stdout_capture_bytes` and `stderr_capture_bytes` on `CommandSpec`
- [ ] `ring_buffer_tail` is absent from `src/` and `tests/`
- [ ] `cargo test --features test-helpers --test process_bounded_capture` passes
- [ ] `fetcher/ytdlp.rs` sets `stdout_capture_bytes: 0`
- [ ] No new ADR drafted; `0021` (inherited from perf-tweaks) is the sole bounded-capture decision
- [ ] T14 closure noted in Phase 2 close-out artifact

## Why this task still exists at all

Three reasons to keep T14 as an explicit verify-only step instead of deleting it from the index:

1. **Numbering stability** — T15–T20 file numbers stay anchored. Removing T14 would shift them and create churn against the spec's task references.
2. **Closing a planning loop** — Epic 2 originally planned to ship bounded capture itself. Recording that perf-tweaks subsumed the work (rather than silently dropping the slot) is operational-record discipline.
3. **Insurance** — if the empirical verification turns up a gap (perf-tweaks shipped X but Epic 2's callers need Y), this task is where the gap gets characterized and either closed or punted to FOLLOWUPS. The expected case is "no gap, close immediately"; an unexpected case is also handled gracefully.
