# v0.4.0 release notes (draft)

Paste the fenced block below into `git tag -a v0.4.0` (release checklist of
`00-overview.md`, ADR-0043 promotion sequence). The tag commit — not this
branch — is where `Cargo.toml` `version` goes 0.3.2 → 0.4.0.

Minor rather than patch: `requeue-failures` is a new operator subcommand and
the ADR-0013 assertion can now *hard-fail* a CUDA build that previously started
happily on CPU. Nothing in the state schema moved.

```
v0.4.0 — Plan B close-out

- NEW `requeue-failures`: the operator escape hatch for failures no
  automatic mechanism can reach — rows blocked by the lifetime attempt
  cap, and rows already written off as failed_terminal — after the
  EXTERNAL condition that failed them changes (fresh cookies, a region
  unblocked, a yt-dlp bump). It grants eligibility; it is not a second
  classifier, and the next fetch is still the liveness oracle.
  Eligibility is DEFAULT-DENY: a bare invocation is a usage error. You
  must pass a qualifying selector — `--error-kind <K>` (repeatable,
  exact byte match, no case folding, no comma splitting),
  `--max-attempts <N>`, `--older-than <DUR>` — or an explicit `--all`.
  `--max <N>` and `--dry-run` are modifiers and never grant eligibility
  on their own; `--all` conflicts with every qualifying selector, so
  `--all --older-than 30d` is a parse error rather than a silent
  intersection; `--include-terminal` requires a qualifying selector
  alongside it, so terminal rows are opt-in twice over. `--older-than`
  reads a failure-events-only clock (failed_retryable, failed_terminal,
  retry_requeued, cookie_parked) — swept_terminal and the other
  administrative events never reset it. Moved rows go to pending with
  claimed_by/claimed_at cleared; attempt_count is NEVER reset and the
  failure fields are retained, so the cap stays auditable, and each row
  gets one `operator_requeued` event carrying its prior status, prior
  kind/reason and attempt count under
  worker_id = operator:<hostname>-<pid>. Arithmetic worth stating
  because it bites: requeueing buys one FORCED attempt, not a retried
  one. For a row at attempt_count = A the next claim bumps it to A + 1
  and in-pipeline retry only fires while attempt_count < retries + 1,
  so an automatic retry needs `--retries > A` strictly — a row
  exhausted at A = 3 under `--retries 2` needs `process --retries 4`.
  Hand-written SQL against `videos` stays unsupported: it mutates
  status without the video_events row every audit depends on, which is
  what this subcommand exists to make unnecessary.
- Per-attempt fetch directories, with a full lifecycle. Each `acquire`
  now creates its own `<transcripts>/.work/ytdlp-<id>.<pid>-<seq>/` —
  never reused across retries, concurrent workers or processes — and
  the downloaded WAV is DISCOVERED by scanning it rather than assumed
  to be `<id>.wav`: exactly one `.wav` is success, zero is
  MissingOutput, more than one is the new AmbiguousOutput, a distinct
  failure, because guessing would stamp an arbitrary file as this
  video's transcript. The whole directory is removed at the end of the
  attempt — after the DB commit on success, or on a decode/transcribe
  failure whose retry re-fetches into a fresh one. Only crash, kill and
  cancellation residue survives, and `process` sweeps it at startup:
  any `.work/ytdlp-*` older than `--stale-claim-threshold` is removed,
  age-gated for the same reason the tmp sweep is — a fresh directory
  may belong to the other GPU instance's in-flight fetch. Operationally
  this means mid-run rsync `file has vanished` warnings for `.work/`
  remain expected (see the runbook's exit-24 note) and the example
  paths now carry the `.<pid>-<seq>` suffix.
- The ADR-0013 GPU backend assertion actually fires. It has been
  accepted-but-unimplemented since Epic 1: a CUDA-built binary that
  silently fell back to CPU looked completely normal, just ~100x
  slower. Engine construction now routes whisper.cpp's log through one
  process-global bridge, captures the init phase across BOTH the
  context constructor and the primary create_state (where the pinned
  whisper.cpp v1.8.3 actually selects the backend), and parses the
  result in ORDER: `using X backend` is a PENDING CLAIM, retracted by a
  following `failed to initialize X backend`, so a GPU that was found
  but failed to start reads as a CPU fallback instead of a success. On
  a `--features cuda` build any non-GPU verdict — including "could not
  tell" — aborts engine construction with BackendMismatch before any
  batch work starts. CPU builds report their backend and assert
  nothing. WHAT THIS MEANS FOR DEPLOYMENT: a CUDA binary on a workspace
  whose GPU is misconfigured now EXITS instead of running the batch
  slowly. That is the intended behavior, and it is the one change here
  that can turn a previously-starting run into a non-starting one.
- Sync-IO policy, and two moves under it. Every blocking call reachable
  from an async fn was inventoried and classified (a: single-task
  subcommands and startup, nothing to starve; b: bounded by
  construction; c: unbounded on the worker hot path). The two class-(c)
  calls moved to spawn_blocking: `decode_wav` (whole-file read + f32
  conversion, run by three concurrent fetch workers) and
  `write_artifacts_durable` (mkdir + two atomic_writes = three fsyncs,
  unbounded on a slow volume). Behavior is preserved exactly — same
  error kinds, same context strings, panics still unwind the caller.
  The policy is a record so a future naked class-(c) call can be
  rejected by citation rather than by taste.
- `backfill-metadata --dry-run --limit N` is now a clap usage error
  (exit 2). Dry-run reports the FULL cohort and invokes nothing, so a
  cap on attempts had nothing to cap; it used to be silently ignored.
  The only operator-visible behavior change in the hygiene bundle.
- Structural: one module root. `src/lib.rs` is now the crate's only
  place modules are declared; `src/main.rs` carries no `mod` line at
  all and does four things — parse, tracing init, dispatch, exit —
  reaching library code through exactly four names. Subcommand dispatch
  moved verbatim into `src/commands.rs` and returns a `CommandExit`
  value instead of calling process::exit inside the library. USER-
  INVISIBLE: the binary's behavior, CLI surface, exit codes and log
  lines are identical. What changed is that every file compiles once
  instead of twice, which removed 84 duplicated inline test executions
  (test census 345 -> 261 runnable at the restructure), narrowed 27
  items from `pub` to `pub(crate)`, and let the `#[allow(dead_code)]`
  census drop 46 -> 1.
- Schema unchanged — still v6, no migration. `video_events.event_type`
  gains the `operator_requeued` value; the column is open TEXT.

Upgrade: supersedes v0.3.2 for all future deployments. The campaign
workspace is still on v0.3.0 and neither v0.3.1 nor v0.3.2 was ever
deployed, so a workspace upgrade is a single 0.3.0 -> 0.4.0 jump and
ALL THREE tags' notes apply (v0.3.1 brought backfill-metadata and
post-subcommand global flags; v0.3.2 the checkpoint hook, the dry
ingest --dry-run, the age-guarded tmp sweep and swept_stale events).
Per docs/operations/src-vm.md (build + cp + -V/-h check; -V must print
0.4.0, `-h` must list requeue-failures); catalog pipeline_git_ref ->
v0.4.0. The SRC catalog item itself is UNCHANGED — no new provisioning
input, no new dependency, no schema migration.
```

**Verification behind these claims** (branch `feat/epic-5b-plan-b-closeout`,
commits `b8fd61f` through this doc commit — 20 commits): `cargo fmt` + `cargo
clippy --all-targets -- -D warnings` + `cargo test --features test-helpers --
--test-threads=1` — **330 passed / 0 failed / 11 ignored** (summed across all
`test result:` lines; `--list` census 341) + `cargo build --release`, all clean.
`adg lean index --root .` validates 39 ADRs, 0 failures (2 pre-existing
leanness warnings on 0040/0041).

**Reading the test count against v0.3.2's 345.** It is not a regression. The
Phase-1 restructure (`6bab68e`) made every file compile exactly once, which
stopped 84 library inline tests from running twice: the `--list` census went
345 → **261 runnable** at that commit with zero test files touched. Every
subsequent count in this epic is built on the 261 base, so v0.3.2's 345 and
this branch's 330 are not comparable numbers. Net of the deduplication the
epic **added 69 runnable tests** (261 → 330) and deliberately swapped one
(Task 09 deleted `output::shard_dir`'s unit test along with the function it was
the only caller of). Ignored went 10 → 11 with the new GPU-gated
`engine_init_passes_the_0013_backend_assertion`.

**CUDA gate — run off-box, evidence and caveat.** This workstation has no CUDA
toolkit, so `cargo build --release --features cuda` cannot run locally. The
operator ran it on the paused SRC workspace at branch tip `2788483`: the build
compiled in **2m 11s** and **6/6** ignored `tests/whisper_engine_init.rs` tests
passed on the GPU, including `engine_init_passes_the_0013_backend_assertion`.
**Caveat, stated rather than papered over:** that evidence is from `2788483`,
not from branch tip. It carries forward because the commits after it touch no
`cfg(cuda)`-gated code — there is none in `src/` at all; the `cuda` feature only
toggles `whisper-rs-sys`'s build, and `EXPECTED_BACKEND` uses `cfg!(...)` (both
arms type-checked by every build) rather than a `#[cfg]` split. Re-run the
gated build once before cutting the tag if you want the evidence at tip rather
than the argument for why it transfers.

**Post-tag doc pass:**

- The v0.3.2 hand-off to the deploy-repo owner is still open and still
  applies: the `sync-to-storage.sh` fixes (`--exclude='.work/'`, keep
  `--exclude='*.tmp-*'`, treat rsync **exit 24** as success) and the
  `yoda-operations.md` pointer note saying the checkpoint hook supersedes the
  manual "Campaign checkpoint ritual". The `.work/` exclusion matters more
  under v0.4.0, not less — attempt directories are created and removed per
  fetch, so a mid-run sync races them by construction.
- First `requeue-failures` use on the VM should be `--dry-run` first, on the
  cap-exhausted cookie cohort, and the operator should check the cohort's
  `attempt_count` (`status --retryable --json`) before choosing the
  `process --retries` value — see the `--retries > A` arithmetic above.
- Confirm the ADR-0013 banner (`whisper_backend_init_gpu: using CUDA0
  backend`) on the first post-upgrade run. Under v0.4.0 its absence is no
  longer something to notice by eye — the binary refuses to start — but the
  banner is the positive confirmation the runbook tells operators to look for.
