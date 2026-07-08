# FOLLOWUPS — Epic 4b active entries

Active-scope review items targeted for Plan B Epic 4b (operator-facing
`status` command, time-window/timezone work, cookie-efficacy verdict). Epic
4a (in-pipeline retry, config-driven classification, triage retirement) has
closed; its resolved entries moved to `../archive/followups-resolved.md`.
See `../FOLLOWUPS.md` for the scope index across all epics;
`../cosmetic-followups.md`, `../bake-findings.md`,
`../archive/followups-resolved.md` for sibling categories. The
unverified-hypothesis prefix rule (`**Hypothesis (unverified):**`) applies
here per 0020.

---

### `parse_watched_at` assumes DDP `Date` strings are UTC; TikTok docs are silent

**Found in:** T13 code quality review (opus).
**Disposition:** Real semantic risk; defer until evidence is available about
TikTok's DDP timestamp convention. Re-targeted to Epic 4b (time-window /
timezone work).
**Trigger to revisit:** any task that begins comparing `watch_history.watched_at`
against an externally-meaningful time (Epic 4b's time-window filter, Plan C's
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

---

### `--retries` / `max_attempts` accept unvalidated i64 ranges

**Found in:** Epic 4a T06 review (adjudicated deferral; no ledger entry until now).
**Disposition:** Robustness gap, not a live bug at the default. Fold into Epic
4b's operator-facing CLI pass.
**Trigger to revisit:** Epic 4b `status`/operator-UX work, or any operator
report of surprising retry behavior at extreme `--retries` values.

`process --retries` is a bare `i64` with `default_value_t = 1` and no range
`value_parser`; the sweep/`record_fetch_failure` cap is computed as
`retries + 1`. Two unvalidated edges:

1. **Negative values degenerate the budget.** `--retries -1` yields a cap of
   `0`, so `attempt_count < 0` is never true and every claimed row is
   exhausted/parked on first failure without a retry — silently, with no
   error. More-negative values are equally silent.
2. **`i64::MAX` overflows at `retries + 1`.** `--retries 9223372036854775807`
   panics (debug) or wraps to `i64::MIN` (release) at the `retries + 1`
   computation, degenerating the cap the other way.

Neither bites at the default. A `RangedI64ValueParser`-style bound (e.g.
`0..=1_000_000`) at parse time — mirroring the `download_workers` /
`channel_capacity` `RangedU64ValueParser` pattern already in `src/cli.rs` —
closes both. (The retired `--rate`'s `parse_positive_rate` was the analogous
hand-rolled guard for the triage path.)

---

### Config echo logs `whisper_model_path` for subcommands that never load the model

**Found in:** First production `triage --dry-run` (2026-07-07, 7,087-row DB);
the triage half of this papercut is now moot (triage retired in Epic 4a), but
the config-echo issue itself is untouched and survives.
**Disposition:** Operator-UX papercut; NOT addressed by Epic 4a. Re-targeted
to Epic 4b's operator-facing pass.
**Trigger to revisit:** Epic 4b operator-commands design (natural bundle with
the `status` subcommand and ADR-0017 done-contract work).

The startup config echo (`src/main.rs`, `"config resolved"` line) logs
`whisper_model_path` for every subcommand, including ones that never load the
model (`init`, `ingest`, `migrate`). On the 2026-07-07 run this sent the
operator chasing a "why is it using tiny?" false alarm. Scope the echo to the
config the invoked command actually consumes, or annotate the fields that are
resolved-but-unused for the current subcommand.

---

### Operator interface is the tool itself — wrapper scripts are non-normative (standing premise)

**Found in:** Epic 3 close-out operations session (2026-07-07); operator ruling
recorded as a comment on ADR-0032 the same day.
**Disposition:** Binding design premise (honored by Epic 4a — `--retries` /
`--classification` and the batch census are baked into the tool, not wrapper
scripts). Left standing as the premise for Epic 4b's `status` work.
**Trigger to revisit:** Epic 4b brainstorming/planning start — read this before
sketching any operator command.

The shell scripts generated by the researchcloud-ddp-transcribe component
(`run-pipeline-gpu*.sh`, `sync-to-storage.sh`, `restore-from-storage.sh`) and
any ad-hoc VM scripts are temporary conveniences for particular operational
moments — at most durable SRC-specific integration glue (data movement,
provisioning). They are NOT the operator interface and Epic 4b must not
inherit them as an assumption: operator commands get baked into the tool
(self-contained, easy to use, per the Epic 4 sketch). The 2026-07-07 session
demonstrated the failure mode this guards against: generated wrappers were
mistaken for the canonical entry point on the strength of their headers.
