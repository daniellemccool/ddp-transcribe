# uu-tiktok — working disciplines

## Project

Pipeline that ingests TikTok DDP (Data Donation Programme) exports, fetches the donor's watched videos, and transcribes the audio with whisper.cpp. Current state on `main`: Plan B Epic 1 complete (embedded `whisper-rs` + raw confidence signals + CUDA bake on a single A10 dev workspace). Plan B Epic 2 (state machine + pipelined orchestrator) is the next epic; planning artifacts under `docs/superpowers/plans/`.

## Working disciplines (project-wide ADRs)

Project conventions live in `docs/decisions/` as Architectural Decision Records. The meta-process slate that applies to every epic:

- **0001** — per-task file split for plans (subagent context economy)
- **0002** — dead-code suppression strategy + deferred bin/lib reassessment
- **0003** — test discipline + brief-deviation honesty in commits
- **0005** — `test-helpers` Cargo feature for integration-test library items
- **0006** — `Store` mutators return `Result<usize>` (row-change count)
- **0007** — stats structs use input-side counters with verb-named fields
- **0008** — pipeline writes transcript artifacts before `mark_succeeded`
- **0018** — three-tier review with codex-advisor delegated via Sonnet reviewer
- **0019** — subagent report format and phase-boundary controller restart
- **0020** — FOLLOWUPS document structure and lifecycle

Feature-derived ADRs (0004, 0009–0017, plus Epic 2+ feature ADRs) live on feat branches and merge in. Read ADRs as needed via `adg view --id NNNN --model docs/decisions` (or browse `adg list`).

## Default working patterns

- **Executing plans:** `superpowers:subagent-driven-development` (in-session) or `superpowers:executing-plans` (multi-session).
- **Before claiming done:** `superpowers:verification-before-completion`. Run `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`.
- **Parallel work / conflict isolation:** `superpowers:using-git-worktrees`.
- **Code review:** `superpowers:requesting-code-review` / `superpowers:receiving-code-review`; the three-tier protocol per 0018.
- **Debugging:** `superpowers:systematic-debugging`.

## Project-local tools

- **ADRs:** see the `using-adg` skill. Use `scripts/adr` (a thin wrapper around `adg` that hardcodes `--model docs/decisions` and takes bodies via stdin).
- **codex-advisor:** see the `using-codex-advisor` skill. Per 0018, the orchestrator never calls codex directly during task reviews — the Sonnet spec-compliance reviewer delegates and distills.

## Active state — query, don't write down

- Active plan: `ls docs/superpowers/plans/ | sort | tail -1`
- Codex-advisor pinned session UUID: `codex-advisor id`
- Current branch: `git branch --show-current`
- Recent commit log: `git log --oneline -5`

## FOLLOWUPS

Per 0020, `docs/FOLLOWUPS.md` carries active-scope review items grouped by target epic, with a scope index at top. `docs/cosmetic-followups.md` and `docs/bake-findings.md` are off the planning-time reading path. `docs/archive/followups-resolved.md` is the append-only resolved history. At epic close, resolved entries move to archive with the resolving commit SHA.

## Verification before any commit

`adg validate` runs as a pre-commit hook (`.githooks/pre-commit`). If a fresh clone doesn't fire it, run `git config core.hooksPath .githooks` once. If validate fails, fix the underlying `docs/decisions/` inconsistency (don't bypass).
