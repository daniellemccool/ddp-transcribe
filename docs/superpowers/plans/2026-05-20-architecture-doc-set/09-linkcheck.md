# Task 9 — Linkcheck pass

**Goal:** Verify every cross-reference in the architecture doc set resolves — ADR links, `src/` file citations, inter-file links between deepdives and the index, and the additions made in T08. Fix any drift inline; if fixes are needed, commit them. If everything resolves cleanly, this task ends with a no-op commit or an explicit "no fixes needed" note.

**ADRs referenced:** none.

**Files:**
- Read: all of `docs/reference/architecture/*.md`, `CLAUDE.md`, `docs/FOLLOWUPS.md`
- Modify (if drift found): any of the above

**Pre-reqs:** T01-T08 complete. The doc set is functionally final; this is the verification pass.

---

## What gets checked

Three categories of link:

1. **ADR references** — `../../decisions/NNNN-...md` from any architecture doc file.
2. **`src/` citations** — `` `src/path/file.rs[:N]` `` from any architecture doc file.
3. **Inter-file links** — `[name.md](name.md)` between deepdives and the index.

Plus a presence check on the T08 additions in `CLAUDE.md` and `docs/FOLLOWUPS.md`.

Line numbers in citations are *not* checked (they drift; the discipline is "file path stays valid, line number is best-effort"). Only file existence is checked.

---

- [ ] **Step 1: ADR-reference linkcheck across all architecture files**

```bash
grep -oPh '\(\.\./\.\./decisions/\K[^)]+' docs/reference/architecture/*.md | sort -u > /tmp/adr-refs.txt
cat /tmp/adr-refs.txt
while read f; do
  test -f "docs/decisions/$f" && echo "OK ADR: $f" || echo "MISSING ADR: $f"
done < /tmp/adr-refs.txt
```

Expected: every line starts with `OK`. For any `MISSING`, the link's filename has drifted from the actual ADR's filename. Fix by:
- Run `ls docs/decisions/` and find the actual filename for that ADR number.
- `Edit` the architecture doc file to use the correct filename. Use `grep -l` to find which file has the broken link.

- [ ] **Step 2: `src/` citation file-existence check**

```bash
grep -oPh '`src/\K[^`:]+' docs/reference/architecture/*.md | sort -u > /tmp/src-cites.txt
cat /tmp/src-cites.txt
while read f; do
  test -e "src/$f" && echo "OK src: $f" || echo "MISSING src: $f"
done < /tmp/src-cites.txt
```

Expected: every line `OK`. The `-e` test passes for both files and directories — `src/fetcher/` (no extension) and `src/state/store.rs` both succeed.

For any `MISSING`: the cited file doesn't exist. Options:
- The cite has a typo — fix with `Edit`.
- The file was renamed since the deepdive was written — update the cite to the current name.
- The file was deleted — investigate; this may indicate the architecture doc is referring to since-removed code, in which case the deepdive section needs revision.

- [ ] **Step 3: Inter-file link check (architecture-internal)**

```bash
grep -oPh '\]\(\K(index|data-input|state-machine|transcription|orchestration)\.md' docs/reference/architecture/*.md | sort -u
```

Expected output:
```
data-input.md
index.md
orchestration.md
state-machine.md
transcription.md
```

(or a subset — the index doesn't link to itself, and not every deepdive links to every other deepdive). Verify each appearing filename exists in `docs/reference/architecture/`:

```bash
for f in data-input.md index.md orchestration.md state-machine.md transcription.md; do
  test -f "docs/reference/architecture/$f" && echo "OK arch: $f" || echo "MISSING arch: $f"
done
```

Expected: every line `OK`.

- [ ] **Step 4: External-link check (`whisper-cpp-deepdive.md` references)**

```bash
grep -l "whisper-cpp-deepdive" docs/reference/architecture/*.md
test -f docs/reference/whisper-cpp-deepdive.md && echo "OK deepdive present" || echo "MISSING deepdive"
```

Expected: the deepdive is present. `transcription.md` should reference it; verify the link form is `../whisper-cpp-deepdive.md` (one `..` because we're inside `architecture/`, not two).

```bash
grep -oP '\.\./whisper-cpp-deepdive\.md' docs/reference/architecture/transcription.md
```

Expected: at least one match.

- [ ] **Step 5: Verify T08 additions are still in place**

```bash
grep -n "architecture/index.md" CLAUDE.md
grep -n "Architecture doc currency\|architecture/state-machine.md" docs/FOLLOWUPS.md
```

Expected: both greps return at least one match. If either is empty, T08 was reverted somehow — re-apply per T08's steps.

- [ ] **Step 6: Verify no `(TBD)` markers and no `<sha>` placeholders remain anywhere**

```bash
grep -rn "(TBD\|<sha>" docs/reference/architecture/
```

Expected: no output (no matches). Any match means a previous task left a placeholder; go back and fix.

- [ ] **Step 7: Verify in-flight stamps name a real short SHA**

```bash
grep -h "As of commit" docs/reference/architecture/state-machine.md docs/reference/architecture/orchestration.md
```

Expected: two lines, each containing a backtick-wrapped 7-character hex hash (e.g., `` `8554c42` ``). If either still contains `<sha>` or is missing, return to T05/T06 respectively.

- [ ] **Step 8: Final line-count audit**

```bash
wc -l docs/reference/architecture/*.md
```

Expected ranges (spec budget plus tolerance):

| File | Spec budget | Acceptable |
|------|-------------|------------|
| index.md | ~230 | 230-300 (includes T07 walk-through fill) |
| data-input.md | ~210 | 180-260 |
| state-machine.md | ~190 | 160-230 |
| transcription.md | ~220 | 190-260 |
| orchestration.md | ~190 | 160-230 |
| **Total** | **~1040** | **920-1280** |

Significant overage on any one file (>30% over budget) suggests the file drifted into ADR-rationale-restatement territory or external-tool-tutorial territory — review and trim. Under-budget files are fine as long as all the required sections are populated and substantive.

- [ ] **Step 9: Commit (only if fixes were applied during the pass)**

```bash
git status
```

If `git status` shows uncommitted changes (drift fixes from steps 1-5):

```bash
git add docs/reference/architecture/ CLAUDE.md docs/FOLLOWUPS.md
git commit -m "$(cat <<'EOF'
docs(reference): linkcheck pass — fix drifted ADR/src refs

[Describe what was fixed; e.g., "ADR 0024 filename had drifted from
0024-stale-claim-sweep-..." or "src/fetcher.rs renamed to src/fetcher/mod.rs"]

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

If `git status` shows nothing changed (linkcheck passed cleanly):

```bash
echo "Linkcheck passed cleanly — no fixes needed. Doc set is complete."
```

Skip the commit. Note the clean pass in your task report to the orchestrator.

- [ ] **Step 10: Final inventory**

```bash
ls -la docs/reference/architecture/
git log --oneline -10
```

Expected: 5 files in `docs/reference/architecture/` (index + 4 deepdives), and the most recent 9 commits should include `docs(reference): architecture skeleton`, plus one populate-commit per file (5 of them), plus the walk-through fill-in commit, plus the cross-cutting commit, plus optionally this linkcheck commit. Total 8-9 architecture-doc commits since T01.
