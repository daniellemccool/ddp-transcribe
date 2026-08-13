# FOLLOWUPS — Plan C active entries

Active-scope review items routed to Plan C (short-link resolution,
multi-engine, storage scale). See `../FOLLOWUPS.md` for the scope index
across all epics; `../cosmetic-followups.md`, `../bake-findings.md`,
`../archive/followups-resolved.md` for sibling categories. The
unverified-hypothesis prefix rule (`**Hypothesis (unverified):**`) applies
here per 0020.

---

### SHORT_LINK_RE does not handle query parameters on short links

**Found in:** T5 code quality review.
**Disposition:** Deferred to Plan C.
**Trigger to revisit:** Plan C planning session, before short-link resolution lands.

The short-link regex in `src/canonical.rs` ends with `/?$`:

```
^https?://(?:vm\.tiktok\.com|vt\.tiktok\.com|(?:www\.)?tiktok\.com/t)/[A-Za-z0-9]+/?$
```

This means a tracking-parameterized short link such as
`https://vm.tiktok.com/ZMabcdef/?utm_source=share` falls through to
`Canonical::Invalid` rather than `Canonical::NeedsResolution`.

CANONICAL_RE handles `?` correctly via `(?:/|\?|$)`. The asymmetry is real.

**Plan A impact:** small. Plan A only logs short links and skips them; the
miscategorization just shifts a count from `short_links_skipped` to
`invalid_urls_skipped` in `IngestStats`. Both end up not transcribed.

**Plan C impact:** real. Plan C will pick up rows from `pending_resolutions`
for HEAD-redirect resolution. Query-stringed short links would never reach
that table → silent data loss for those URLs.

**Suggested fix (when Plan C lands):** change the SHORT_LINK_RE suffix to
something like `(?:/[A-Za-z0-9]*)?(?:\?.*)?$` (match optional trailing slash,
then optional query string). Add a coverage test for both forms.

If DDP exports turn out to commonly include `?utm_source=…` on shared short
links, consider promoting this to a fixed bug in Plan B's first iteration
rather than waiting for Plan C — depends on what the donation extraction
script actually emits.

---

### `claim_next`'s doc comment states the 19-digit id guarantee unconditionally

**Found in:** v0.5.0 census-completion Task 03 (recency claim order),
parked by operator ruling.
**Disposition:** Latent, not a bug today. `Store::claim_next`
(`src/state/mod.rs:669-673`) documents the recency ordering as relying on
"video_id is a 19-digit snowflake, so DESC text order = DESC creation
time" — stated as a property of every row the function claims, with no
`canonical` qualifier. The v6→v7 migration guard that actually enforces
this (`src/state/migrate.rs`, the `v6→v7: canonical id-width census`) only
censuses `WHERE canonical = 1` rows and refuses to migrate if any of
*those* violate the width/digit assumption — non-canonical rows are never
checked. Production `ingest` only ever inserts `canonical = true`
(`src/ingest.rs:407`), so every pending row today is canonical and the doc
comment's unconditional claim happens to be true in practice.
**Trigger to revisit:** Plan C short-link resolution — the point where
non-canonical (`canonical = 0`) rows start landing in `videos` and can
reach `claim_next`'s pending set. At that point either scope the doc
comment to canonical rows explicitly, or widen the migration guard (and
`claim_next`'s ordering guarantee) to cover non-canonical ids too —
whichever Plan C's actual non-canonical id shape turns out to need.

---

### `output::shard` slices by bytes; panics on non-ASCII input

**Found in:** T8 code quality review (opus).
**Disposition:** Latent footgun; defer to whenever a `VideoId` newtype is introduced.
**Trigger to revisit:** any task that introduces a typed `VideoId`, or any task that begins accepting video IDs from a source other than the DDP-JSON parser.

`src/output/mod.rs::shard` does `&video_id[len-2..]`, which slices by bytes.
For multi-byte UTF-8 input where `len-2` lands mid-codepoint, this panics.
Real TikTok video IDs are ASCII digits and Plan A's parser only ever produces
those, so this is not exploitable today. The function takes `&str` rather
than a `VideoId` newtype, so the ASCII-only contract is implicit.

The natural fix arrives whenever the project introduces a `VideoId` newtype
(probably Plan B or Plan C, when DB rows and trait boundaries start passing
IDs around as values rather than `&str`). At that point, `shard` should be
a method on `VideoId` and the byte-slice is safe by construction.

Lowest-cost stopgap before then: add a debug assertion or a one-line doc
comment stating the ASCII-only contract.

---

### Consider promoting 0010's pass-through rule to a meta-process ADR

**Found in:** T1 (ADR drafts for Plan B Epic 1).
**Disposition:** Deferred to Plan C planning.
**Trigger to revisit:** When Plan C surfaces speculative-aggregation pressure for new derived data (comments, video metadata, etc.), evaluate whether the pass-through rule should be promoted from 0010's scope to a standalone meta-process ADR alongside 0001–3.

The pass-through rule ("raw pass-through is canonical for research signals; only
compute summaries needed for pipeline operation, indexing, or cheap sanity checks")
is currently codified in 0010 (raw_signals schema). It generalizes beyond Plan B
Epic 1. If it surfaces in Plan C as a recurring pattern, promote it to a standalone
ADR.

---

### `decode_wav` trusts float-format WAV sample values

**Found in:** T3 (WAV decoder) — codex-advisor code-quality review.
**Disposition:** Deferred. yt-dlp's ffmpeg postprocessor emits PCM_S16LE in Plan B; the float path in `decode_wav` is dead code for production input and the cost-vs-benefit of validating it now is low.
**Trigger to revisit:** If any future fetcher (alternate downloaders — a Research-API fetcher is ruled out, see README "Why scrape") introduces float-format WAV input, add finite/range validation to `src/audio.rs:decode_wav`'s `SampleFormat::Float` arm — reject `NaN`, `inf`, and out-of-`[-1.0, 1.0]` values with a new `AudioDecodeError` variant. The module is the audio invariant boundary; the float path should not trust whatever hound yields.

---

### Per-token `id` + `text` roughly doubles JSON artifact size vs `{p, plog}` only

**Found in:** T10 (artifact schema freeze) — implementer note.
**Disposition:** Pretty→compact JSON component landed in perf-tweaks `decdf6f`; drop-text-field component remains deferred pending 0010 amendment + bake validation that downstream filtering still works on `id`-only tokens.
**Trigger to revisit:** Plan C reviews artifact storage layout, OR observed
shard-disk pressure during a bake.

**Partial resolution by perf-tweaks `decdf6f`:** the `to_vec_pretty` → `to_vec` swap removed ~3× pretty-print indentation bloat from the per-token raw_signals payload. The dropping-`text`-field half of the original finding is unchanged: per 0010's pass-through rule, downstream consumers need both `id` and `text` to filter special tokens (`[BEG]`, `[END]`, `<|en|>`, etc.) which numerically include but lexically distinguish themselves from content tokens. Dropping `text` requires either (a) an 0010 amendment that relaxes the pass-through rule for tokens, OR (b) a sparse-token mode that keeps `text` only for special tokens. Neither is in scope for the perf-tweaks worktree.

T10's `RawToken` carries `id: i32` and `text: String` in addition to
`p`/`plog`, matching T9's `TokenRaw` shape exactly. This is intentional per
0010's pass-through rule — downstream consumers need both fields to
filter special tokens (`[BEG]`, `[END]`, `<|en|>`, etc.) which numerically
include but lexically distinguish themselves from content tokens. The cost
is a roughly 2× growth in per-video JSON size compared to the `{p, plog}`-
only sketch in the original T10 brief.

At pilot scale (~10³ videos) this is irrelevant. Once the project hits
~10⁵–10⁶ videos (or shards a single donor's history that spans years), the
storage line item starts to matter. Two reasonable mitigations when this
surfaces:

1. **Streaming JSON gzip at the artifact-write boundary.** `atomic_write`
   currently writes raw bytes; wrap with `flate2::write::GzEncoder` and
   change the `.json` suffix to `.json.gz`. ~5–10× compression on token-
   heavy JSON in typical measurements.
2. **Sparse-token mode** — emit `id`+`text` only for tokens flagged as
   special (low `p` or matching the model's special-token id range), and
   the dense numeric pair `{p, plog}` for content tokens. Requires a
   schema_version bump (`"1.1"` or `"2"`); covered by 0010 comment-2's
   string-versioning rationale.

Option 1 is cheaper structurally; option 2 keeps the wire format inspectable.
Don't pre-optimize — wait for the storage line item to actually pinch.

---

### yt-dlp argv: no `--` separator before `source_url`

**Found in:** T11 code quality review (opus); finding 4 of the original
four-finding `YtDlpFetcher::acquire` entry. Split out at Epic 3 close:
findings 1–2 were resolved by Epic 3 (`9974d69`, archived in
`../archive/followups-resolved.md`), finding 3 moved to
`docs/followups/epic-5.md`.
**Disposition:** Deferred to Plan C (short-link resolution).
**Trigger to revisit:** when Plan C wires resolved URLs into the fetcher
pipeline.

`source_url` is bound as the last positional arg with no `--` separator.
Today this is safe because `source_url` always comes from
`Canonical::Valid`, whose regex anchors `^https?://`. Plan C will introduce
short-link resolution that produces resolved URLs from external sources; an
attacker-controlled or malformed URL beginning with `-` could be
reinterpreted as a yt-dlp flag. One-line defense: insert `"--".into()`
immediately before `source_url.to_string()` in the `args` vector.

---

### `scrub_cookie_path` doesn't handle canonicalized/relative path variants

**Found in:** Epic 3 final whole-branch review.
**Disposition:** Deferred to Plan C (multi-engine / alternate fetcher work is the likeliest place a path gets re-derived in a different form before reaching the redaction call; note a Research-API fetcher is ruled out — README "Why scrape" — so the realistic trigger is the leak report or an alternate downloader).
**Trigger to revisit:** Plan C multi-engine work, or any report of a cookie
path leaking into logs/state despite `--cookies-file` being set.

`src/fetcher/ytdlp.rs::scrub_cookie_path` matches the cookie path via exact
string equality against `path.display().to_string()`. If yt-dlp's stderr
(or a future engine's) echoes the path in a different form than what was
passed in — canonicalized (symlinks resolved), relative vs. absolute, or
with a trailing slash — the substring match misses and the path leaks into
the persisted `stderr_excerpt` uncorrected. Today's single fetcher/single
call site doesn't hit this; worth a normalize-before-compare pass (e.g.
`std::fs::canonicalize` both sides, or match on the basename as a fallback)
before a second engine's stderr conventions are in scope.

---

### Revisit DB-at-runtime transcript storage only under a concrete scale or research trigger

**Found in:** transcript-storage assessment (Epic 4a close-out, format-selector
worktree).
**Disposition:** Deferred — measured evidence says the current
artifacts-on-disk design is the right one at present scale. Runtime artifact
writing costs ~5-20ms/video (4 fsyncs, ~10-25KB) against a ~1-2s transcription
call, i.e. noise; and moving transcripts into the DB would make
sync-to-storage strictly worse, because a sqlite `.backup` of a 10-25GB
database re-ships the whole file where incremental rsync of per-video
artifacts ships only deltas.
**Trigger to revisit:** either (1) the ADR-0004 ~1M-small-files ceiling
approaches on the transcripts tree, or (2) SQL-queryable transcripts become an
actual research need. Absent one of those, do not open this.

If a trigger fires, the change is its own epic, not a task: schema v4 (a
transcripts table + migration), an export subcommand (DB → per-video files for
researchers who want files), and a redesign of the sync-to-storage path away
from whole-file DB shipping. The pipelined write path's 0008 invariant
(artifacts durable before `mark_succeeded`) would need a DB-transactional
restatement.

---

### T1 codex ADR-refinement bullets gated on multi-engine / CUDA-fallback work

**Found in:** T1 (ADR drafts for Plan B Epic 1) — codex-advisor code-quality
review. Re-routed here from `cross-epic.md` by the Epic 5b close-out
(2026-07-30): the entry's other three bullets are terminal and archived
(`../archive/followups-resolved.md`, "Resolved by Plan B Epic 5b") — 0011 and
0017 resolved 2026-07-29 via ADR-0041, the 0013 global-log-callback invariant
implemented by `ebc4ee0` + `2788483`. These three are all gated on work Plan B
deliberately does not do, which is why they move rather than close.
**Disposition:** Deferred to Plan C (multi-engine / multi-GPU / CUDA-fallback).
**Trigger to revisit:** per bullet, below.

- **0009 fallback Engine API preservation.** If the CUDA build fallback is ever
  invoked, the superseding ADR must preserve the public `WhisperEngine` API —
  samples in, `TranscribeOutput` out, `Arc<AtomicBool>` cancel — so the Epic 1
  implementations do not have to be rewritten around it. Re-surface when the
  fallback ADR is drafted.
- **0016 multi-engine GPU memory caution.** The "wraps a `WhisperPool` of N
  Engines" alternative in ADR-0016 risks duplicating model loads on a single
  GPU, since each Engine owns its own `WhisperContext`. Prefer multi-state on
  one context for same-GPU parallelism; keep the wrapper option only for
  multi-GPU or process isolation. Amend ADR-0016 when Plan C multi-state /
  multi-GPU work begins.
- **Error-variant enumeration.** ADRs 0012/0013/0014/0016 each reference typed
  error variants (`WhisperInitError::BackendMismatch`, `AudioDecodeError::*`,
  `TranscribeError::Cancelled`, worker-panic, closed-reply) but no record
  enumerates the canonical set. Write a small implementation-constraint ADR if
  the variants ever drift across files. Note the surface has already moved
  once: `BackendMismatch` became a struct variant carrying
  `{ expected, detected }` when the 0013 assertion shipped, and
  `DetectedBackend::GpuInitFailed` was added alongside it — so an enumeration
  written today would already be describing a moving target, which is part of
  why it waits for the multi-engine work that would stabilize it.
