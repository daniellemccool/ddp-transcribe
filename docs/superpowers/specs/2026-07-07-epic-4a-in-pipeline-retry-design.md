# Epic 4a design — in-pipeline retry, config-driven classification, triage retirement

**Date:** 2026-07-07 · **Status:** approved in brainstorm (operator, same day)
**Supersedes operationally:** ADR-0034's operator-triage flow (formal supersession
lands with the implementation). **Companion:** `docs/superpowers/plans/PLAN-B-EPIC-4-KICKOFF-PROMPT.md`
(operator rulings + census evidence; where the two disagree, this spec is newer).

## Goal

Retry becomes pipeline behavior. A single `process` run sweeps parked failures,
retries retryable ones at the end of its queue under an attempt cap, writes off
proven-dead classes inline, parks cookie-gated rows unless cookies are supplied,
and records a durable, policy-attributed census. The `triage` subcommand and the
oEmbed probe retire. Classification of yt-dlp output becomes an operator-editable
TOML table with an evidence-derived compiled-in default.

Scope split (operator ruling): this is **4a**. Time-window filter, DDP timezone
resolution, and the full `status` subcommand are **4b** (unchanged from
`EPIC-4-SKETCH.md`).

## Evidence base (dry-run census, 2026-07-07, n=7,087)

| kind | examined | dead | alive |
|---|---|---|---|
| IpBlockedMessage (write-off) | 3,241 | 3,241 | — |
| VideoNotAvailable10231 (write-off) | 68 | 68 | — |
| "status code 10240" (was YtDlpOther) | 606 | 606 | 0 |
| NoPermission | 452 | 427 | 25 |
| NoDataBlocks | 2,318 | 7 | 2,311 |
| SensitiveLoginGated | 301 | — (alive per probe) | 301 |
| Ffprobe/HttpError/NetworkTransient/NoVideoFormats | 101 | 0 | 101 |

Zero probe-unreachable (network posture clean). Lessons binding on the default
table: `10240` is a new pure terminal class (single exact message, 100% dead);
`NoPermission` is **impure** (5.5% alive) and MUST stay retryable — per-row
adjudication by re-fetch, not blanket write-off; `NoDataBlocks` is 99.7% alive.

## 1. Classification config

**Loading.** New global `--classification <path>` (env
`DDP_TRANSCRIBE_CLASSIFICATION`). Absent → compiled-in default table stored as a
commented TOML string — one format, one parser (`toml` crate; the one new
dependency of this epic). Parsed and validated at startup **before any claim**;
validation failure is a hard startup error (0022 philosophy). Startup logs source
(`compiled-default` | path) + rule count. The full active TOML text snapshots
into `batch_runs` (§4) — policy provenance for the attrition record is
non-negotiable.

**Schema** (`schema = 1`): ordered `[[rule]]` array, first-match-wins, order
load-bearing; explicit `fallback`:

```toml
schema = 1
fallback = { label = "YtDlpOther", disposition = "retryable" }

[[rule]]
pattern = "Your IP address is blocked"   # exact substring, case-sensitive
label = "IpBlockedMessage"               # tag stored in DB columns
disposition = "terminal"                 # terminal | retryable | requires-cookie
# evidence: probe 10/10 dead 2026-07-06; yt-dlp misfire = VIDEO REMOVED (ADR-0033 comment)
```

**Validation (hard-fail):** unknown `schema`; empty `pattern`; unknown
`disposition`; empty rule list; missing/invalid `fallback` (fallback disposition
must be `retryable` or `terminal` — `requires-cookie` as a blind fallback is
rejected as unsafe).

**Default table** (each rule commented with its evidence):

| pattern | label | disposition |
|---|---|---|
| `Your IP address is blocked` | IpBlockedMessage | terminal |
| `status code 10231` | VideoNotAvailable10231 | terminal |
| `status code 10240` | VideoNotAvailable10240 | terminal *(new; n=606, 100% dead)* |
| `Did not get any data blocks` | NoDataBlocks | retryable |
| `do not have permission to view this post` | NoPermission | retryable *(impure: 25/452 alive)* |
| `not be comfortable for some audiences` | SensitiveLoginGated | requires-cookie |
| `No video formats found` | NoVideoFormats | retryable |
| `unable to obtain file audio codec with ffprobe` | FfprobePostprocess | retryable |
| `HTTP Error` | HttpError | retryable |
| each current NETWORK_MARKER (8 rules) | NetworkTransient | retryable |

**Code boundary.** `classify_message` becomes a table interpreter over the
loaded config. Message-class enums (`UnavailableReason`, message-borne
`RetryableKind` variants) dissolve into config-carried label strings — DB
columns are already TEXT, storage untouched. Structural errors keep their
code-side mapping (`ToolNotFound`/timeout/decode/`SystemIo` etc. — not yt-dlp
opinions). The cookie gate (`cookie_opts_for`) switches from a hardcoded
`kind == "SensitiveLoginGated"` comparison to a table lookup: *is this label's
disposition `requires-cookie`?* Policy lives in exactly one place.

## 2. Retry mechanics

**Failure-time decision** — one new `Store` mutator (working name
`record_fetch_failure`), one transaction, replacing the workers' current
`Retryable`-arm call. Inputs: claim, label, message, disposition, cap, cookies
configured y/n. Behavior:

- `terminal` → as today's `mark_terminal_failure` (inline write-off).
- `retryable` → record label + message, bump `attempt_count` (counts actual
  fetch attempts, lifetime); if `attempt_count < retries + 1` → `status =
  'pending'` (rejoins queue); else → `failed_retryable` (exhausted pool).
- `requires-cookie` → same recording/bump; requeue only if cookies configured
  this run, else → `failed_retryable` (parked; not retried without cookies).
- Bug arm unchanged (escalates per 0025).

Returns which outcome occurred (enum) so worker logs and ADR-0007 stats stay
truthful; row-count discipline per 0006 internally. Both pipelined workers and
`run_serial` dispatch through it.

**End-of-queue ordering.** `claim_next` ordering becomes `attempt_count ASC,
first_seen_at ASC, video_id ASC`: all fresh videos drain before any retry;
retries FIFO behind them. No schema change; `BEGIN IMMEDIATE` concurrency
unchanged. This amends the claim-ordering contract (new ADR).

**Parameters.** `--retries N`, default **1** (so ≤ 2 lifetime attempts by
default). Cap compares against lifetime `attempt_count` — composing with the
parked pilot rows (all at 1): default grants each exactly one automatic retry;
operator raises `--retries` to grant more in later runs. `--max-videos` caps
**total claims including retries** (work-budget accounting).

**Cancellation / crash:** unchanged — requeued rows are plain `pending`;
in-flight claims recover via the 0024 stale sweep.

## 3. Start-of-batch sweep

Every `process` run, after config validation and before workers start: classify
every `failed_retryable` row's stored message through the active table —

- `terminal` disposition → terminalize (event `swept_terminal`; this is where
  the 3,309 write-offs and the 606 `10240` rows die on first run);
- `retryable` + under cap → `pending` (event `requeued`);
- `requires-cookie` → requeued only when cookies are configured this run, else
  left parked (no attempt bump — sweeping isn't fetching); over-cap rows stay put.

Built on Epic 3's Task-05 mutators (`list_failed_retryable` + terminal/requeue
mutators; event kinds updated — exact naming is plan detail). Prints a one-line
summary; counts feed the census. Idempotent by status predicate: a concurrent
second instance's sweep matches zero rows. Historical-DB tolerance carries over:
rows with placeholder kind `"Fetch"` classify from the stored message alone.

## 4. Batch lifecycle + durable census (`batch_runs`, schema v3)

Run sequence: **open** (validate config → GPU banner → insert `batch_runs` row)
→ **sweep** (§3) → **drain** (workers; exit 3 = nothing claimed, unchanged) →
**close** (update row with census + `finished_at`; print the census table).

`batch_runs` (schema v2→3 via the existing 0022 gate + `migrate` extension;
no changes to existing tables):

```sql
CREATE TABLE batch_runs (
  run_id       INTEGER PRIMARY KEY,
  started_at   INTEGER NOT NULL,
  finished_at  INTEGER,            -- NULL = crashed/interrupted (honest)
  params_json  TEXT NOT NULL,      -- retries, max_videos, workers, cookies_present, …
  policy_toml  TEXT NOT NULL,      -- full active classification table
  census_json  TEXT                -- sweep + run sections, counts by label
);
```

Census JSON: `sweep` section (examined / swept_terminal-by-label / requeued /
cookie-parked / kept-capped) + `run` section (claims, succeeded,
terminal-by-label, retried, exhausted, bug-escalations). Stats structs follow
0007 (input-side counters, verb-named).

## 5. Retirements

Deleted (git history is the archive): `triage` subcommand + `src/triage.rs`,
`src/probe.rs`, `--rate`, `tests/triage.rs`, the `curl` runtime dependency.
Docs updated in the close-out task: `docs/operations/src-vm.md` operate section,
architecture doc set (+ the standing `uu-tiktok` naming-sweep FOLLOWUPS item).
`EPIC-3-CLOSE.md` stays untouched as history.

## 6. Testing

- **Config:** parse/validate units incl. every hard-fail path; first-match-wins
  precedence with overlapping patterns (closes the T03 review Minor); compiled
  default re-validated against Epic 3's nine real-corpus fixture files, now
  asserting dispositions (incl. the `10240` rule).
- **Retry:** `pipeline_fakes` integration — new fails-N-then-succeeds fake:
  requeue → end-of-queue → recovery; cap exhaustion → `failed_retryable`;
  `requires-cookie` parked without cookies / requeued+cookied with;
  `--max-videos` accounting with retries in the mix.
- **Sweep:** seeded historical shapes (placeholder kind `"Fetch"`, real corpus
  messages incl. `10240`) → correct split; idempotence (second sweep = zero
  rows); classify-from-message-only tolerance.
- **Ordering:** fresh-before-retry claim-order test. **batch_runs:** row
  open/close, policy snapshot present and parseable, census == stats.
- Discipline: `--test-threads=1` (mandatory, thermal), ADR-0005 feature gates,
  per-file allow headers, 0002/0003 hygiene as always.

## 7. ADR slate (all via `adg`)

1. Supersede **0034** → in-pipeline capped retry; the fetcher is the liveness
   oracle; probe retired (census = closing evidence).
2. New: **classification-config** (TOML schema, first-match, compiled default,
   hard-fail validation, provenance-in-`batch_runs`).
3. New: **retry semantics + claim-ordering amendment** (lifetime attempt cap,
   end-of-queue via `attempt_count ASC`, budget accounting).
4. Comment on **0033**: patterns now live in the config default table; evidence
   semantics unchanged (see the 2026-07-07 misreading-guard comment).

## 8. Deliberately out of scope (4b or later)

Time-window filter + DDP timezone (AD0027-0029 sketch scope); full `status`
subcommand (renders `batch_runs` history when it arrives); cookie acquisition
ops; backoff/jitter sophistication; `run_serial` retirement (Epic 5);
`ibun`/transfer tooling (infra repo).

## Expected first-run outcome (pilot DB)

Sweep: ~3,915 terminalized by message class (3,309 write-offs + 606 `10240`),
**~2,871 requeued** (all retryable-disposition rows — the sweep has no probe and
cannot distinguish dead from alive; that is the drain's job), 301 cookie-parked
(unless cookies supplied), 0 capped. Drain: ~2,871 fetch attempts → ~2,437
expected recoveries (census alive counts: 2,311 NoDataBlocks + 101 misc + 25
NoPermission) and ~434 expected inline write-offs (the 427 + 7 probe-dead rows
re-fail with a write-off message and terminalize — fetch-as-oracle working as
designed). Projected corpus success ≈ 91.5–92%; census lands in `batch_runs`
with the policy that produced it.
