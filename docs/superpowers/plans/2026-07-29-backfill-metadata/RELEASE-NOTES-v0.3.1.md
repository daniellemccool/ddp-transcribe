# v0.3.1 release notes (draft)

Paste the fenced block below into `git tag -a v0.3.1` (release checklist
step 2 of `00-overview.md`, ADR-0043 promotion sequence). The tag commit —
not this branch — is where `Cargo.toml` `version` goes 0.1.0 → 0.3.1.

```
v0.3.1 — metadata backfill + CLI ergonomics

- NEW `backfill-metadata` subcommand: recovers video_metadata_raw
  envelopes for succeeded videos that predate fetch-time capture (the
  rc1-era cohort, 10,235 videos at the 2026-07-29 snapshot). One
  metadata-only yt-dlp invocation per video — no media, no GPU, never
  touches video status or lifecycle, never carries cookies. Serial by
  design (the loop is the rate limiter) and safe to run alongside a live
  `process`. Best-effort and re-runnable: inserts only if missing, so a
  fetch-path envelope is never overwritten, and re-runs converge on the
  unreachable residue. `--dry-run` prints the full cohort size and exits
  (it ignores `--limit`); `--limit N` caps a smoke run. Stats line is
  `examined / captured / capture-failed / already-filled / insert-failed`.
  Run `load-metadata` afterwards to fill the typed columns — backfilled
  envelopes are schema-identical to fetch-time ones.
- All GlobalArgs flags now accept placement after the subcommand
  (`global = true` on the 10 that lacked it; SRC-bake + T11 followup).
  `ddp-transcribe process --state-db …` no longer fails to parse.
- Cargo package version now tracks release tags (this tag's commit
  bumps 0.1.0 → 0.3.1; `-V` finally means something).
- State layer: backfill cohort queries (count + keyset page) and an
  insert-if-missing `video_metadata_raw` mutator. Schema unchanged —
  still v6, no migration.
- ADR-0042 revised: the backfill carve-out to its single-invocation rule.
  Runbook gains a `backfill-metadata` section.

Upgrade: in-place per docs/operations/src-vm.md (build + cp + -h
check); catalog pipeline_git_ref → v0.3.1.
```

**Verification behind these claims** (branch `feat/backfill-metadata`,
commits `d182c45`, `f310604`, `83443e1`, `f962432`, `7dfa771`, plus this
doc commit): `cargo fmt` + `cargo clippy --all-targets -- -D warnings` +
`cargo test --features test-helpers -- --test-threads=1` — 330 passed / 0
failed / 10 ignored (9 model/network-gated ignored from v0.3.0 plus this
branch's live backfill smoke = 10).

**Post-tag doc pass:** archive the "Cargo package version must track
release tags" FOLLOWUPS entry with the tag commit's SHA (it is annotated
"resolution in flight" until then).
