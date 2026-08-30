# Cookied mop-up kickoff (2026-08-30 — post-campaign-close)

Paste into a fresh session in `~/src/ddp-transcribe` (main checkout):

---

We're starting the cookied mop-up arc. The fetch campaign drained
2026-08-30 (2,446,943/2,982,461 transcribed; final sync landed; the VM is
paused — the ADR's sole-writer handoff trigger has fired).

**The spec already exists, uncommitted in this checkout — read all three
first; they are authoritative except where the deltas below correct
them:**
- `docs/decisions/0048-run-cookied-mop-up-batches-supervised-on-the-desktop.md`
  (ratified invariant, 2026-08-10 — NOTE: lands under a NEW adg-assigned
  number per the 2026-08-12 collision ruling; re-author via
  `adg lean new --from-stdin --date 2026-08-10`, never hand-renumber)
- `docs/operations/cookied-runs.md` (the runbook: retry arithmetic, jar
  procedure, rehearsal, batch calculus, handoff)
- `docs/superpowers/plans/COOKIED-RUNS-KICKOFF-PROMPT.md` (the original
  prompt, pre-campaign — superseded by this file where they differ)

**Deltas since the drafts froze (2026-08-10) — verify each against the
final DB, then refresh the runbook before any batch:**
1. Gated pool ≈ 53k (was 18.7k), attempt counts now heterogeneous —
   re-derive the `--retries` floor (§2's method) from the FINAL snapshot;
   re-do the batch calculus (~52 supervised hours at drafted pace ≈
   ~100 sessions — operator re-ruling needed on size/cadence).
2. Transport: four WAF generations passed. Current REQUIRED stack
   (VM-proven 2026-08-26): yt-dlp ≥ nightly 2026.08.25 + curl_cffi +
   Deno; impersonation posture REVERSED (see
   `docs/operations/incident-2026-08-25-tiktok-challenge-requires-impersonation.md`
   and the runbook's posture-history bullet — on the PR-35 branch if not
   yet merged). Cookies+impersonation together are UNTESTED — the
   rehearsal (§4) must be rebuilt from a current snapshot and re-run
   under this stack before anything authoritative.
3. Schema is v7 (drafts say v6) — desktop binary must be v0.5.x lineage.
4. **Cut and build v0.5.1 FIRST** (merged 2026-08-19, never tagged):
   raising `--retries` un-parks high-attempt residue including video
   7645028780246895894, which under v0.5.0 kills the run via the
   deadline/Cancelled bug v0.5.1 fixes. Tag per ADR-0043 steps 1–3
   (Cargo.toml bump in the tag commit), build the desktop binary from
   the tag.
5. Terminal-semantics knowledge is newer than the drafts: IpBlockedMessage
   = removed-or-private; 10240 = format-mixed; NoVideoFormats exhaustees
   = live audio-less photo posts. None changes the gated cohort, but §1's
   counts and the do-not-relitigate rationales deserve refreshed numbers
   (`~/projects/crime-and-policing/methodology/` has the evidence notes).

**Environment facts:** desktop CUDA build line is in CLAUDE.md; sandboxed
pipeline invocations must START with the absolute binary path (runbook
§7); `--test-threads=1` for any cargo test; git pushes over HTTPS with gh
credentials (`git -c credential.helper='!gh auth git-credential' push
https://github.com/daniellemccool/ddp-transcribe.git <branch>`).

**Suggested order:** (1) land the docs — re-author the ADR via adg,
refresh the runbook's stale sections, commit, PR; (2) cut v0.5.1 +
desktop build + stack install (nightly/curl_cffi/Deno) + smoke; (3)
verify the research account + jar per §3 (sample URLs:
`~/data/d3i/uu-tiktok/cookied-rehearsal/sample-gated-urls.txt`); (4)
rebuild + run the rehearsal under the new stack (§4, disposable copy
ONLY); (5) operator re-rules the batch calculus; (6) sole-writer handoff
and the first authoritative session (§6). The operator supervises every
cookied batch by definition — this arc is paced over weeks, not a sprint.
