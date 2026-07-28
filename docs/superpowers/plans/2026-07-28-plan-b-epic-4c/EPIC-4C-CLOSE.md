# Plan B Epic 4c — Epic Close

**Branch:** `feat/plan-b-epic-4c`
**Status:** all 6 tasks complete. Every yt-dlp fetch now captures the video's metadata as an unparsed versioned envelope into `video_metadata_raw` — on failure paths too, at zero extra network cost — and the post-run `load-metadata` subcommand parses those blobs into eight nullable schema-v5 columns on `videos`, replayably. Plus the pre-production hardening pass (unique atomic-write tmp names, artifact fsyncs outside the store lock). Capture-chain decision recorded as ADR-0042; ADR-0008 revised in place.

## What landed

| Task | Commit(s) | Subject |
|---|---|---|
| 01 | `2ac4775` | Schema v5 — `video_metadata_raw(video_id PK, fetched_at, raw_json, FK→videos)` + 8 nullable metadata columns on `videos`; migrate ladder v4→v5; `Store::upsert_metadata_raw` (last-write-wins) |
| 02 | `493ef56`, `f9de7c3`, `eb0ffc6`, `1accb54`, `afa0253`, `4d067a9` | Fetch-time capture: `--no-simulate --print <template>`, 64 KB stdout retention, envelope `{"schema":1,"printed":…}`, `acquire`/`fetch_and_decode` widened to `(Option<MetadataCapture>, Result<…>)`, `FakeFetcher::canned_metadata`. Three fix loops, then the operator caption descope (`afa0253`) |
| 03 | `1b458e0` | Both pipeline paths upsert the envelope **before** outcome dispatch, best-effort (warn + continue); pipelined store lock scoped to the insert; both placeholder `#[allow(dead_code)]`s lifted |
| 04 | `1c79a0e`, `46dae5f` | `load-metadata`: `src/metadata_loader.rs` (`parse_envelope`, `load_metadata`, `LoadStats`, PAGE_SIZE 10,000), `Store::metadata_raw_page` + `apply_metadata_batch`, `--dry-run`, missing-DB bail; fix loop split the page query |
| 05 | `964e9c2` | Pre-production hardening: unique `atomic_write` tmp names (`.tmp-{pid}-{seq}`), `write_artifacts_durable` / `mark_after_artifacts` split so the store lock covers only the mark, honest `cleanup_tmp_files` count; ADR-0008 revised to name the pair as ordering owner |
| 06 | `ee24494`, this commit | Ignored live e2e; ADR-0042 via `adg lean new`; architecture + operations doc pass; FOLLOWUPS (5 entries, 2 groups); this close doc |

**Verification at close:** `cargo fmt` clean; `cargo clippy --all-targets -- -D warnings` clean; `cargo test --features test-helpers -- --test-threads=1` → **309 passed, 0 failed, 9 ignored**. Suite progression 283 (branch base) → 286 → 294 → 296 → 305 → 309 → 309. The ignored count rose 8 → 9: Task 06's live e2e is `#[ignore]`d and does not count against the suite. `adg lean index --root .` — 34 records, 0 failures, 2 advisory warnings (0040/0041 Decision length, both pre-existing).

## Deviations and adjudications, disclosed

1. **Task 02 — the caption arc, ending in an operator descope.** Caption/subtitle collection was specced, built twice, and then removed before merge. Sequence: subtitle flags on the primary invocation were flagged as an invariant risk (a subtitle download failure raises `DownloadError` in the pinned yt-dlp 2026.07.04 — source-verified at `YoutubeDL.py` ~4498 — so it could flip a good fetch to a spurious video failure) → redesigned as a guarded secondary invocation → a Fable consult found the gate could never open **and** that the "0/46 caption tracks ≈ 0%" probe reading was an artifact of probing without the listing flags → an honest re-probe measured **4/11 ≈ 36%** coverage → the listing was repaired onto the primary at zero language selection → the reviewer's residual finding was that listing-only capture still spends the primary's timeout budget (`tiktok.py:333` via the `common.py:3870` gate, all `fatal=False`) → **operator adjudication 2026-07-28: descope entirely** (`afa0253`). The yield does not justify the failure surface and request pressure. Schema v5 was amended in-branch (`captions_json` dropped, 8 typed columns not 9); plan files 01–04 and 06 were updated and archive notes left on 02/03. **The corrected ~36% figure is the number of record — the retracted ≈0% must not be cited.**
2. **Task 04 — pagination fix (brief-inherited defect).** The brief's page query used a single `WHERE (?1 IS NULL OR video_id > ?1)` shape, which SQLite plans as a full table scan *per page* — O(n²) over a 3M-row table. Split into separate first-page and subsequent-page prepared statements; EXPLAIN-verified in both directions by the reviewer (`46dae5f`).
3. **Task 04 — module registration judgment call.** `metadata_loader` is declared in `src/main.rs` only, matching `status`, not in `src/lib.rs` as the brief's call path implied — `main.rs` uses a bin-local `state::Store` throughout, so the brief's path would have crossed the lib/bin type boundary. This is a symptom of the duplicated module tree, now filed for Epic 5 (below).
4. **Tasks 03 + 05 — two `#[allow(dead_code)]` lifts.** Task 01's placeholder allow on `upsert_metadata_raw` and the pre-existing one on `CommandOutcome.stdout` (whose justification, "all call sites pass 0", stopped being true) were both removed at Task 03 when real consumers landed, per ADR-0002.
5. **Task 05 — ADR-0008 revised in place, not superseded**, and with one Guidance bullet beyond the brief (the lock scope, which is the entire point of the split). Task 05 also refreshed five other doc comments that still named `write_artifacts_and_mark` as the 0008 ordering owner.
6. **Task 06 — migrate ladder is *four* stages, not five.** The dispatch brief said five; the code has four sequential stages (v1→v2, v2→v3, v3→v4, v4→v5) spanning five schema versions. The docs record four, matching `src/state/migrate.rs`.
7. **Task 06 — two files edited beyond the brief's list.** `transcription.md` and `orchestration.md` still asserted that the transcribe worker calls `write_artifacts_and_mark` and that `atomic_write` uses a fixed `.tmp` suffix — both false after Task 05. Corrected rather than left standing.
8. **Task 06 — a stale code comment left in place, filed instead.** `src/metadata_loader.rs` still says `video_metadata_raw` is "6–12 GB at production scale", a figure that predates the caption descope. The corrected estimate (~3–6 GB) is in the capacity-estimate FOLLOWUPS entry; the comment itself is a cosmetic fix for whoever next edits that file — a docs commit is the wrong place to touch source.

## Live e2e evidence

`tests/e2e_real_tools.rs::real_fetch_captures_metadata_envelope` (`#[ignore]`d; needs network + yt-dlp, no whisper model) drives `YtDlpFetcher::acquire` directly and asserts capture present → envelope parses → `schema == 1` → printed line parses with a non-empty `id` and a non-null `description`. Run against a real corpus URL on yt-dlp 2026.07.04:

```
$ DDP_TRANSCRIBE_E2E_URL=<real url> cargo test --test e2e_real_tools \
      real_fetch_captures_metadata_envelope -- --ignored --test-threads=1
test real_fetch_captures_metadata_envelope ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out
```

The same URL's print line, measured directly the same day (one line, **660 B** — values elided): `{"id": "…", "title": "…", "description": "…", "uploader": "…", "uploader_id": "…", "channel_id": "…", "timestamp": …, "duration": 23, "view_count": 33400, "like_count": 1753, "comment_count": 27, "repost_count": 1044}` — all twelve fields populated, consistent with the plan's ~615 B measurement and ~100× under the 64 KB capture cap. This test is also the designated re-verification hook after any yt-dlp upgrade.

## Deliberately omitted (spec non-goals, restated)

Transcript artifact schema unchanged; no `status` surface extension; no pilot-corpus backfill; no delivery-export subcommand; no comments (Research API only); no production-run capacity planning. Added during the epic: **no caption/subtitle track collection** (item 1 above) — the creator's caption *text* is captured as `videos.video_description`, which is the PI's "caption" in Research API vocabulary.

## FOLLOWUPS filed

Five entries across two scope groups (`docs/FOLLOWUPS.md` index; bodies in `docs/followups/`):

- **Production run** (new group, `production-run.md`): capacity estimate before the first non-pilot batch (2,982,471 uniques; throughput, window narrowing, `video_metadata_raw` ~3–6 GB, transient WAVs, rate-limit exposure); `video_metadata_raw` prune/VACUUM decision after the first batch's `load-metadata`.
- **Epic 5** (`epic-5.md`): `main.rs` re-declares the library's module tree (double compilation, broadened `pub` surface, a driver of the accumulated `dead_code` allows — cites ADR-0002's deferred bin/lib reassessment; operator finding 2026-07-28); startup `cleanup_tmp_files` sweep can delete a concurrent live process's in-flight tmp (pre-existing, multi-process deployments only); `upsert_metadata_raw` is not claim-guarded (accepted last-write-wins tradeoff — snapshot staleness only, self-heals on re-fetch).
