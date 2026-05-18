---
name: using-adg
description: Use when creating, amending, deciding, commenting on, or linking Architectural Decision Records (ADRs) in this repo. ADRs are managed with the `adg` CLI; multi-paragraph body content is piped through `scripts/adr-fill`. Skill carries project conventions (branch-of-record, title quality, the comment-anchor gotcha); operational surface is in `adg --help`.
---

# Using `adg` in this repo

ADRs live in `docs/decisions/`. Use `adg` directly for everything — see `adg --help` and `adg <subcommand> --help` for the surface. Pass `--model docs/decisions` on every call (or run `adg set-config` once to persist it).

**One exception:** `adg edit --question/--option/--criteria` doesn't accept multi-paragraph content cleanly. After `adg add`, pipe body sections through `scripts/adr-fill <id>` via heredoc with `QUESTION:` / `OPTION:` (repeatable) / `CRITERIA:` section markers. See `scripts/adr-fill` header for the format.

## Conventions

- **Branch-of-record.** Meta-process ADRs (apply project-wide) land on `main`. Feature-derived ADRs (scoped to a design) land on the active feat branch.
- **Titles must be informative.** `adg list` (titles only) is the primary lookup surface — there is no summary field. A good title encodes the WHAT and (often) the WHY in 10–15 words. Models: AD0006, AD0008.
- **Validate after mutating** with `adg validate`. The pre-commit hook backstops, but earlier is better.

## Gotcha: do not use `adg comment`

`adg comment` rewrites the rendered Comments section, destroying the text of prior comments (which live only in the body, not `index.yaml`). It's a data-loss bug. **Don't use it.** For ADR amendments, record context in the **commit message body** of the commit that triggered the amendment; for significant rethinks, use `adg revise` to create a superseding ADR. A forked adg with comment-text in frontmatter is in progress — this gotcha goes away when that lands.
