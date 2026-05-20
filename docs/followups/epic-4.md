# FOLLOWUPS — Epic 4 active entries

Active-scope review items targeted for Plan B Epic 4. See `../FOLLOWUPS.md`
for the scope index across all epics; `../cosmetic-followups.md`,
`../bake-findings.md`, `../archive/followups-resolved.md` for sibling
categories. The unverified-hypothesis prefix rule
(`**Hypothesis (unverified):**`) applies here per 0020.

---

### `parse_watched_at` assumes DDP `Date` strings are UTC; TikTok docs are silent

**Found in:** T13 code quality review (opus).
**Disposition:** Real semantic risk; defer until evidence is available about
TikTok's DDP timestamp convention.
**Trigger to revisit:** any task that begins comparing `watch_history.watched_at`
against an externally-meaningful time (Plan B's time-window filter, Plan C's
status/export commands, or any operator inspecting a single donor's timeline);
also any DDP-docs refresh that adds a timezone annotation to the
"Browsing History" data type.

**Hypothesis (unverified):** If DDP `Date` is actually the user's local wall-clock —
plausible since DDP renders into the user's locale — every `watched_at` is
off by the user's UTC offset (1–2h for NL donors), silently miscategorizing
any time-window filter built on top.

`src/ingest.rs::parse_watched_at` parses TikTok DDP's `Date` field with
`NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")` and then converts via
`Utc.from_utc_datetime(&naive)`, baking a UTC assumption into every
`watch_history.watched_at` i64. The TikTok Data Portability API documentation
in this repo (`docs/reference/tiktok-for-developers/markdown/doc_data-portability-data-types.md`)
lists the Browsing History `Date` field with no timezone annotation. The only
"UTC" mentions in the DDP corpus apply to API request/response timestamps
(`docs/...check-status-of-data-request.md` lines 1955 / 1963), not to data
inside the export.

**Plan A impact:** none. Plan A only persists the i64 and never compares it.

**Plan B impact:** real if a time-window filter or stale-claim recovery uses
`watched_at` as input. Stale-claim recovery uses `claimed_at` (server-side
clock, not affected); the time-window filter is the load-bearing case.

**Plan C impact:** real for status/export. A donor inspecting their own
timeline will see times shifted by their own UTC offset.

**Suggested resolution paths (when this surfaces):**

1. Empirically check a known donation: pick a DDP export from a donor whose
   true watch time is known (e.g., the test fixture's owner) and compare
   parsed UTC against expected wall-clock. If skewed by exactly the donor's
   UTC offset, they're local times.
2. Find authoritative TikTok statement (developer-relations contact, source
   inspection of the DDP renderer, or a fresh docs scrape post-2026-04-16).
3. If local: store the original string alongside the i64 (add column, or
   defer parsing to display time), or add a `respondent_timezone` column
   captured at donation time, or document the i64 as "naive timestamp
   reinterpreted as UTC" and force every consumer to treat the offset as
   unknown.
4. If UTC: add a one-line doc-comment on `parse_watched_at` citing the
   evidence so the next reader doesn't re-litigate.

The verbatim T13 brief made this assumption silently. Recording the gap so
the project can answer it deliberately rather than discover it via a
data-quality bug.
