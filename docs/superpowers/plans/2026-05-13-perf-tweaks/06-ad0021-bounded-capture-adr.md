# Task 6 — Author AD0021 (Bounded subprocess output capture)

**Goal:** Record the design decision from T5 as `AD0021` via the project's `adg` workflow. The pre-commit hook (`.githooks/pre-commit`) validates ADR consistency, so the ADR file must round-trip through `adg --model docs/decisions validate`.

**Spec commit:** 5 — `docs(adr): add AD0021 bounded subprocess capture`.

**ADRs directly relevant:**
- **AD0001** — feature-derived ADR; rides the feat branch.
- **AD0021** — the ADR being authored. Title locked in spec: "Bounded subprocess output capture via streaming `VecDeque<u8>`".

**Numbering note for the implementer.** The Epic 2 sketch at `docs/superpowers/plans/2026-05-12-plan-b/EPIC-2-SKETCH.md` anticipated this ADR as AD0023. AD0018, AD0019, and AD0020 (meta-process ADRs) landed on `main` after the sketch was written, making 21 the next available number at plan write. Before running `adg add`, verify:

```bash
ls docs/decisions/*.md | grep -oE 'AD00[0-9]+' | sort -u | tail -3
```

Expected output ends at `AD0020`. If `AD0021` already exists (e.g., another in-flight worktree took it), **stop and re-coordinate** — Epic 2's session may have started authoring its remaining ADRs early.

**Files:**
- Add: `docs/decisions/AD0021-bounded-subprocess-output-capture-via-streaming-vecdeque.md` (via `adg add`)
- Modify: `docs/decisions/index.yaml` (auto-updated by `adg add`)

---

- [ ] **Step 1: Confirm AD0021 is available**

Run:
```bash
ls docs/decisions/*.md | grep -oE 'AD00[0-9]+' | sort -u | tail -3
adg --model docs/decisions validate
```

Expected:
- Last three ADRs are AD0018, AD0019, AD0020 (no AD0021 yet).
- `adg validate` reports clean.

If AD0021 already exists, **stop and resolve with the operator** before proceeding.

- [ ] **Step 2: Create the ADR shell via `adg add`**

Run:
```bash
adg --model docs/decisions add \
  --title "Bounded subprocess output capture via streaming VecDeque<u8>"
```

Expected: a new file at `docs/decisions/AD0021-bounded-subprocess-output-capture-via-streaming-vecdeque.md` (or similar slugified path). `docs/decisions/index.yaml` is automatically updated with the new ADR ID and title.

Verify:
```bash
ls docs/decisions/AD0021-*.md
adg --model docs/decisions list --id 0021
```

Expected: the file exists; `list` shows the new ADR with status `open`.

- [ ] **Step 3: Fill in the body sections via `scripts/adr-fill`**

The helper reads QUESTION (context), OPTION (×N considered options), and CRITERIA (decision drivers) sections from stdin and pipes them into `adg edit`. Run:

```bash
scripts/adr-fill 0021 <<'BODY'
QUESTION:
`src/process.rs::run` previously read child stdout AND stderr each into unbounded `Vec<u8>` via `tokio::io::AsyncReadExt::read_to_end`, then sliced an 8 KiB tail of stderr via `ring_buffer_tail` for inclusion in `CommandOutcome::stderr_excerpt`. The `stderr_capture_bytes` field only bounded the *retained excerpt*, not the peak memory during read. A misbehaving subprocess emitting many gigabytes to stderr would allocate all of it before truncation. FOLLOWUPS T6 documented this gap; Plan B Epic 2 anticipated an ADR to close it (numbered AD0023 in the Epic 2 sketch, renumbered to AD0021 here because AD0018-AD0020 meta-process ADRs landed on main after the sketch was written).

OPTION:
Streaming reader filling a `VecDeque<u8>` of size `cap`; pop leading bytes when full; add `stdout_capture_bytes: usize` field on `CommandSpec` parallel to existing `stderr_capture_bytes`; change `CommandOutcome::stdout` from `Vec<u8>` to `Option<Vec<u8>>` (None when intentionally discarded, Some(bounded Vec) when capture requested); remove `ring_buffer_tail` helper.

OPTION:
Keep `read_to_end` but bound CommandOutcome's retained bytes via post-hoc tail-slicing using a new field `*_capture_bytes` interpreted as "max retained" rather than "max in-flight." Status quo extended; peak memory remains unbounded.

OPTION:
Replace `CommandOutcome` with an enum splitting AudioFile-write vs Captured-stdout use cases (`enum CommandOutput { AudioFile, Captured(Vec<u8>) }`). Restructures the public surface; tighter type-level expression of intent.

CRITERIA:
Peak memory bounded by construction (load-bearing under N concurrent fetches anticipated in Plan B Epic 2 phase 2); shape distinguishes "captured but empty" from "intentionally discarded" cleanly at the type level; idiomatic Rust matches existing call-site style; does not over-fit Epic 2 T13's design freedom for symmetric stdout policy decisions; keeps `process::run` generic.
BODY
```

Verify the file looks right:
```bash
cat docs/decisions/AD0021-bounded-subprocess-output-capture-via-streaming-vecdeque.md
```

Expected: the file now has filled `## Context and Problem Statement`, `## Considered Options` (3 options), and `## Decision Drivers` sections. Status is still `open`.

- [ ] **Step 4: Decide the ADR (option 1)**

Run:
```bash
adg --model docs/decisions decide --id 0021 --option 1 \
  --rationale "Distinguishes 'captured but empty' from 'intentionally discarded' at the type level (codex-advisor recommendation, idiomatic Rust). Avoids over-fitting Epic 2 T13's design freedom — T13 may add symmetric stdout policy decisions on top without authoring a new ADR. Keeps process::run generic. Option 2 doesn't fix peak memory (only retained); Option 3 is heavier API surface for this worktree's scope."
```

(If the `--rationale` flag isn't supported in this `adg` build, the rationale goes into the file's `## Decision Outcome` section via a separate `adg edit --outcome '...'` call. Check `adg decide --help`.)

Expected: ADR status flips from `open` to `decided`; the `## Decision Outcome` section now references Option 1 with the rationale.

- [ ] **Step 5: Hand-edit the Consequences section**

`adg` doesn't have a flag for the consequences ladder; edit the file directly. Add a `## Consequences` section (or fill an existing empty one). Use the spec text:

```markdown
## <a name="consequences"></a> Consequences

- **Retained-output memory ceiling** is now `stdout_capture_bytes + stderr_capture_bytes` per subprocess (call-site controlled), not "tool exit + truncation." Total peak memory during a `run` call is `retained + O(read_chunk_size_stdout + read_chunk_size_stderr + tokio task overhead)` — the chunk buffers used by the streaming reader hold at most one `read()` worth of bytes each before draining into the `VecDeque<u8>`. "Bounded by construction" applies to the retained buffer, not to instantaneous transient allocations.
- Call sites must explicitly opt in to stdout capture; `yt-dlp` and ffmpeg-postprocessor paths get `stdout_capture_bytes: 0` (no behavioral change — they did not read `outcome.stdout`).
- Plan B Epic 2's T13 inherits this design. T13 may add symmetric stdout policy decisions (different defaults for specific tools) without authoring a new ADR; if it changes the design, it supersedes AD0021 with a new ADR per existing convention.
- Test coverage in `tests/process_bounded_capture.rs` covers overflow tail preservation, stdout opt-in/opt-out, exit-code passthrough, and a direct `read_bounded` peak-memory-bounded assertion via an `Arc<AtomicUsize>` counter.
```

- [ ] **Step 6: Verify ADR consistency**

Run:
```bash
adg --model docs/decisions validate
adg --model docs/decisions list --id 0021
```

Expected:
- `validate` reports clean.
- `list --id 0021` shows status `decided` and the correct title.

If `validate` complains about missing section anchors (the index expects `<a name="…"></a>` anchors on certain headings), fix them inline. The existing AD0020 file is a reference for the anchor names (`question`, `options`, `criteria`, `outcome`, `consequences`, `comments`).

- [ ] **Step 7: Commit**

```bash
git add docs/decisions/AD0021-*.md docs/decisions/index.yaml
git commit -m "$(cat <<'EOF'
docs(adr): add AD0021 bounded subprocess output capture

Records the design decision implemented in the immediately-preceding
commit (T5 perf-tweaks: bounded streaming subprocess capture).

Option 1 (bounded VecDeque<u8> streaming + Option<Vec<u8>> stdout +
remove ring_buffer_tail) chosen. Distinguishes "captured but empty"
from "intentionally discarded" at the type level (codex-advisor
recommendation, idiomatic Rust). Avoids over-fitting Epic 2 T13's
design freedom for symmetric stdout policy decisions.

Numbering: the Epic 2 sketch anticipated this as AD0023; renumbered
to AD0021 because AD0018-AD0020 (meta-process ADRs) landed on main
after the sketch was written. Epic 2's remaining anticipated ADRs
renumber from AD0022 onward.

Refs: AD0001 (feature-derived ADRs ride the feat branch),
FOLLOWUPS L47 + L48 (closed by T5 commit; SHA backfilled in T11)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

The pre-commit hook will re-run `adg validate` and either pass or fail. If it fails, the validate output names the issue; fix and re-commit.

---

## Self-check

- [ ] `docs/decisions/AD0021-*.md` exists with status `decided`.
- [ ] `docs/decisions/index.yaml` includes AD0021 with title and status.
- [ ] `adg --model docs/decisions validate` returns clean.
- [ ] Context, three Options, Criteria, Outcome (with Option 1 + rationale), and Consequences sections are all populated.
- [ ] Pre-commit hook ran on the commit and passed.
