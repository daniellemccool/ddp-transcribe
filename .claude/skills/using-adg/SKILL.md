---
name: using-adg
description: Use when creating, amending, deciding, commenting on, or linking Architectural Decision Records (ADRs) in this repo. ADRs are MADR 4.0 markdown managed by the `adg` CLI; this repo wraps the common operations in `scripts/adr` which hardcodes `--model docs/decisions` and accepts bodies via stdin. Prefer the wrapper for the token saving; drop to bare `adg <cmd>` only when you need a flag the wrapper doesn't expose.
---

# Using `adg` in this repo

ADRs live in `docs/decisions/`. `scripts/adr` is a thin wrapper around `adg` that
hardcodes the model path and takes positional args. Use it as the default.

## Subcommands

```text
adr new <title>                       create a proposed ADR; prints the new ID
adr edit <id> [--force] < body        replace body via stdin
adr decide <id> <option> [<reason>]   mark as accepted with the chosen option
adr supersede <new> <old> [<reason>]  bidirectional supersession
adr comment <id> <text>               append a comment
adr tag <id> <tag>...                 add one or more tags
adr link <src> <dst> <tag> [<rev>]    add a custom link, optional reverse
adr list [filter...]                  list ADRs (extra args pass to `adg list`)
adr view <id> [--section ...]         show one ADR
adr validate                          run adg validate
```

`scripts/adr help` prints the same table.

## Create a new ADR

```bash
ID=$(scripts/adr new "Use VecDeque for bounded stdout capture")

scripts/adr edit "$ID" <<'EOF'
# Use VecDeque for bounded stdout capture

## Context and Problem Statement

What problem are we solving?

## Considered Options

* Streaming reader into a VecDeque
* Unbounded read_to_end with post-hoc tail slicing
* Hard limit via setrlimit

## Decision Outcome

placeholder — `adr decide` fills this in
EOF

scripts/adr decide "$ID" "Streaming reader into a VecDeque" "bounded peak memory in addition to bounded retained bytes"
```

Required sections in the body: `## Context and Problem Statement`,
`## Considered Options`, `## Decision Outcome`. The tool refuses bodies missing
any of these. Optional but common: `## Decision Drivers` and `## Pros and Cons of the Options`.

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
