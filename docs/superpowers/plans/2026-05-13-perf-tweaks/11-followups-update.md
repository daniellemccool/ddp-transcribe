# Task 11 — FOLLOWUPS update: retire L47/L48/L87, amend L89

**Goal:** Move three FOLLOWUPS entries that this worktree resolved (L47, L48, L87 in `docs/FOLLOWUPS.md`'s scope index) from their per-epic sub-files to `docs/archive/followups-resolved.md` with the resolving commit SHAs backfilled. Amend L89 to acknowledge the partial-resolution via T4 (compact JSON) while keeping the drop-text-field component deferred. Update the scope-index lines accordingly. Per AD0020, this is the canonical resolve-an-entry workflow.

**Spec commit:** 10 — `docs(followups): retire L47/L48/L87, amend L89 for perf-tweaks`.

**ADRs directly relevant:**
- **AD0020** — four-file FOLLOWUPS split + archive-with-SHA at resolution.

**Files:**
- Modify: `docs/FOLLOWUPS.md` (remove 3 scope-index lines, update 1)
- Modify: `docs/followups/epic-2.md` (remove 2 entry bodies)
- Modify: `docs/followups/plan-c.md` (remove 1 entry body, amend 1)
- Modify: `docs/archive/followups-resolved.md` (append 3 resolved-with-SHA entries)

---

- [ ] **Step 1: Collect the resolving commit SHAs**

This task runs AFTER all preceding implementation/bake commits land. Run:

```bash
git log --oneline feat/perf-tweaks ^main | tac
```

Identify and record the SHAs of:
- **T2** (`refactor(transcribe): lazy-allocate lang_state ...`) — resolves FOLLOWUPS L87.
- **T4** (`perf(pipeline): write compact JSON ...`) — partially resolves FOLLOWUPS L89 (amend, do not remove).
- **T5** (`feat(process): bounded streaming subprocess capture`) — resolves FOLLOWUPS L47 + L48.

Export them for use in Step 5:

```bash
export T2_SHA=<...>
export T4_SHA=<...>
export T5_SHA=<...>
```

**Edge case:** if T9 was reverted by T10's bake (outcome c), there is no FOLLOWUPS entry to retire for T9 (the spec did not propose one). Proceed with L47/L48/L87/L89 only.

- [ ] **Step 2: Remove L47 and L48 scope-index lines from `docs/FOLLOWUPS.md`**

In `docs/FOLLOWUPS.md` under `**Epic 2 (concurrent fetch + state-machine)**`, the existing lines:

```
- T6: `process::run` unbounded stdout/stderr → Epic 2 (bounded streaming capture)
- T6: `ring_buffer_tail` misnamed → Epic 2 (rename alongside bounded-buffer work)
```

are deleted. The Epic 2 group continues with its remaining entries.

- [ ] **Step 3: Remove L87 scope-index line from `docs/FOLLOWUPS.md`**

In `docs/FOLLOWUPS.md` under `**Plan C (short-link resolution, multi-engine, storage scale)**`, the existing line:

```
- T8-Epic1: Lazy-allocate `lang_state` on first opt-in request → Plan C (multi-state / memory pressure)
```

is deleted.

- [ ] **Step 4: Update L89 scope-index line in `docs/FOLLOWUPS.md`**

Find the existing line in the Plan C group:

```
- T10-Epic1: Per-token `id`+`text` ~2× JSON artifact size → Plan C (when storage cost pinches)
```

Replace with:

```
- T10-Epic1: Per-token `id`+`text` ~2× raw_signals payload → Plan C (compact JSON landed in perf-tweaks <T4_SHA>; drop-text still deferred pending AD0010 amendment)
```

(Inline the actual short SHA for `<T4_SHA>` — e.g., `abc1234`.)

- [ ] **Step 5: Remove resolved entry bodies from sub-files**

In `docs/followups/epic-2.md`, delete the entries titled:
- `### `process::run` buffers full stderr/stdout in memory before truncation` (and its `---` separator)
- `### `ring_buffer_tail` is misnamed (it's not a ring buffer)` (and its `---` separator)

In `docs/followups/plan-c.md`, delete the entry titled:
- `### Lazy-allocate lang_state on first opt-in request` (and its `---` separator)

- [ ] **Step 6: Amend the L89 entry body in `docs/followups/plan-c.md`**

Find `### Per-token `id` + `text` roughly doubles JSON artifact size vs `{p, plog}` only`. Update the **Disposition** and add a partial-resolution paragraph:

```markdown
### Per-token `id` + `text` roughly doubles JSON artifact size vs `{p, plog}` only

**Found in:** T10 (artifact schema freeze) — implementer note.
**Disposition:** Pretty→compact JSON component landed in perf-tweaks <T4_SHA>; drop-text-field component remains deferred pending AD0010 amendment + bake validation that downstream filtering still works on `id`-only tokens.
**Trigger to revisit:** Plan C reviews artifact storage layout, OR observed shard-disk pressure during a bake.

**Partial resolution by perf-tweaks <T4_SHA>:** the `to_vec_pretty` → `to_vec` swap removed ~3× pretty-print indentation bloat from the per-token raw_signals payload. The dropping-`text`-field half of the original finding is unchanged: per AD0010's pass-through rule, downstream consumers need both `id` and `text` to filter special tokens (`[BEG]`, `[END]`, `<|en|>`, etc.) which numerically include but lexically distinguish themselves from content tokens. Dropping `text` requires either (a) an AD0010 amendment that relaxes the pass-through rule for tokens, OR (b) a sparse-token mode that keeps `text` only for special tokens. Neither is in scope for the perf-tweaks worktree.

(Original body continues unchanged below.)

T10's `RawToken` carries `id: i32` and `text: String` in addition to
`p`/`plog`, matching T9's `TokenRaw` shape exactly. ...
```

(Replace `<T4_SHA>` with the actual short SHA, twice.)

- [ ] **Step 7: Append the resolved entries to `docs/archive/followups-resolved.md`**

Append a new `## Resolved by perf-tweaks worktree (2026-05-13)` section. The full template:

````markdown
---

## Resolved by perf-tweaks worktree (2026-05-13)

Three entries resolved by the perf-tweaks worktree commits that merged before Plan B Epic 2's T11 began. Coordinated cross-session with the Epic 2 author — see `docs/superpowers/specs/2026-05-13-perf-tweaks-design.md` § Cross-session coordination.

### `process::run` buffers full stderr/stdout in memory before truncation

**Found in:** T6 code quality review (opus).
**Originally:** FOLLOWUPS L47, routed to Epic 2.
**Resolved by:** commit <T5_SHA> (`feat(process): bounded streaming subprocess capture`) on `feat/perf-tweaks`. AD0021 records the design.

`src/process.rs` previously read entire stdout AND stderr streams into `Vec<u8>` via `read_to_end` before slicing the tail; the `*_capture_bytes` field only bounded the retained excerpt, not peak memory. The perf-tweaks worktree replaced this with a streaming reader filling a `VecDeque<u8>` of size `cap`; peak retained memory is now bounded by construction. `stdout` capture got a symmetric opt-in via `stdout_capture_bytes`; `CommandOutcome::stdout` is now `Option<Vec<u8>>` (None = intentionally discarded). Cross-session coordination: Plan B Epic 2's T13 inherits the design and may add per-tool stdout defaults on top of AD0021.

---

### `ring_buffer_tail` is misnamed (it's not a ring buffer)

**Found in:** T6 code quality review (opus).
**Originally:** FOLLOWUPS L48, routed to Epic 2.
**Resolved by:** same commit <T5_SHA>. The helper is removed; capture is bounded by construction rather than by post-hoc tail-slicing. No rename needed.

---

### Lazy-allocate lang_state on first opt-in request

**Found in:** T8-Epic1 (lang_probs opt-in) — codex-advisor code-quality review.
**Originally:** FOLLOWUPS L87, routed to Plan C.
**Resolved by:** commit <T2_SHA> (`refactor(transcribe): lazy-allocate lang_state on first opt-in request`) on `feat/perf-tweaks`. Brought forward from Plan C scope.

`WhisperEngine` worker thread previously allocated `lang_state` unconditionally at startup; non-opt-in workers paid ~500MB-1GB VRAM/host overhead for an unused state. Replaced with `Option<WhisperState>` lazily allocated on the first request with `compute_lang_probs=true`. AD0016 invariant preserved (state stays inside the worker thread). New `tests/transcribe_lang_state.rs` asserts via an `Arc<AtomicUsize>` counter that non-opt-in workers never allocate and that opt-in workers allocate exactly once.
````

(Replace `<T5_SHA>` and `<T2_SHA>` with the actual short SHAs.)

- [ ] **Step 8: Verify the deletions and amendment are consistent**

Run:
```bash
# Confirm no orphan references to the deleted L47/L48/L87 lines.
grep -n "process::run.*unbounded\|ring_buffer_tail misnamed\|Lazy-allocate.*lang_state" docs/FOLLOWUPS.md docs/followups/

# Confirm the archive carries the three entries with SHAs.
grep -A 2 "Resolved by perf-tweaks worktree" docs/archive/followups-resolved.md | head -10

# Confirm L89's scope-index line is updated.
grep "T10-Epic1" docs/FOLLOWUPS.md
```

Expected:
- No grep hits in the first command (the deleted entries are gone from active scope).
- The archive has the new section.
- L89's scope-index line shows the amended text.

- [ ] **Step 9: Commit**

```bash
git add docs/FOLLOWUPS.md docs/followups/epic-2.md docs/followups/plan-c.md docs/archive/followups-resolved.md
git commit -m "$(cat <<'EOF'
docs(followups): retire L47/L48/L87, amend L89 for perf-tweaks

Per AD0020's archive-at-resolution workflow:

Resolved (moved to docs/archive/followups-resolved.md with SHAs):
- L47: process::run unbounded stdout/stderr capture
  → resolved by <T5_SHA> (T5 bounded streaming + AD0021).
- L48: ring_buffer_tail misnamed
  → resolved by same <T5_SHA> via elimination (no rename).
- L87: Lazy-allocate lang_state on first opt-in request
  → resolved by <T2_SHA> (T2 Option<WhisperState> + Arc counter).

Amended in place (docs/followups/plan-c.md):
- L89: Per-token id+text ~2× JSON artifact size
  → partial resolution: pretty→compact JSON landed in <T4_SHA>
  (T4 perf-tweaks). Drop-text-field component still deferred to
  Plan C pending AD0010 amendment + bake validation.

Cross-session coordination: this worktree resolved FOLLOWUPS that
Plan B Epic 2 was scheduled to address (L47, L48 were Epic 2-routed).
Epic 2's T13 will inherit AD0021 without re-authoring it.

Refs: AD0020

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

(Replace each `<SHA>` placeholder with the actual short hash captured in Step 1.)

---

## Self-check

- [ ] L47, L48, L87 scope-index lines are gone from `docs/FOLLOWUPS.md`.
- [ ] L89's scope-index line reflects the amendment (partial resolution + remaining deferral).
- [ ] L47, L48 entry bodies are gone from `docs/followups/epic-2.md`.
- [ ] L87 entry body is gone from `docs/followups/plan-c.md`.
- [ ] L89 entry body in `docs/followups/plan-c.md` has the amendment paragraph + updated Disposition.
- [ ] `docs/archive/followups-resolved.md` carries the three resolved entries in a new "Resolved by perf-tweaks worktree (2026-05-13)" section with backfilled SHAs.
- [ ] This is the LAST commit on `feat/perf-tweaks` before the merge to main.
