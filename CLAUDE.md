# ddp-transcribe — working disciplines

## Project

Video-transcription pipeline for data-donation studies (formerly `uu-tiktok`; historical docs/ADRs use the old name and `UU_TIKTOK_*` env prefix). Ingests TikTok DDP (Data Donation Programme) exports — TikTok is the currently supported source — fetches the donor's watched videos, and transcribes the audio with whisper.cpp. Current state on `main`: Plan B complete (Epics 1–5b), released as v0.4.0; the live campaign workspace stays pinned on v0.3.0 (ADR-0043 promotion model). Next arc: production ops (`docs/superpowers/plans/PRODUCTION-OPS-KICKOFF-PROMPT.md`); remaining work is trigger-gated in `docs/FOLLOWUPS.md` (production-run / Plan C / cross-epic groups).

Deployment/ops live in a sibling repo: `~/src/d3i/d3i-infra/researchcloud-ddp-transcribe` owns the SRC catalog item (`pipeline_git_ref`), sync/yoda scripts, and provisioning; campaign-VM operational work usually spans both repos. Pipeline-side runbook: `docs/operations/src-vm.md`.

## Working disciplines (project-wide ADRs)

Project conventions live in `docs/decisions/` as lean ADRs (Decision/Guidance/Why with routing frontmatter), managed by `adg lean` and the write-adr plugin skills. The pre-migration MADR corpus is frozen in `docs/madr-archive/` — full Context/Considered Options prose for any record whose lean `## Why` needs its backstory. The meta-process slate that applies to every epic:

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

Feature-derived ADRs (0004, 0009–0017, plus Epic 2+ feature ADRs) live on feat branches and merge in. Pull the rules governing files you'll touch via `adg lean brief --model docs/decisions <paths>`; browse the generated `docs/decisions/README.md` index or open records directly.

## Default working patterns

- **Executing plans:** `superpowers:subagent-driven-development` (in-session) or `superpowers:executing-plans` (multi-session).
- **Before claiming done:** `superpowers:verification-before-completion`. Run `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1 && cargo build --release` (the release build gates thin-LTO per ADR-0045). The `--features test-helpers` flag exposes library items needed by integration tests (per 0005). **`--test-threads=1` is mandatory on the operator's dev workstation — multi-threaded `cargo test` overheats the machine.** Secondary benefits: deterministic ordering for state-machine tests with per-test fixture DBs, and avoiding GPU-contention noise across the whisper-engine integration tests. Do not drop `--test-threads=1` for "faster" runs.
- **Parallel work / conflict isolation:** `superpowers:using-git-worktrees`. `EnterWorktree` auto-names the branch `worktree-<name>`; when a plan mandates an exact branch name, rename with `git branch -m` immediately after creation.
- **CUDA is per-machine:** development happens on two machines. The **laptop** has no CUDA — `cargo build --release --features cuda` and GPU smokes CANNOT run there. The **desktop** (Arch, RTX 2080) builds and smokes GPU work locally: `PATH=/opt/cuda/bin:$PATH CUDAHOSTCXX=/usr/bin/g++-15 CUDAARCHS=75 cargo build --release --features cuda` (CUDA 13.3 rejects the system gcc 16; 75 = the 2080's compute capability). Verified 2026-08-02: the `whisper_backend_init_gpu: using CUDA0` / `backend="GPU" device="CUDA0"` banner (0013) is the proof CUDA engaged — its absence means CPU fallback and the engine aborts by design. Production runs stay on the SRC workspace (see `docs/operations/src-vm.md`). README "GPU (CUDA) build" section carries the full prerequisite list.
- **Code review:** `superpowers:requesting-code-review` / `superpowers:receiving-code-review`; the three-tier protocol per 0018.
- **Debugging:** `superpowers:systematic-debugging`.
- **Onboarding / system orientation:** start at `docs/reference/architecture/index.md` — the architecture doc set (index + four lifecycle-stage deepdives).

## Project-local tools

- **ADRs:** author/migrate/review records with the write-adr plugin's `write-lean-adr` skill (`adg lean new --from-stdin`, never by hand); obey injected briefs per `follow-adr-governance`.
- **codex-advisor:** see the `using-codex-advisor` skill. Per 0018, the orchestrator never calls codex directly during task reviews — the Sonnet spec-compliance reviewer delegates and distills.

## Active state — query, don't write down

- Active plan: `ls docs/superpowers/plans/ | sort | tail -1`
- Codex-advisor pinned session UUID: `codex-advisor id`
- Current branch: `git branch --show-current`
- Recent commit log: `git log --oneline -5`

## FOLLOWUPS

Per 0020, `docs/FOLLOWUPS.md` carries active-scope review items grouped by target epic, with a scope index at top. `docs/cosmetic-followups.md` and `docs/bake-findings.md` are off the planning-time reading path. `docs/archive/followups-resolved.md` is the append-only resolved history. At epic close, resolved entries move to archive with the resolving commit SHA.

## Verification before any commit

`adg lean index --root .` plus `adg lean check` on staged files run as a pre-commit hook (`.githooks/pre-commit`). If a fresh clone doesn't fire it, run `git config core.hooksPath .githooks` once. If the gate fails, fix the underlying `docs/decisions/` inconsistency (don't bypass).
