# v0.5.1 kickoff prompt (paste into a fresh session)

Execute the v0.5.1 deadline-attribution patch plan using
superpowers:subagent-driven-development.

**Plan:** `docs/superpowers/plans/2026-08-18-v0.5.1-deadline-attribution-patch/00-overview.md`
— read it first; its Ground Truth section was verified against code
2026-08-18 and is current. Single phase, tasks 01–05, three-tier reviews
per ADR-0018.

**Project in one paragraph:** ddp-transcribe fetches donated TikTok
watch-history videos (yt-dlp) and transcribes them (whisper.cpp) for the
crime-and-policing data-donation study. v0.5.0 (claim recency order /
schema v7, claim-time canonical fetch-URL derivation per ADR-0049, the
mass-failure circuit breaker per ADR-0050, transport observability) was
deployed to the campaign VM 2026-08-13 by in-place tag checkout; the
~1.9M-video census is RUNNING on that VM right now in large capped batches.
This plan is workstation-side only — nothing touches the VM until the
operator promotes the v0.5.1 tag (in-place checkout again; no schema
change).

**Why this patch exists (evidence, 2026-08-17):** one video hit the 600 s
transcription deadline; the engine returned `TranscribeError::Cancelled`
(the variant also used for real shutdown), the transcribe worker exited
"cooperatively", the fetch workers died on the closed channel, the run
terminated with an unclosed `batch_runs` row (census lost), and the
stale-claim sweep had to recover 7 stranded rows. The offending video now
sits in the attempt-2 claim tier, which drains at the census tail (~2
weeks) — where it would kill runs again. Hence: fix the variant
attribution (Task 01), prove the kill path is closed (Task 02), stop
losing censuses on run errors (Task 03), cap yt-dlp's internal retries
(Task 04, a filed followup), close out (Task 05).

**Working state:**
- Branch `v0.5.1-patch` exists, based on main @ 0348ad9, with the plan
  committed — it is checked out in the worktree
  `.claude/worktrees/v050-census` (the worktree's name is historical;
  the branch is what matters). Work there; do not create a new worktree.
- The SDD workspace for the previous plan was deleted; start a fresh
  ledger via the skill's `sdd-workspace` script.
- codex-advisor: the pinned session exists (`codex-advisor id`); do NOT
  re-init. Send one short orientation message describing this plan's scope
  before the first review dispatch.

**Environment facts that cost time to rediscover:**
- `cargo test` MUST run `--test-threads=1` (workstation thermal limit —
  never drop it). Full gate: `cargo fmt && cargo clippy --all-targets -- -D
  warnings && cargo test --features test-helpers -- --test-threads=1 &&
  cargo build --release`. Desktop CUDA build (Task 05):
  `PATH=/opt/cuda/bin:$PATH CUDAHOSTCXX=/usr/bin/g++-15 CUDAARCHS=75 cargo
  build --release --features cuda`.
- Git pushes: SSH is broken in the sandbox (`~/.ssh` is deny-listed) —
  push over HTTPS with gh credentials:
  `git -c credential.helper= -c 'credential.helper=!gh auth git-credential'
  push https://github.com/daniellemccool/ddp-transcribe.git <branch>`.
  The shared `.git/config` is write-masked by the sandbox: `push -u` and
  tracking-branch setup fail harmlessly — use `git switch --no-track` and
  plain pushes; ignore `config.worktree` permission warnings.
- The engine deadline test is model-gated: it needs
  `./models/ggml-tiny.en.bin` (Task 01 has the fetch command) and runs with
  `-- --ignored`.
- ADR governance hooks fire on docs/decisions and plan edits — obey the
  injected briefs; `adg lean index --root .` must stay clean; never bypass
  the pre-commit hook.
- No `Cargo.toml` version bump on the branch (0.5.0 → 0.5.1 happens in the
  post-merge tag commit, ADR-0043).

**After the tasks:** final whole-branch review on the most capable model
(per subagent-driven-development), one fix wave max, then
superpowers:finishing-a-development-branch. On "push + PR": the PR body
must describe THIS PR's changes (bug, fix, evidence, verification) — the
release-notes draft from Task 05's report is the seed; the operator merges
and then cuts the tag (ask before pushing a tag commit to main). The
campaign keeps running on v0.5.0 throughout; promotion timing is the
operator's call.
