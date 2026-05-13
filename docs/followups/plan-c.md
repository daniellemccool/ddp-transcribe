# FOLLOWUPS — Plan C active entries

Active-scope review items routed to Plan C (short-link resolution,
multi-engine, storage scale). See `../FOLLOWUPS.md` for the scope index
across all epics; `../cosmetic-followups.md`, `../bake-findings.md`,
`../archive/followups-resolved.md` for sibling categories. The
unverified-hypothesis prefix rule (`**Hypothesis (unverified):**`) applies
here per AD0020.

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

### Consider promoting AD0010's pass-through rule to a meta-process ADR

**Found in:** T1 (ADR drafts for Plan B Epic 1).
**Disposition:** Deferred to Plan C planning.
**Trigger to revisit:** When Plan C surfaces speculative-aggregation pressure for new derived data (comments, video metadata, etc.), evaluate whether the pass-through rule should be promoted from AD0010's scope to a standalone meta-process ADR alongside AD0001–3.

The pass-through rule ("raw pass-through is canonical for research signals; only
compute summaries needed for pipeline operation, indexing, or cheap sanity checks")
is currently codified in AD0010 (raw_signals schema). It generalizes beyond Plan B
Epic 1. If it surfaces in Plan C as a recurring pattern, promote it to a standalone
ADR.

---

### Lazy-allocate lang_state on first opt-in request

**Found in:** T8 (lang_probs opt-in) — codex-advisor code-quality review.
**Disposition:** Defer; eager allocation is acceptable for Epic 1 but the lazy pattern is the efficient default.
**Trigger to revisit:** Memory pressure becomes a binding constraint (multi-state per Plan C, or smaller dev VMs), OR Epic 4's `--compute-lang-probs` use becomes commonplace enough that the eager-allocation cost feels unjustified for non-opt-in workloads.

T8 currently allocates `lang_state` unconditionally in `WhisperEngine::new`'s
init phase. Since `compute_lang_probs` defaults false, every engine pays
~500MB-1GB of unused WhisperState memory until the feature is opted in.

Refactor target:

```rust
// In init phase: no lang_state allocation.
// In worker request loop:
let mut lang_state: Option<WhisperState> = None;
// ...
while let Some(req) = request_rx.blocking_recv() {
    if req.config.compute_lang_probs {
        if lang_state.is_none() {
            // Lazy allocation on first opt-in. If it fails, surface as
            // tracing::warn! + lang_probs: None (consistent with best-effort).
            match ctx.create_state() {
                Ok(s) => lang_state = Some(s),
                Err(e) => { tracing::warn!(...); /* no lang_probs this call */ }
            }
        }
        if let Some(ls) = lang_state.as_mut() {
            // run lang_detect on ls
        }
    }
    // ... rest of inference ...
}
```

Trade-off: lazy saves ~500MB-1GB when feature is unused; costs a one-time
allocation latency on first opt-in (~10-50ms on CPU; faster on GPU).

---

### `decode_wav` trusts float-format WAV sample values

**Found in:** T3 (WAV decoder) — codex-advisor code-quality review.
**Disposition:** Deferred. yt-dlp's ffmpeg postprocessor emits PCM_S16LE in Plan B; the float path in `decode_wav` is dead code for production input and the cost-vs-benefit of validating it now is low.
**Trigger to revisit:** If any future fetcher (Plan C API direct, alternate downloaders) introduces float-format WAV input, add finite/range validation to `src/audio.rs:decode_wav`'s `SampleFormat::Float` arm — reject `NaN`, `inf`, and out-of-`[-1.0, 1.0]` values with a new `AudioDecodeError` variant. The module is the audio invariant boundary; the float path should not trust whatever hound yields.

---

### Per-token `id` + `text` roughly doubles JSON artifact size vs `{p, plog}` only

**Found in:** T10 (artifact schema freeze) — implementer note.
**Disposition:** Accepted for Plan B Epic 1; revisit when storage cost becomes
load-bearing.
**Trigger to revisit:** Plan C reviews artifact storage layout, OR observed
shard-disk pressure during the A10 bake (T13), OR the artifact-storage cost
becomes a discussion topic in any capacity (donor count > pilot scale,
retention policy debate, etc.).

T10's `RawToken` carries `id: i32` and `text: String` in addition to
`p`/`plog`, matching T9's `TokenRaw` shape exactly. This is intentional per
AD0010's pass-through rule — downstream consumers need both fields to
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
   schema_version bump (`"1.1"` or `"2"`); covered by AD0010 comment-2's
   string-versioning rationale.

Option 1 is cheaper structurally; option 2 keeps the wire format inspectable.
Don't pre-optimize — wait for the storage line item to actually pinch.
