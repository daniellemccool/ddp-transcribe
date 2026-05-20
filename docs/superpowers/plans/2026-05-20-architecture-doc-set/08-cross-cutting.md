# Task 8 — Cross-cutting additions

**Goal:** Make the new architecture doc set discoverable from the project's two top-level "entry-point" documents. One line in `CLAUDE.md` (so any reader — human or AI agent — starting from `CLAUDE.md` finds the architecture doc) and one entry in `docs/FOLLOWUPS.md` (so the next epic close remembers to verify the in-flight subsystems).

**ADRs referenced:** none.

**Files:**
- Modify: `CLAUDE.md` (one line added under "Default working patterns")
- Modify: `docs/FOLLOWUPS.md` (one entry added under the active-epic section)

**Pre-reqs:** T01-T07 complete (the doc set is functionally complete; this task adds the pointers).

---

- [ ] **Step 1: Read the current CLAUDE.md structure to find the right insertion point**

```bash
grep -n "^## " CLAUDE.md
```

Look for a section header named "Default working patterns" or similar. The pointer goes inside that section.

If the section name has drifted from what the spec assumed, use the closest match (e.g., "Working disciplines"). If neither exists, add the line at the end of the "Project-local tools" section.

- [ ] **Step 2: Add the architecture-doc pointer to CLAUDE.md**

Add this line inside the chosen section, formatted as a bullet matching the surrounding style:

```markdown
- **Onboarding / system orientation:** start at `docs/reference/architecture/index.md`.
```

Use `Edit` to insert it. Preserve the existing bullets and section structure exactly. Do not reformat the surrounding bullets.

- [ ] **Step 3: Verify the CLAUDE.md addition**

```bash
grep -n "architecture/index.md" CLAUDE.md
```

Expected: exactly one matching line. If zero or more than one, fix.

- [ ] **Step 4: Read the current FOLLOWUPS.md structure**

```bash
head -80 docs/FOLLOWUPS.md
grep -n "^## \|^### " docs/FOLLOWUPS.md
```

Per [ADR 0020](../../decisions/0020-followups-four-file-split-with-archive-at-epic-close-and-unverified-hypothesis-marking.md), FOLLOWUPS is grouped by target epic. Find the section for the active epic — most likely a heading matching the current branch name or "Plan B Epic 2".

- [ ] **Step 5: Add the architecture-doc-currency entry**

Add an entry under the active epic's section. Format matches existing entries (verify by reading a couple of existing ones first). The content:

```markdown
- **Architecture doc currency.** Before Plan B Epic 2 closes, revise `docs/reference/architecture/state-machine.md` and `docs/reference/architecture/orchestration.md` against the final code state and remove their in-flight stamps. Update `docs/reference/architecture/index.md` §4 (ADR map) for any ADRs added during the epic.
```

If the existing entries use a different format (e.g., a table, a "TN" task numbering scheme, or a metadata footer), adapt to match the local style — read the existing entries to calibrate.

- [ ] **Step 6: Verify the FOLLOWUPS.md addition**

```bash
grep -n "architecture doc currency\|Architecture doc currency\|architecture/state-machine.md" docs/FOLLOWUPS.md
```

Expected: the entry is findable. If the active-epic section also contains a scope index at the top (per ADR 0020), update the index too if it lists entries by name/number.

- [ ] **Step 7: Commit**

```bash
git add CLAUDE.md docs/FOLLOWUPS.md
git commit -m "$(cat <<'EOF'
docs: discoverability + drift-check for the new architecture doc set

- CLAUDE.md gains an "Onboarding / system orientation" pointer to
  docs/reference/architecture/index.md so any reader (human or AI agent)
  starting from CLAUDE.md finds the architecture doc.
- docs/FOLLOWUPS.md gains an architecture-doc-currency entry under the
  active epic so the in-flight stamps on state-machine.md and
  orchestration.md are revised at epic close.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
