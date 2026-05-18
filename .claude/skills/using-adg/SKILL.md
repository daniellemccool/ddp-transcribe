---
name: using-adg
description: Use when creating, amending, deciding, commenting on, or linking Architectural Decision Records (ADRs) in this repo. ADRs are MADR 4.0 markdown managed by the `adg` CLI; this repo wraps the common operations in `scripts/adr` which hardcodes `--model docs/decisions` and accepts bodies via stdin. Prefer the wrapper for the token saving; drop to bare `adg <cmd>` only when you need a flag the wrapper doesn't expose.
---

# Using `adg` in this repo

ADRs live in `docs/decisions/`. `scripts/adr` is a thin wrapper around `adg` that
hardcodes the model path and takes positional args. Use it as the default.

## Subcommands

```text
adr new <title> [<id>]                       create a proposed ADR; prints the new ID.
                                             Optional <id> for deterministic assignment
                                             (1-9999, fails fast on collision).
adr edit <id> [--force] < body               replace body via stdin
adr decide <id> <option-name-or-index> [<r>] mark as accepted; <option> accepts the
                                             bullet text OR a 1-based index
adr supersede <new> <old> [<reason>]         bidirectional supersession
adr comment <id> <text>                      append a comment
adr tag <id> <tag>...                        add one or more tags
adr link <src> <dst> <tag> [<rev>]           add a custom link, optional reverse
adr list [filter...]                         list ADRs (extra args pass to `adg list`)
adr view <id> [--section ...]                show one ADR
adr validate                                 run adg validate
```

`scripts/adr help` prints the same table.

## Create a new ADR

The body's H1 is optional — if omitted, `adg` prepends `# <title>` from the
decision's stored title before save, so you only restate the title when you
want to rename the ADR. Prefer omitting it.

```bash
ID=$(scripts/adr new "Use VecDeque for bounded stdout capture")

scripts/adr edit "$ID" <<'EOF'
## Context and Problem Statement

What problem are we solving?

## Considered Options

* Streaming reader into a VecDeque
* Unbounded read_to_end with post-hoc tail slicing
* Hard limit via setrlimit

## Decision Outcome

placeholder — `adr decide` fills this in
EOF

# Pass the option as a 1-based index (preferred for any non-trivial text):
scripts/adr decide "$ID" 1 "bounded peak memory in addition to bounded retained bytes"
```

Required sections in the body: `## Context and Problem Statement`,
`## Considered Options`, `## Decision Outcome`. The tool refuses bodies missing
any of these. Optional but common: `## Decision Drivers` and `## Pros and Cons of the Options`.

### Picking an option by index vs. by name

`adr decide <id> <option>` accepts either:

- a 1-based integer index into the `## Considered Options` bullets (`1`, `2`, …), or
- the option's full text, matched case-insensitively after `strings.TrimSpace`.

Matching is **strict equality**, not fuzzy — a missing space or backtick errors
loudly with `option "X" is not in Considered Options` (it does not silently match
a near-miss). For options that contain shell metacharacters, backticks, or quotes,
prefer the index — it sidesteps shell-quoting entirely. Example with a SQL-shaped
option:

```bash
scripts/adr edit "$ID" <<'EOF'
## Context and Problem Statement

The worker leases need a single-writer SELECT-FOR-UPDATE pattern.

## Considered Options

* `WHERE status = 'in_progress' AND claimed_by = ?` with a FOR UPDATE row lock
* Optimistic lease with a `last_claimed_at` timestamp and a retry on conflict

## Decision Outcome

placeholder — `adr decide` fills this in
EOF

# Pick by index — no '\''-escaping, no backtick escapes:
scripts/adr decide "$ID" 1 "single-writer guarantee outweighs the lock-hold cost"
```

### Plan-paper authoring with explicit IDs

When a plan-paper commits to specific ADR IDs in advance (e.g. "Task T1 will land
AD0022, AD0023, AD0024"), use the optional second positional `<id>` so the
executor doesn't have to capture-and-verify the auto-assigned counter:

```bash
ID22=$(scripts/adr new "Schema-version policy for the worker queue" 22)
# ID22 is now exactly 0022, or the call exited non-zero if 0022 was taken.
```

The `<id>` collides loudly if the ADR already exists — there's no auto-skip,
because silently shifting to the next free slot would defeat the point of
asking for a specific ID. `adr new <title>` without `<id>` keeps the
auto-assign-next behavior unchanged.

## Amend or supersede

```bash
# Comments are stored in frontmatter and preserved verbatim (the §A.1 bug is fixed):
scripts/adr comment 0042 "follow-up: see commit abc123 for the implementation"

# Re-edit a proposed ADR; non-proposed requires --force:
scripts/adr edit 0042 < new-body.md
scripts/adr edit 0042 --force < new-body.md   # for accepted/rejected/superseded

# Bidirectional supersede — writes both ends in one transaction:
scripts/adr supersede 0050 0042 "approach evolved during implementation"
```

## Conventions

- **Branch of record.** Meta-process ADRs (apply project-wide) land on `main`. Feature-derived ADRs land on the active feat branch.
- **Titles must be informative.** `scripts/adr list` is the primary lookup surface — there is no summary field. A good title encodes the WHAT and (often) the WHY in 10–15 words.
- **Validate after mutating.** `scripts/adr validate` exits non-zero on issues. The `.githooks/pre-commit` backstops, but earlier is better.
- **Stdout vs stderr.** `adg add` and `adr new` write the new ID to stdout (for `$()`-capture); status text goes to stderr; the global `--quiet` flag suppresses stderr status without affecting machine values or errors.

## File format

MADR 4.0 markdown with YAML frontmatter. The `## Comments` body section is regenerated from frontmatter on every save — hand-edits to that section are lossy. All other sections are authoritative as written.

### `legacy-outcome: true` frontmatter

A small number of pre-migration ADRs carry `legacy-outcome: true`. The validator
uses it to skip the "Decision Outcome contains `Chosen option: \"X\"`" check on
ADRs whose outcome was hand-written before `adg decide` existed. **Don't set it
on new ADRs.** It's cleared automatically the first time `adg decide` runs on
the ADR, because at that point the outcome is freshly tool-generated and the
check applies.
