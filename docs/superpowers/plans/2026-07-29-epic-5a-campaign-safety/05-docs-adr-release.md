# Task 05: checkpoint ADR, runbook updates, FOLLOWUPS lifecycle, release notes

**Files:**
- Create (via adg ONLY): a new lean ADR for the checkpoint hook (`docs/decisions/00NN-…`)
- Modify: `docs/operations/src-vm.md` (dry-run note, tmp-sweep note, checkpoint section)
- Modify: `docs/FOLLOWUPS.md`, `docs/followups/epic-5.md`, `docs/followups/production-run.md`, `docs/archive/followups-resolved.md` (ADR-0020 lifecycle)
- Create: `docs/superpowers/plans/2026-07-29-epic-5a-campaign-safety/RELEASE-NOTES-v0.3.2.md`

**Interfaces:**
- Consumes: the landed Tasks 01–04 (cite their commit SHAs from `git log --oneline` on this branch); the shipped surfaces: `cleanup_tmp_files(root, older_than)` with `stale_claim_threshold` at the call site; truly-dry `ingest --dry-run` (rollback-based, takes brief write locks); `swept_stale` events + real hostname; `process --checkpoint-cmd/--checkpoint-every` with `checkpoints_run/checkpoints_failed` stats.
- Produces: docs consistent with the code; the v0.3.2 tag-notes draft (fenced block, same convention as RELEASE-NOTES-v0.3.1.md).

**Semantics (binding):**
- The executing subagent MUST load `write-adr:write-lean-adr` before touching `docs/decisions/` and author through `adg lean new --from-stdin`. Run `adg lean index --root .` + `adg lean check` on the new record; fix, never bypass.
- FOLLOWUPS lifecycle per ADR-0020 (bodies move to archive with resolving SHAs; index lines are one-line pointers).

- [ ] **Step 1: New lean ADR — the checkpoint hook contract**

Author via the skill. Substance the record must carry (the skill's format governs; this is content, not wording):
- **Decision:** in-run checkpointing is an operator-supplied hook (`--checkpoint-cmd`, `--checkpoint-every`) run by a supervised periodic task through the bounded subprocess runner; the pipeline never embeds sync logic itself (the deploy repo owns it — 0032 operator-interface premise: the binary is the interface, but sync *content* is deployment-specific).
- **Guidance:** hook failures warn and count (`checkpoints_failed`), never abort the run — review rejects any error propagation from the hook path into worker supervision (0025's first-error shutdown must be unreachable from here); timeout = the interval, making a too-slow hook self-evident; the hook runs bounded (0021) with no arguments and no pipeline state access; `batch_runs.params_json` records the checkpoint config.
- `applies_to`: `src/pipeline/pipelined.rs`, `src/cli.rs` (match how existing ADRs scope; the skill/adg will shape it).

- [ ] **Step 2: Runbook (`docs/operations/src-vm.md`)**

1. **Checkpoint section** beside the existing "Volume sync" material: recommended invocation for uncapped campaign runs —
   `process --checkpoint-cmd ~/sync-to-storage.sh --checkpoint-every 15m` — noting the script's own flock makes overlap with manual runs safe; the first firing is one full interval after start; a failed/timed-out hook warns and counts but never stops the batch (`checkpoints_failed` in the run summary); this replaces the manual "campaign checkpoint ritual" as the default (the ritual doc in the researchcloud repo gets a pointer note — hand that to the deploy-repo owner, do not edit that repo here).
2. **`ingest --dry-run`**: update any text describing it — it is now actually dry (rollback-based): real stats, zero writes, ledger untouched; it takes the same brief per-file write locks as a real ingest (safe under WAL/busy_timeout alongside a live `process`, not lock-free).
3. **Tmp sweep**: note the age guard — startup cleanup only collects tmps older than the stale-claim threshold, so restarting one instance while its sibling runs is safe by construction (previously a narrow abort window).
4. Version-signal touch-ups: anywhere the runbook says v0.3.1-only behavior starts, v0.3.2 supersedes (the VM upgrade jumps 0.3.0 → 0.3.2; `-V` must print 0.3.2 after the upgrade).

- [ ] **Step 3: FOLLOWUPS lifecycle**

1. **Archive** (with Task SHAs): the Epic 5 "startup `cleanup_tmp_files` sweep can delete a concurrent live process's in-flight tmp" entry (Task 01); the Epic 5 "`ingest --dry-run` is not dry" entry (Task 02); the production-run "periodic in-run checkpoint" entry (Task 04).
2. **Annotate, do NOT archive**: the production-run concurrent-writer + `--max-videos` anomaly cluster — add: "Instrumented in v0.3.2 (Task 03 SHA): every sweep-recovered row now writes a `swept_stale` event with the stale claim's provenance, and instances report real hostnames. Next occurrence of a pending-count bump: pull the snapshot, check for matching `swept_stale` events — present = sweep (expected), absent = writer-loss evidence; then this entry graduates to a fix task."
3. Index lines updated accordingly; hypotheses keep their `**Hypothesis (unverified):**` prefixes.

- [ ] **Step 4: Release notes draft**

`RELEASE-NOTES-v0.3.2.md`, same shape as the v0.3.1 file (preamble + fenced tag-message block + verification footnote citing the branch's gate totals). Content: the four operator-facing changes (tmp-sweep age guard — safe instance restarts; truly dry `ingest --dry-run`; `swept_stale` forensics + hostname attribution; checkpoint hook flags) and the note that v0.3.2 supersedes the undeployed v0.3.1 for the VM upgrade (0.3.0 → 0.3.2 jump; both tags' notes apply).

- [ ] **Step 5: Full verification + gates**

`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` (docs-only task — must stay green) and `adg lean index --root . && adg lean check <new record>`.

- [ ] **Step 6: Commit**

```bash
git add docs/
git commit -m "docs: checkpoint-hook ADR; runbook dry-run/tmp-sweep/checkpoint updates; FOLLOWUPS lifecycle; v0.3.2 release notes draft"
```
