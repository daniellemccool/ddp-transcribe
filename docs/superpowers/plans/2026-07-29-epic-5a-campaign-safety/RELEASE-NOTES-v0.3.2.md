# v0.3.2 release notes (draft)

Paste the fenced block below into `git tag -a v0.3.2` (release checklist of
`00-overview.md`, ADR-0043 promotion sequence). The tag commit — not this
branch — is where `Cargo.toml` `version` goes 0.3.1 → 0.3.2.

```
v0.3.2 — campaign-safety slice

- Startup tmp sweep now has an age guard: `cleanup_tmp_files` only
  collects tmps older than the stale-claim threshold
  (`--stale-claim-threshold`, default 30m). Restarting one GPU instance
  while its sibling runs is safe by construction — previously the sweep
  matched on the name alone and could unlink a live sibling's in-flight
  tmp, failing its rename and aborting that entire batch run. A tmp whose
  mtime cannot be read is skipped and warned, never deleted; fresh crash
  orphans survive one startup and are collected on the next.
- `ingest --dry-run` is actually dry. It used to log "not yet
  implemented" and run a real ingest. It now does the full pass — every
  file read, parsed and upserted, ledger rows included — inside one
  transaction spanning the whole inbox scan, then rolls it back. Stats
  are exactly a real run's, cross-file duplicates and raw-date backfills
  included. Cost, stated honestly: a dry-run holds one write transaction
  (BEGIN IMMEDIATE … rollback) for the whole scan where a real ingest
  takes brief per-file locks. A full-inbox dry-run beside a live
  `process` can hold that lock past busy_timeout (5s) and abort the
  live batch's next claim — run dry-runs only at a pause, not alongside
  a live `process`.
- Two-writer forensics: every row the stale-claim sweep recovers now
  writes a `swept_stale` event carrying the stale claim's provenance
  (`was_claimed_by`, `claimed_at`, `threshold_secs`), and instances
  report their real hostname in `worker_id` instead of the literal
  "host". A mid-campaign `pending`-count bump is now adjudicable from a
  DB snapshot: matching events = the sweep did it, none = evidence of
  writer loss. Observability only — the sweep still reverts blind, with
  no validation and no attempt bump (ADR-0024, amended for the events).
- NEW `process --checkpoint-cmd <path> [--checkpoint-every <dur>]`:
  runs the operator's own script (e.g. ~/sync-to-storage.sh) on a
  periodic supervised task through the bounded subprocess runner, so an
  uncapped campaign run no longer leaves the storage volume and resume
  snapshot stale until it exits. Interval doubles as the hook's timeout;
  first firing is one full interval after start; failures warn and count
  (`checkpoints_run` / `checkpoints_failed` in the run summary, census
  and `batch_runs`) but never stop the batch. Config is recorded in
  `batch_runs.params_json`. Default interval 15m; `--checkpoint-every`
  requires `--checkpoint-cmd` (ADR-0044).
- Schema unchanged — still v6, no migration. `video_events.event_type`
  gains the `swept_stale` value; the column is open TEXT.

Upgrade: the workspace is still on v0.3.0 and v0.3.1 was never
deployed, so this is a single 0.3.0 -> 0.3.2 jump and BOTH tags' notes
apply (v0.3.1 brought `backfill-metadata` and post-subcommand global
flags). In-place per docs/operations/src-vm.md (build + cp + -V/-h
check; -V must print 0.3.2, `process -h` must show --checkpoint-cmd);
catalog pipeline_git_ref -> v0.3.2.
```

**Verification behind these claims** (branch `feat/epic-5a-campaign-safety`,
commits `fd54fea`, `130c8a1`, `9e61b99`, `31c18df`, `0cb20f7`, `11a2500`,
plus this doc commit): `cargo fmt` + `cargo clippy --all-targets -- -D
warnings` + `cargo test --features test-helpers -- --test-threads=1` — **344
passed / 0 failed / 10 ignored** (summed across all `test result:` lines;
+14 over v0.3.1's 330, all from this slice's new tests).

**Post-tag doc pass:**

- Hand the deploy-repo owner two items: a pointer note in the researchcloud
  repo's `yoda-operations.md` saying the checkpoint hook supersedes the manual
  "Campaign checkpoint ritual", and the `sync-to-storage.sh` fixes
  (`--exclude='.work/'`, keep `--exclude='*.tmp-*'`, treat rsync **exit 24**
  as success). Until that script fix lands, a non-zero `checkpoints_failed`
  made entirely of exit-24 cycles is noise, not signal — see the runbook's
  checkpoint section.
- First real checkpoint smoke on the VM: `--checkpoint-every 2m
  --checkpoint-cmd ~/sync-to-storage.sh`, confirm one clean firing before
  trusting the 15m default on a long run.
