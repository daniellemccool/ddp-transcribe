# Bake findings — operational observations

Operational findings from bake runs; not code-quality FOLLOWUPS. These are
empirical observations made while running the system on real workspaces
(SRC A10 in particular); they may or may not need a code change. Sibling
files: `docs/FOLLOWUPS.md` (active-scope code-quality entries),
`docs/cosmetic-followups.md` (deferred-indefinitely items),
`docs/archive/followups-resolved.md` (append-only history).

**Discipline:** entries that record unverified hypotheses must prefix the
hypothesis with `**Hypothesis (unverified):**` so the next operator knows
to verify before acting (per AD0020).

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
