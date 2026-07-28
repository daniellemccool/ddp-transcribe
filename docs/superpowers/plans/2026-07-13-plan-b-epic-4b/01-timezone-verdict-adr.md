# Task 01: DDP timestamp timezone verdict — evidence dossier → lean ADR

**Files:**
- Modify: `src/ingest.rs` (the `parse_watched_at` `FORMATS` comments + a doc comment recording the verdict)
- Create: one lean ADR in `docs/decisions/` via the `write-adr:write-lean-adr` skill (`adg lean new --from-stdin` — number assigned by adg, expected 0039; NEVER hand-create the file)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: the ADR number + verdict string ("UTC-confirmed" / "local-time" / "unresolved for unlabeled exports") that Task 05's window ADR references and Task 08's FOLLOWUPS resolution records. Report both in the task report.

**This task has an OPERATOR CHECKPOINT (Step 3).** The orchestrator relays the question to the operator via AskUserQuestion; the implementing subagent (if dispatched) must return to the orchestrator for it rather than guessing.

## Evidence dossier (pre-gathered 2026-07-13 during planning — verify, don't re-derive)

1. **The PI's own May-2026 export labels UTC.** Commit `2d89860` (2026-05-21, "fix(ingest): accept ' UTC' suffix in DDP dates") records that the PI DDP bake's 65,024 entries all used `"2026-02-17 21:28:52 UTC"` — a literal `" UTC"` suffix. Verify: `git show 2d89860 --stat --format=full`.
2. **Fresh real-donor exports (July 2026) carry NO suffix.** The two pilot donations in `/home/dmm/data/d3i/uu_tiktok/research-tiktok-crime-policing/pilot-donations/20260707_uu_sociology_facebook_study.zip`:
   - `participant=69f39df5…-tiktok.json` (9.6 MB): 90,466 watch rows, **all** `%Y-%m-%d %H:%M:%S` (no suffix), range 2026-01-30 → 2026-07-05.
   - `participant=6942a793…-tiktok.json` (1.9 MB): 3,823 watch rows, all no-suffix, range 2026-04-20 → 2026-06-18.
   So the no-suffix format is a **live production format**, not synthetic-fixture-only as `parse_watched_at`'s comment claims, and the `" UTC"` documentary evidence does NOT cover these exports. Extraction note: `unzip` false-positives a zip-bomb on other entries (exit 12 AFTER inflating both targets) and the files land mode 0000 — `chmod u+r` after extracting. Extract to the session scratchpad, never into the repo.
3. **Donation-time bounds are consistent with either reading (inconclusive alone).** Platform receipt (from the filename `key=` epoch-millis) vs newest watch entry read as-UTC: donor 69f39df5 received 2026-07-05 18:27:55Z, newest entry 11:53:55 (gap 6.5 h); donor 6942a793 received 2026-07-06, newest entry 18 days earlier. No contradiction either way.
4. **Circadian check has only ~2 h discrimination for NL donors.** Hour-of-day histogram of donor 69f39df5's 90,466 rows (hours 00–23 as written): `[8901, 9091, 5812, 6167, 4280, 1620, 560, 103, 224, 295, 1140, 1686, 2347, 3448, 2473, 2031, 2360, 3618, 5543, 6214, 4218, 4581, 5129, 8625]` — deep trough 07–09, peak 23–01. Coherent as an extreme night owl under either UTC or CEST reading. Supporting evidence at best.
5. **TikTok's docs are silent.** The scraped Data Portability corpus (`docs/reference/tiktok-for-developers/markdown/doc_data-portability-data-types.md`) gives Browsing History `Date` no timezone annotation; the only "UTC" mentions apply to API request/response timestamps. (From the FOLLOWUPS entry `docs/followups/epic-4.md` § `parse_watched_at`.)

## Verdict framework

- The May export's explicit `" UTC"` suffix is TikTok's own pipeline labeling its convention. The most economical reading: the backend convention is UTC and the July renderer **dropped the label**, not the convention. The operator spot-check (Step 3) tests the labeled pipeline directly against known ground truth.
- If the spot-check confirms UTC → verdict **UTC-confirmed** (documentary + empirical for the labeled format; documentary-by-continuity for the unlabeled format). No code change beyond comments; `watched_at_raw` (Task 05) remains the hedge for the unlabeled exports.
- If the spot-check shows a local-time skew → verdict **local-time**; the ADR must then direct every consumer (window filter, future export) to treat stored i64s as offset-unknown, and Task 05's window ADR documents the day-granularity mitigation. No re-ingest is forced — `watched_at_raw` preserves reinterpretability.
- If the operator has no usable ground-truth memory → verdict **UTC-assumed (documentary), unresolved empirically**; record exactly that. Per the FOLLOWUPS discipline: record the verdict that the evidence supports, don't overclaim.

- [ ] **Step 1: Re-verify evidence items 1–2**

Run:

```bash
git show 2d89860 --format=full --stat | head -30
```

Expected: the commit message describing the `' UTC'` suffix discovery on the PI bake.

```bash
SCRATCH=<session scratchpad>/tz-check && mkdir -p "$SCRATCH" && \
unzip -o -j "/home/dmm/data/d3i/uu_tiktok/research-tiktok-crime-policing/pilot-donations/20260707_uu_sociology_facebook_study.zip" \
  "*participant=69f39df5*source=tiktok*" "*participant=6942a793*source=tiktok*" -d "$SCRATCH" || true
chmod u+r "$SCRATCH"/*.json
python3 - "$SCRATCH" <<'EOF'
import json, sys, glob, collections
for f in glob.glob(sys.argv[1] + '/*.json'):
    data = json.load(open(f))
    for section in data:
        rows = section.get('tiktok_watch_history')
        if rows:
            fmt = collections.Counter(
                'UTC-suffix' if e.get('Date','').endswith(' UTC') else 'no-suffix' for e in rows)
            print(f.split('participant=')[1][:8], len(rows), dict(fmt))
EOF
```

Expected: `69f39df5 90466 {'no-suffix': 90466}` and `6942a793 3823 {'no-suffix': 3823}`.

- [ ] **Step 2: Pull the operator's own export's parsed values for the spot-check**

The production snapshot's `preview` respondent is the PI's own May export (the `" UTC"`-labeled pipeline). Pick 3 recent, distinctive watch moments:

```bash
sqlite3 ddp-run-export.sqlite \
  "SELECT datetime(watched_at,'unixepoch') || ' UTC', video_id
   FROM watch_history WHERE respondent_id='preview'
   ORDER BY watched_at DESC LIMIT 5;
   SELECT 'late-night rows:';
   SELECT datetime(watched_at,'unixepoch') || ' UTC', video_id
   FROM watch_history WHERE respondent_id='preview'
     AND CAST(strftime('%H', watched_at,'unixepoch') AS INTEGER) IN (1,2,3)
   ORDER BY watched_at DESC LIMIT 5;"
```

- [ ] **Step 3: OPERATOR CHECKPOINT — empirical spot-check**

Present the Step 2 rows to the operator (AskUserQuestion) with the question: *"These are your most recent parsed watch times displayed as UTC. Do they match your memory of when you actually watched (NL local = UTC+2 in summer, +1 in winter)? E.g., a row shown as 20:30 UTC should correspond to ~22:30 your clock in May. Also: do the 01:00–03:00-UTC rows correspond to ~03:00–05:00 local (UTC-consistent) or 01:00–03:00 local (local-time-consistent)?"* Record the answer verbatim in the task report.

- [ ] **Step 4: Author the ADR via the write-lean-adr skill**

Invoke `write-adr:write-lean-adr`. Content to convey (the skill owns final format; verdict wording adjusted per Step 3's outcome):

- **Title:** "DDP watch-history timestamps are treated as UTC" (or the local-time variant).
- **Decision:** `parse_watched_at` interprets DDP `Date` strings as UTC (`Utc.from_utc_datetime`); verdict grounded in: (a) TikTok's own May-2026 export rendering an explicit `" UTC"` suffix (commit `2d89860`, PI bake, 65,024 entries), (b) the operator's empirical spot-check of known watch moments (result from Step 3), (c) July-2026 real-donor exports dropping the suffix while — by pipeline continuity — keeping the convention, (d) docs-corpus silence. `watched_at_raw` (schema v4, Epic 4b) preserves the original string so any future reinterpretation never requires re-ingest.
- **applies_to:** `src/ingest.rs`.
- **Guidance:** consumers may compare `watched_at` against UTC instants; day-granularity windows absorb sub-day ambiguity for unlabeled exports; a future DDP format change that reintroduces timezone ambiguity re-opens this record (trigger: a new `Date` format variant appearing in `date_parse_failures`); never strip `watched_at_raw` to save space.
- **Why:** the empirical + documentary trail above; the cost asymmetry (a wrong silent assumption miscategorizes every window filter; the raw column makes the assumption non-fatal).

Then run `adg lean index --root .` (the pre-commit hook re-checks).

- [ ] **Step 5: Fix the `parse_watched_at` comments**

In `src/ingest.rs`, update the `FORMATS` entries and add the verdict doc comment (adjust ADR number + verdict wording to reality):

```rust
/// Parse a DDP `Date` string into a unix timestamp, interpreting the naive
/// value as UTC per ADR-0039 (evidence: TikTok's May-2026 export pipeline
/// labels these strings with a literal " UTC" suffix; operator spot-check
/// against known watch moments confirmed no local-time skew; July-2026
/// exports dropped the suffix but not — by pipeline continuity — the
/// convention). The raw string is preserved in watch_history.watched_at_raw
/// (schema v4) so a future reinterpretation never requires re-ingest.
fn parse_watched_at(s: &str) -> Option<i64> {
    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S",     // production TikTok DDP (July 2026 exports) + synthetic fixtures
        "%Y-%m-%d %H:%M:%S UTC", // production TikTok DDP (May 2026 exports)
    ];
    for fmt in FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Utc.from_utc_datetime(&naive).timestamp());
        }
    }
    None
}
```

(Behavior identical — comments only. If Step 3 lands on local-time, the doc comment instead documents the i64 as "naive local time stored as-if-UTC; offset unknown; consumers treat accordingly" and cites the ADR.)

- [ ] **Step 6: Verify + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green; no behavior change (comment-only code diff).

```bash
git add src/ingest.rs docs/decisions/
git commit -m "docs(adr): DDP timestamp timezone verdict — <verdict> (ADR-00NN); fix parse_watched_at format provenance comments

Evidence: ' UTC' suffix in May-2026 PI export (2d89860); operator empirical
spot-check <result>; July-2026 real-donor exports use the no-suffix format
(previously mislabeled 'synthetic fixtures' in the FORMATS comment)."
```
