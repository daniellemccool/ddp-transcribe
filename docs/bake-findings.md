# Bake findings — operational observations

Operational findings from bake runs; not code-quality FOLLOWUPS. These are
empirical observations made while running the system on real workspaces
(SRC A10 in particular); they may or may not need a code change. Sibling
files: `docs/FOLLOWUPS.md` (active-scope code-quality entries),
`docs/cosmetic-followups.md` (deferred-indefinitely items),
`docs/archive/followups-resolved.md` (append-only history).

**Discipline:** entries that record unverified hypotheses must prefix the
hypothesis with `**Hypothesis (unverified):**` so the next operator knows
to verify before acting (per 0020).

---

## Residual yt-dlp no-audio failure rate after format-preference workaround

**Found in:** T13 bake (`@rtl.nl/video/7571766274108181792`); root-cause analysis 2026-05-13 against `yt_dlp/extractor/tiktok.py` v2026.03.17 + upstream issues yt-dlp/yt-dlp#15891 and yt-dlp/yt-dlp#16622.
**Disposition:** Format-selector workaround landed on `fix/ytdlp-prefer-download` (selector switched from yt-dlp default to `"download/b[vcodec=h264]/b"`). Residual reliability gap deferred to Plan B Epic 3.
**Trigger to revisit:** Epic 3 fetcher hardening; OR if pilot-scale bake observes the `unable to obtain file audio codec with ffprobe` error despite the workaround.

**Root cause (primary-source-confirmed):** TikTok's web API non-deterministically populates `bitrateInfo` with h265 variants that are served video-only at the CDN. yt-dlp's TikTok extractor (`tiktok.py:562-606`) unconditionally stamps `acodec: 'aac'` on every `bitrateInfo` entry via `COMMON_FORMAT_INFO`; it has no way to verify the claim. The default selector picks the highest-tbr format (often the lying h265 variant); the ffmpeg postprocessor then discovers via `ffprobe` that there is no audio stream. The bake-notes framing about "yt-dlp's auto-select walking OFF the listed menu" was a misreading: the listing and download invocations are separate API calls and can return different `bitrateInfo` arrays — the format isn't hidden, the API just rotates.

**Why the workaround works:** TikTok's `download` format (`tiktok.py:621-628`) is a pre-rendered share-link MP4 served as a static asset, distinct from the on-demand-muxed `bitrateInfo` pipeline. It's h264, pre-muxed, ~5 MiB at 540p, and empirically the most-validated path (it's what every "Save video" tap in the mobile app hits). Verified across 6 fixture URLs on 2026-05-13. The visible watermark only affects video pixels, which the pipeline discards.

**Residual gap Epic 3 should close:**

1. Classify `Postprocessing: WARNING: unable to obtain file audio codec with ffprobe` as `RetryableFailure::NoAudioStream` (a distinct variant from network errors / generic tool failures).
2. On classification, retry the whole `acquire` against the same URL. TikTok's API non-determinism means a second invocation typically returns a different (working) format menu. Upstream issue #16622 confirms even h264-preferring filters intermittently produce no-audio downloads.
3. Bound retries (e.g., 3 attempts with brief backoff) before marking the row `failed_retryable`.

The selector workaround and the Epic 3 retry compose cleanly — prevention reduces the rate; retry catches the residual. Do NOT revert the selector when retry lands.

**A10 verification (2026-05-13):** `--simulate --print "%(format_id)s"` sweep on the SRC workspace against the 20 unique URLs in `tests/fixtures/ddp/news_orgs/` confirmed the selector picks the `download` format on 20/20 — 100% hit rate. Fallback paths (`b[vcodec=h264]` and `b`) not exercised on news-org content. Expected to exercise on donor content where creators have disabled the share-button download in privacy settings; the fallback is still h264 and still smaller than the default selector, but the residual-gap retry work above becomes more important on that surface.

**Bake-notes cross-reference:** `docs/SRC-BAKE-NOTES.md` § "Plan B Epic 3 findings surfaced during bake" — Finding 1 (now superseded by this entry).

---

## RESOLVED 2026-05-13: `pipx inject yt-dlp curl-cffi` left yt-dlp with an unsupported curl_cffi version

**Found in:** T13 bake (workspace-side fetcher hardening attempt). Resolved on the SRC A10 workspace 2026-05-13.

**Hypothesis (unverified) — original framing, later disproved:** missing `libcurl4-openssl-dev` causing the C extension to build without proper libcurl linkage. The systematic-debugging discipline (diagnostic *before* fix) would have caught this earlier — recorded here so future bake entries mark unverified hypotheses explicitly.

**Actual root cause:** `pipx inject yt-dlp curl-cffi` (unpinned) grabbed `curl-cffi 0.15.0` — the latest release at bake time. yt-dlp 2026.03.17's networking handler at `yt_dlp/networking/_curlcffi.py:34-37` parses the curl_cffi version into a tuple and dynamically appends `(unsupported)` to the version string when the bound check fails. Empirically: yt-dlp 2026.03.17 accepts `0.14.0`, rejects `0.15.0`. The package was correctly installed and importable (`import curl_cffi` succeeded cleanly), but yt-dlp refused to load the handler at request-time, so `Request Handlers: urllib` (no curl_cffi) and all impersonate targets showed `(unavailable)`.

**Resolution:** `pipx install --force 'yt-dlp[default,curl-cffi]'` on the SRC workspace. This wipes the existing pipx venv and reinstalls yt-dlp with both:

- `[default]` — the full tested-recommended optional-dependency set (Cryptodome, brotli, mutagen, requests/urllib3/websockets, yt_dlp_ejs)
- `[curl-cffi]` — lets yt-dlp's own setup.py resolve to a curl-cffi version it tested against (resolved to `curl-cffi 0.14.0`)

Verification on the A10 (`yt-dlp -v --list-impersonate-targets`):

- Before: `Optional libraries: ..., curl_cffi-0.15.0 (unsupported), ...` + `Request Handlers: urllib` + all 5 targets `(unavailable)`.
- After: `Optional libraries: Cryptodome-3.23.0, brotli-1.2.0, certifi-2026.04.22, curl_cffi-0.14.0, mutagen-1.47.0, requests-2.34.0, sqlite3-3.45.1, urllib3-2.7.0, websockets-16.0, yt_dlp_ejs-0.8.0` + `Request Handlers: urllib, requests, websockets, curl_cffi` + 25+ impersonate targets `(available)`.
- Real-URL verification on `@pbsnews/video/7609743407577173262 --simulate`: no `attempting impersonation, but no impersonate target is available` warning.

**Operator runbook line for fresh SRC workspace provisioning:** install yt-dlp via `pipx install 'yt-dlp[default,curl-cffi]'`. Do NOT use `pip install` (Ubuntu 24.04's PEP 668 externally-managed-environment marker blocks it). Do NOT use `pipx inject yt-dlp curl-cffi` unpinned (will grab a curl-cffi version yt-dlp may reject). If something already in the venv must be preserved, fall back to `pipx inject yt-dlp 'curl-cffi==<X>' --force` with `<X>` matching yt-dlp's tested range.

**Lessons captured:**

1. `pipx install --force '<tool>[extras]'` lets the tool's packaging declare its tested-compatible optional-dep versions, rather than relying on operator guesses or pipx's default-latest. This is the cleanest install pattern when an optional dep has version-range constraints.
2. yt-dlp marks dep status dynamically via the `_yt_dlp__version` attribute (`_curlcffi.py:37`), so the verbose `--list-impersonate-targets` output is the load-bearing diagnostic — not the bare `import curl_cffi` test the bake initially proposed.
3. The original bake-time hypothesis was carried forward without verification. The systematic-debugging discipline — diagnostic *before* fix — would have caught the wrong-hypothesis path earlier. Future FOLLOWUPS entries should mark unverified hypotheses explicitly as `**Hypothesis (unverified):**` so subsequent operators don't apply fixes built on guesses.

**Still open (separate question, not blocked by this resolution):** whether working impersonation actually changes anything at SURF scale. Both today's local 6-URL verification (without impersonation) and the A10 8-URL run (without impersonation) succeeded cleanly on real Dutch/English/Tagalog content. The "is impersonation needed at small scale" question is empirically leaning toward "no," but the N=20+ comparator mini-bake (impersonation on vs off on a representative URL sample) would settle it definitively. This question stands as a separate FOLLOWUPS for Plan B Epic 3 / production-grant scoping.

**Bake-notes cross-reference:** `docs/SRC-BAKE-NOTES.md` § "Plan B Epic 3 findings surfaced during bake" — Finding 2 (now resolved by this entry).

---

## yt-dlp `-S +size,+br,+res,+fps` is inert against `news_orgs` fixture

**Found in:** Perf-tweaks T8 bake (commits `9725055` T7 code + this entry T8 finding).
**Disposition:** Change is correct and defensive; bake confirms no regression on the success path. No follow-up needed unless a future fixture exercises the `b[vcodec=h264]/b` fallback.
**Trigger to revisit:** if a future bake or production run observes the `download`-format selector failing (e.g., creators disabling downloads at scale, TikTok deprecating the static-asset endpoint, fixture demographics shifting), confirm `-S` actually preferring smaller formats in the resulting fallback path.

**Observation:** All 25 news_orgs URLs (fixture has grown from T13's 20 to 25 entries with the bbcnews/aljazeeraenglish/gmanews/cnews additions) resolved to `format_id = download` both pre- and post-T7. `-S` only orders within a selector match; since the first selector token `download` is a literal format ID, the success path bypasses sort entirely. The change therefore had no measurable effect on this fixture.

**Bake environment context:** Run on the dev machine (Arch Linux + yt-dlp 2026.03.17) rather than the A10 workspace. T8's plan flagged the trade-off explicitly: yt-dlp's selector-resolution logic is environment-independent (same TikTok API responses, same selector code), so the dev machine produces the same signal at lower cost than waiting on the A10 for a bake that already-known to be unlikely to exercise the fallback. The A10 might surface operational issues (curl-cffi flakiness per the resolved `docs/bake-findings.md` entry above) that the dev machine doesn't, but those affect *network reliability*, not *format selection*.

**Raw bake table:** 25 URLs × 2 invocations = 50 `yt-dlp --print` calls. Diff between pre-change (no `-S`) and post-change (with `-S +size,+br,+res,+fps`) yt-dlp output: empty. Full table at `/tmp/bake-t8/results.tsv` during the bake; not preserved in-repo because the result is "no diffs anywhere."

| Side | format_id counts |
|------|------------------|
| pre  | download × 25    |
| post | download × 25    |

**Why we kept the change:** defensive — TikTok's `download` format depends on creator settings (some videos disable downloads) and on yt-dlp's `download`-format extractor staying functional (yt-dlp #15891 and #16622 document upstream non-determinism in adjacent paths). When the fallback runs, `-S` ensures we don't accidentally pick a needlessly-large h264 variant. The cost of the change is zero (one extra flag in the argv); the benefit is conditional but real.

---

## `set_no_timestamps(true)` causes content loss and repetition hallucinations across 30s window boundaries

**Found in:** Perf-tweaks T10 bake on SRC A10 (commit `042f038` introduced; revert commit `368fe4b` on same branch backs it out; this entry routes the finding).
**Disposition:** **Optimization invalid: semantic regression.** Reverted. Not a tunable; do not re-attempt with parameter adjustments or audio-side chunking — those workarounds reintroduce the same problem class. (Framing per codex-advisor review 2026-05-18: "keep timestamps enabled even if you do not serialize them"; chunk-boundary independence is the same family of issue 0015 explicitly rejected for `whisper_full_parallel`.)
**Trigger to revisit:** only if whisper-rs / whisper.cpp ships a change that decouples `no_timestamps` from the seek-advancement control signal (i.e., a different mechanism for inferring speech boundaries when timestamp tokens are suppressed). Not a "Plan C might want this" item.

**Hypothesis (unverified) at spec time:** Source-level inspection of `~/src/whisper.cpp/src/whisper.cpp` (logits-suppression block + the `if (single_segment || no_timestamps)` seek branch) suggested `no_timestamps=true` only suppresses timestamp tokens in the logits — content tokens would be preserved bit-for-bit, only the segment grouping would change (one segment per 30s window instead of multiple timestamp-split segments). The spec proposed an empirical bake to confirm this on real audio.

**Observation:** the bake on `news_orgs` (25 URLs, `large-v3-turbo-q5_0`, A10) ran 4 sides (pre run1 + pre run2 + post run1 + post run2). Intra-side determinism is perfect — run1 vs run2 byte-equal (modulo `transcribed_at`). Cross-side `.txt` content shows material divergence on every video. Two representative examples:

- **Video `08/7636791907133164808`** (GMA Regional TV, Tagalog, ~131s). Post-change `.txt` drops the sentence `"Aanihin ang mga bulaklak nito na nagsisilbi pangunahing hibla"` (replacing it with truncated `"Laklak nito na nagsisilbi pangunahing hibla"`) and loses the entire closing paragraph beginning `"Sumunan ginabuhat na ito ang efforts din na ito..."` through the speaker's final sign-off paragraph. Roughly the last ~25 seconds of audio produces no transcript.
- **Video `14/7612056384116477214`** (Al Jazeera English, ~120s+). Post-change `.txt` exhibits classic Whisper repetition hallucinations (`"...launching multiple strikes towards Israeli cities and Israeli cities and Israeli cities and Israeli cities and Israeli cities and U.S. military assets..."` and `"...where the U.S. has key regional air defense in the heart of the U.S. has key regional air defense..."`). An entire paragraph about the Al-Adaid / Al-Dhafra / Al-Salim / Prince Sultan air bases is missing from the post transcript.

Token-count summary across all 20 videos: post had fewer tokens than pre on 19/20 (range 1–185 fewer; median ~30 fewer); one video (`62/...262`) had 16 more in post — likely repetition-driven inflation.

**Root cause (refined via codex-advisor review):** the timestamp token IS whisper.cpp's control signal for inferring where speech ends within a 30s decode window. `no_timestamps=true` suppresses the token in the logits step, which deletes that control signal entirely. The `seek_delta = 100*WHISPER_CHUNK_SIZE` branch is the *downstream consequence* — whisper.cpp falls back to a context-free fixed 30s stride because there's no longer a model-inferred boundary to seek to. Content that spans the resulting arbitrary cuts either disappears (partial trailing utterance) or triggers entropy-guard / temperature-fallback repetition cascades when the next-window prompt context loses alignment. The earlier framing — "seek_delta forces a fixed window jump" — was backwards: it's the loss of the control signal that's primary; the seek behavior is what falls out.

**No safe workaround at our scale.** Audio-side chunking to ≤ 30s would reintroduce the same chunk-boundary problem class that 0015 explicitly rejects for `whisper_full_parallel`. TikTok speech crosses arbitrary 30s cuts frequently. Sampling tweaks may reduce repetition rate but cannot restore missing boundary content. Recommendation (codex-advisor): keep timestamps enabled in `FullParams` even though we don't serialize them into the artifact JSON — the small inference-cost win is not worth the semantic regression.

**Cross-reference:** revert commit on `feat/perf-tweaks` post-merge to main; `scripts/bake-t10-no-timestamps.sh` + `scripts/bake-t10-compare.sh` are the harness; raw bake artifacts were under `/tmp/bake-t10/{pre,post}/run{1,2}/transcripts/` (ephemeral; not preserved in-repo).

**Spec correctness verified.** The bake's role per spec § Bake plan #2 was precisely "confirm or refute the hypothesis empirically." It refuted. The verification-before-completion discipline worked exactly as designed — the cheap source-level hypothesis got expensive real-data validation before the change shipped to main.
