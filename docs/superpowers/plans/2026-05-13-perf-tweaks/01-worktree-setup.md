# Task 1 — Create the `feat/perf-tweaks` worktree

**Goal:** Set up an isolated git worktree at `../uu-tiktok-perf-tweaks` on a new branch `feat/perf-tweaks` branching off `main`. No code changes; this task just establishes the workspace the remaining 10 tasks will run inside.

**ADRs touched:** none.

**Files:** none (no commits in this task — git worktree creation is not itself a commit).

---

- [ ] **Step 1: Verify the main checkout is on `main` and clean enough for a worktree**

Run, from the main checkout (`/home/dmm/src/uu-tiktok`):
```bash
git rev-parse --abbrev-ref HEAD
git status --short
```

Expected:
- `HEAD` is `main`.
- `git status` shows only the pre-existing untracked items (`docs/archive/`, `docs/reference/whisper-cpp-deepdive.md`) — no tracked-file modifications. Worktrees share the object database; untracked files in the main checkout do NOT propagate to the new worktree.

If there are unexpected tracked-file modifications, **stop and check with the operator** — they may be in-flight Epic 2 work from the other session.

- [ ] **Step 2: Confirm the branch name is unused**

Run:
```bash
git branch -a | grep perf-tweaks || echo "branch name unused — good"
```

Expected output: `branch name unused — good`. If the branch already exists (locally or on `origin`), **stop and check with the operator** — there may be prior in-flight work to rebase onto, not a fresh start.

- [ ] **Step 3: Create the worktree on a new branch**

Run, still from the main checkout:
```bash
git worktree add -b feat/perf-tweaks ../uu-tiktok-perf-tweaks main
```

Expected:
- Command completes with no errors.
- A new directory `../uu-tiktok-perf-tweaks` exists at the same level as `uu-tiktok`.

- [ ] **Step 4: Verify the worktree is set up correctly**

Run:
```bash
cd ../uu-tiktok-perf-tweaks
git rev-parse --abbrev-ref HEAD
git status
ls Cargo.toml
```

Expected:
- `HEAD` is `feat/perf-tweaks`.
- `git status` shows a clean tree (no untracked files; the main checkout's untracked items did NOT come along).
- `Cargo.toml` exists at the worktree root.

- [ ] **Step 5: Verify the build still works from the worktree**

Run, inside the worktree:
```bash
cargo build --tests
```

Expected: clean build (uses the shared cargo cache via the main checkout's `target/` if your local config shares it; otherwise rebuilds dependencies once, then caches). All of Plan A + Plan B Epic 1's existing code is present; no compile errors.

If the build fails, the cause is almost certainly environmental (compiler toolchain, cmake/whisper.cpp build dependencies). Resolve before proceeding — every subsequent task assumes the worktree builds cleanly.

- [ ] **Step 6: Verify the existing test suite passes from the worktree**

Run, inside the worktree:
```bash
cargo test --features test-helpers
```

Expected: all existing tests pass (Plan B Epic 1 ended with 76+ passing tests). Do NOT run `e2e_real_tools` here — those require a real model and the A10 workspace.

If any test fails, **stop and resolve** — every subsequent task assumes a green baseline. The failure is almost certainly environmental, not a regression.

- [ ] **Step 7: No commit in this task**

There is no commit for Task 1. The worktree's first commit is Task 2's `refactor(transcribe): lazy-allocate lang_state on first opt-in request`.

The spec's "commit 0" naming is descriptive (it's the worktree-creation step, not a code commit).

---

## Self-check

- [ ] `../uu-tiktok-perf-tweaks` exists and is the operator's working directory for the remaining 10 tasks.
- [ ] `git rev-parse --abbrev-ref HEAD` returns `feat/perf-tweaks` from inside the worktree.
- [ ] `cargo test --features test-helpers` runs clean.
- [ ] No `feat/perf-tweaks` branch exists on `origin` yet (first push is `-u origin feat/perf-tweaks` at the end of the worktree's life, via HTTPS per CLAUDE.md).
