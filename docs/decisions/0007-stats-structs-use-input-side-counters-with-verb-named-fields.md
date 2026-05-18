---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-04-17 08:26:50"
      text: '1. (2026-04-17 08:26:50) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Stats structs use input side counters with verb named fields

## Context and Problem Statement

T13's `IngestStats` struct exposed a tension between two different counter semantics. The brief author wrote DB-side counters (`unique_videos_seen` = newly inserted videos, `watch_history_rows_inserted` = newly inserted DB rows) but the brief's idempotence test asserted input-side semantics (`first.unique_videos_seen == second.unique_videos_seen` and the same for processed rows). On a fresh-DB first run both interpretations agree; on a re-run they diverge: DB-side gives `(0, 0, M_dups)` while input-side gives `(N, M, M)`. T14 will introduce `ProcessStats`, T15 may introduce `InitStats`, and Plan B will likely add `TranscribeStats` / `RetryStats` — we need a convention before each new struct re-litigates the choice.

## Considered Options

* Input-side counters: HashSet for uniques; `*_processed` = total observed in input; `*_duplicates` / `*_skipped` = subset where the consequent action was a no-op
* DB-side counters: `*_inserted` = newly inserted rows; `*_ignored` = subset where INSERT OR IGNORE was a no-op
* Both families on the same struct (e.g., `unique_videos_in_input` and `unique_videos_newly_inserted` side by side)
* Caller derives the metric they want from primitive counters (e.g., `rows_observed`, `rows_inserted_into_videos`, `rows_inserted_into_watch_history`, `rows_skipped_short_link`, etc.)

## Decision Drivers

Operator observability — re-running an idempotent operation should log meaningful values (DB-side gives "0 0 0 0" on re-run, which is uninformative). Idempotence assertions in tests need stable values across runs; input-side counts give that for free. Field NAMING must communicate which interpretation the caller is reading; ambiguous names like `_inserted` that could be input-side or DB-side are a real source of bugs (the T13 brief proved this). Cheap memory cost (Plan A scale: 1k–10k unique videos per donation; HashSet of strings is negligible). Composes naturally with `tracing::info!` log emission at the end of a stats-producing operation.



## Decision Outcome
We decided for [Option 1](#option-1) because: Option 1 answers the operator question that actually matters at re-run time: "did I read the same input?" rather than "did the database grow?". Operator can derive new-row-count from rows_processed minus rows_duplicates if needed. Idempotence becomes testable as "same input → same metrics" without contortions. The naming convention is the load-bearing part of this decision: VERBS that describe the input-side action (rows_processed, videos_seen, urls_skipped, files_processed, date_parse_failures) versus reserved verbs for DB-side metrics IF ever added (_inserted, _ignored, _updated, _deleted). The T13 brief proved that ambiguous names like watch_history_rows_inserted that could mean either input-side or DB-side are an active bug source: the brief impl tracked DB-side, the test asserted input-side, and the field name made both interpretations look reasonable. Rejected option 2 (DB-side): re-runs log "0 0 0 0" — useless for the operator. Idempotence asserts become "first equals what?" rather than "first equals second". For a Plan B retry/recovery context where the operator wants to know "what happened in this run regardless of DB outcome" this is the wrong shape. Rejected option 3 (both families): doubles the field count on every stats struct; readers must remember which family they want. The cost-benefit only pencils out if BOTH sides are routinely needed — they are not for Plan A or visible Plan B work. Add DB-side counters as a separate Store query API (e.g., Store::row_count_in_videos) when needed. Rejected option 4 (primitive counters with caller derivation): pushes interpretation work to every caller; main.rs would need to compute "duplicates = rows_observed - rows_inserted_videos" inline at each log emission site. Defeats the readability gain of named stats fields. Trigger: T13 surfaced the test-vs-impl mismatch and forced a rename (watch_history_rows_inserted → watch_history_rows_processed). Recording the convention so T14 ProcessStats, T15 InitStats if introduced, and any Plan B stats struct use the same shape and naming verbs from day one. Consequences: positive - re-runs produce stable non-zero metrics that match operator intuition; idempotence testable for free; field names communicate semantics without comments. Negative - callers wanting "did the DB grow this run" must combine rows_processed and rows_duplicates to derive new-row count; if they need that frequently a separate Store query is warranted. Negative - HashSet for unique counters adds a small per-run memory cost (Plan A scale: negligible at 1k-10k unique videos per donation; Plan B at 1M+ may revisit but unique-video-id strings are 19 chars so even 10M = ~190MB which is acceptable). Negative - new stats structs MUST follow the verb-naming convention or future readers will be confused; reviewers should flag any new field with _inserted that means input-side. Trade-off: chose operator UX and naming clarity over the slight extra work of derivation when callers do want DB-side metrics.

## Wording Clarification (post-T13 review)

**`_processed` and `*_skipped` / `*_failures` counters are PARALLEL, not nested.**
A row that is short-link-skipped, invalid-URL-skipped, or date-parse-failed is
NOT counted in `*_rows_processed`. The relationship for any input row is:
exactly one of `*_rows_processed`, `*_skipped` (any reason), or `*_failures`
counters increments. Operators reconstruct the total observed input as the
sum of those parallel counters. This was implicit in T13's `IngestStats`
implementation (the `_processed` increment lives AFTER the early-return
branches for skipped and failed rows) and the verbs listed as examples in
the Decision Outcome above already imply parallel counters; recorded
explicitly here so future stats-struct authors do not interpret `_processed`
as "all observed including skipped." Surfaced by the T13 opus code review.

## Comments

* **2026-04-17 08:26:50 — @Danielle McCool:** 1. (2026-04-17 08:26:50) Danielle McCool: marked decision as decided
