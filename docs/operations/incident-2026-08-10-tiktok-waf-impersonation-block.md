# Incident 2026-08-10 — Akamai WAF blocks the impersonated client (campaign-wide fetch stop)

**Status: DIAGNOSED, remedy proven, NOT YET APPLIED to the campaign pool.**
**The VM is currently in a half-applied state and CANNOT fetch — see
[Current state](#current-state-2026-08-11) before running anything.**

Second outage in five days, and a direct consequence of the first one's
mitigation. Read with
[`incident-2026-08-06-tiktok-tls-403.md`](incident-2026-08-06-tiktok-tls-403.md)
(incident 1 — its `--impersonate chrome` fix is what put us in this blast
radius), ADR-0033 (evidence-derived classification — this is the strongest
instance yet), ADR-0037 (the classification table), and ADR-0038 (the
download-first format selector, which turns out to matter here).

All timestamps UTC unless marked local (VM local = UTC+2).

## Summary

At **2026-08-10 19:14:09** every fetch began failing. Root cause: TikTok's
Akamai WAF began serving a **537-byte HTML block page with HTTP 200** to
clients presenting curl_cffi's Chrome TLS fingerprint — the fingerprint
incident 1's mitigation had told yt-dlp to use on every request. yt-dlp's
TikTok extractor cannot recognise that page as a block, so it reports
`Unexpected response from webpage request` and asks the user to file an
upstream bug. It is not a bug; it is a refusal.

This is **not specific to us**: upstream
[yt-dlp#17403](https://github.com/yt-dlp/yt-dlp/issues/17403) collected
reports from the US, Europe and Brazil, on residential lines and VPSes,
within the same ~48 hours.

The operator stopped the run **~7 minutes after onset** (contrast incident
1's 60 unattended hours). Cost: ~1,400 attempts, zero terminal writes, no
data loss.

**Remedy (proven, see [Proof](#proof-the-remedy-works-end-to-end)):** fetch
the **canonical** `www.tiktok.com/@<user>/video/<id>/` URL with
**impersonation off**. Both halves are required — neither works alone.

## Timeline

| When | What |
|---|---|
| 2026-08-09 ~11:30 | Incident 1 mitigation applied (`~/.config/yt-dlp/config` → `--impersonate chrome`); 50-video validation batch passes |
| 2026-08-09 20:08:48 / 20:09:09 | Runs 9 and 10 start (2 GPU instances, `retries: 2`, `download_workers: 2`, `max_videos: 50000` each) |
| 2026-08-09 → 08-10 | Healthy operation: flat ~1,850 successes/hour, ~77% success rate, 58,601 videos transcribed |
| **2026-08-10 19:14:09** | **Onset.** 9 rows fail `Unexpected response from webpage request` over 14 seconds |
| 2026-08-10 19:14:11 → 19:21:21 | 1,351 further rows fail `Unsupported URL: https://www.tiktok.com/share/video/<id>/` |
| 2026-08-10 19:21:21 | Operator stops the run (~7 min after onset). Runs 9 and 10 remain `OPEN` in `batch_runs` |
| 2026-08-11 | Diagnosis: probe matrix, local reproduction, page dump. Remedy proven by smoke test |

Both failure texts are the same event. `Unsupported URL` is the *generic*
extractor's message when the redirect chain leaves it holding
`www.tiktok.com/share/video/<id>/`; `Unexpected response` is the *TikTok*
extractor's message when it is handed the block page directly. Which one you
get depends on which extractor reaches the WAF first.

## DB evidence (snapshot pulled 2026-08-11, content ends 19:21:21)

- Failure classes in the final hour: **`YtDlpOther` 1,360** (baseline for
  that class: **zero**), against `HttpError` 14 and `NoPermission` 4 — i.e.
  ordinary background attrition plus one entirely new class. Split:
  1,351 `Unsupported URL` / 9 `Unexpected response`.
- `pending` mutations (= requeues) jumped from ~100/hour to **1,378 in
  ~21 minutes**, a 14× spike.
- Pool at stop: pending 1,908,746 (57,352 @ attempt 0 + 1,851,394 @ attempt 1),
  succeeded 880,387, failed_terminal 173,404, failed_retryable 19,920,
  in_progress 4 (stale claims from the stop; ADR-0024's sweep recovers them
  with no attempt bump).
- **`max(attempt_count) = 1` across all 2,982,461 rows.** Nothing approached
  the cap. The whole wave is recoverable for free.
- Terminal writes stopped at onset — the incident-1 signature repeating.

`YtDlpOther` growth was a **pre-registered alarm**: the incident-1 FOLLOWUPS
entry named "the first unexplained growth of `YtDlpOther`" as its trigger and
called it a cheap census query worth adding to periodic checks. It was never
added. An hourly run of it would have caught this in minutes.

## Mechanism (established by reading the extractor and dumping the page)

`yt_dlp/extractor/tiktok.py`, `_extract_web_data_and_status`:

1. `get_webpage()` calls `_download_webpage_handle(..., **impersonate=True**)`
   — **hardcoded in the extractor**. There is no CLI flag to disable it;
   removing curl_cffi from the interpreter is the only lever.
2. It looks for `__UNIVERSAL_DATA_FOR_REHYDRATION__` (the page's embedded
   app-state JSON).
3. If absent it assumes a WAF *challenge* interstitial and calls
   `_solve_challenge_and_set_cookies(webpage)`.
4. That function requires an element with `id='cs'` carrying base64 challenge
   data. Missing, and the page does not contain `Please wait...`?
   → `raise ExtractorError('Unexpected response from webpage request')`.

So our error means: **the page was neither the data-bearing page nor a
challenge yt-dlp recognises.** What TikTok actually returned, captured with
`yt-dlp --write-pages` (537 bytes, HTTP **200**):

```html
<!doctype html>
<html>
<head><title>Site Maintenance</title>...</head>
<body>
    <div id="error">
    <h1>Oops! Something went wrong</h1>
    <div><hr>
        <p>Please contact your administrator with the error code: 0.5b37655f.1786463639.58155046</p>
    </div>
    </div>
</body>
</html
```

That `0.<hex>.<epoch>.<hex>` reference format is Akamai's block receipt
(`1786463639` decodes to 2026-08-11), and the success-path response headers
confirm the fronting: `X-Akamai-Request-ID`, `X-Cache: ... AkamaiGHost`.

**Three layers all misdescribe the cause** — the definitive ADR-0033
specimen:

| Layer | What it says | Truth |
|---|---|---|
| HTTP status | `200 OK` | denied |
| HTML body | "Site Maintenance / Oops! Something went wrong" | denied |
| yt-dlp stderr | "Unexpected response … please report this issue" | denied, not a bug |

Only the opaque Akamai reference code is honest.

### Controlled experiment

| Egress | curl_cffi | URL | Result |
|---|---|---|---|
| Campaign VM (SURF) | present (0.15.0) | canonical | **block page** |
| Campaign VM | **absent** | canonical | **success** — valid 16 kHz WAV |
| Campaign VM | absent | share (`tiktokv.com`) | 403 (incident-1 gate, still live) |
| Residential NL | **present** (0.15.0, venv) | canonical | **block page** — reproduced |
| Residential NL | absent | canonical / share | success |

Same yt-dlp (2026.07.04), same video, one variable. **Impersonation alone is
sufficient to cause the block, independent of egress.** The residential
reproduction is what rules out "SURF's IP earned this."

### Why the impersonated fingerprint specifically (hypothesis, unverified)

**Hypothesis (unverified):** the WAF targets curl_cffi's TLS ClientHello as a
high-precision scraper signature. Two supporting observations, neither
conclusive:

- The **plain** request sends `User-Agent: Chrome/145.0.0.0` (yt-dlp's
  default) over a python/OpenSSL ClientHello and **passes**. So the block is
  keyed on the **TLS fingerprint**, not the UA.
- curl_cffi 0.15.0's newest targets are **Chrome-133 / Chrome-136** while real
  Chrome is 140+. A coherent-but-stale browser signature is a strong bot
  signal, and curl_cffi is *the* standard scraping stack, so its exact
  ClientHello is cheap for a WAF to enumerate.

Upstream reports a `--user-agent` override to a current Chrome also works,
which does not fit a pure-TLS rule — so the real rule is probably composite.
We cannot read it. Do not over-fit; re-run the matrix if this recurs.

## Proof the remedy works end-to-end

Run through the **pinned v0.3.0 binary** (not a hand-built yt-dlp argv — an
earlier hand-reconstructed argv omitted `-f download/b[vcodec=h264]/b`,
selected a 1080p h265 stream, and produced a spurious ffprobe postprocessing
failure; ADR-0038's selector is load-bearing). Isolated scratch state DB and
transcripts tree; campaign DB never opened.

```
claimed=3 succeeded=2 failed=1
terminal (inline) 1 → IpBlockedMessage 1
```

- 2 transcripts: 683 and 2,457 chars of coherent English, language confidence
  `p = 0.981` / `0.9998`.
- Artifacts at the ADR-0004 shard paths (`55/`, `79/`), `.txt` + `.json` pairs.
- **2/2 metadata envelopes captured** (ADR-0042) — the signal whose absence
  was the tell in both incidents.
- Zero `YtDlpOther`, zero `Unsupported URL`, zero `HttpError`.
- The 1 failure is `IpBlockedMessage` = **the video was removed** (ADR-0033;
  the text is a misfire, the IP is fine). Same video returned the same class
  in an independent 5-video probe.

Classification stayed correct across the fix: the terminal class was written
off inline with `requeued_for_retry=0`, so a dead video consumed no retry
budget.

## Current state (2026-08-11)

**Applied on the VM:**

- Run stopped since 2026-08-10 19:21:21 (`state.sqlite` mtime 21:22 local).
- Backup: `~/ddp-state/state.sqlite.bak-2026-08-11` (`sqlite3 .backup`).
- `~/.config/yt-dlp/config` **deleted** (incident-1 mitigation retired).
- **curl_cffi uninstalled** from the pipx yt-dlp venv.
  `yt-dlp --list-impersonate-targets` now reports every target
  `(unavailable)` — that output is the only positive witness that
  impersonation is off.

**NOT applied — the campaign cannot fetch until this is done:**

- The pool still stores incident-1-era **share URLs**
  (`https://www.tiktokv.com/share/video/<id>/`). With impersonation now off,
  those 403 at the first hop (incident-1 gate, re-confirmed live 2026-08-11).
  **Starting `process` in this state fails 100% of fetches.**

### Remaining steps

Rehearsed against a copy of the real snapshot: 1,928,670 rows, 6m49s,
`integrity_check ok`, DB growth 737 KB (0.02%), succeeded/terminal rows
untouched.

```bash
# 1. rewrite the fetch URLs — MUST print 1928670
sqlite3 ~/ddp-state/state.sqlite <<'SQL'
UPDATE videos
   SET source_url = 'https://www.tiktok.com/@x/video/' || video_id || '/'
 WHERE canonical = 1
   AND status IN ('pending','failed_retryable','in_progress')
   AND source_url NOT LIKE 'https://www.tiktok.com/@x/video/%';
SELECT changes() AS rewritten;
SQL

# 2. verify scoping
sqlite3 -readonly ~/ddp-state/state.sqlite "
SELECT status,
       SUM(source_url LIKE 'https://www.tiktok.com/@x/%') AS rewritten,
       SUM(source_url LIKE 'https://www.tiktokv.com/%')   AS form1,
       COUNT(*) FROM videos GROUP BY status;"

# 3. validation batch — treat as a RATE MEASUREMENT, not pass/fail
CUDA_VISIBLE_DEVICES=0 ddp-transcribe \
    --state-db ~/ddp-state/state.sqlite \
    --transcripts ~/ddp-work/transcripts \
    --whisper-model ~/ddp-work/models/ggml-large-v3-turbo-q5_0.bin \
    process --max-videos 50 --retries 2
```

`changes()` must print exactly **1,928,670** — that count comes from the
2026-08-11 snapshot, so any other number means the DB moved and the census
should be re-run before continuing.

Rollback: restore the `.bak`, `pipx inject yt-dlp curl_cffi`, recreate
`~/.config/yt-dlp/config`.

### Why `@x` and not `@`

`@/video/<id>` (empty username) fetches fine but does **not** match
`CANONICAL_RE` in `src/canonical.rs:25` (`@[^/]+/video` requires a non-empty
segment) — it would classify `Canonical::Invalid`. Nothing re-canonicalises a
stored `source_url` today (`canonicalize_url` is called only at ingest,
`src/ingest.rs:373`, on the DDP's own `entry.link`), so it would be latent
until Plan C's short-link work or any re-validation pass read these rows.
`@x` fetches identically and stays `Canonical::VideoId`. Any non-empty
placeholder works (`@x`, `@_`, `@placeholder` all verified), as does the real
uploader name where known.

### Corpus consequence: two URL forms (operator decision, pending)

`source_url` is a provenance field written into **every transcript JSON
artifact** (ADR-0010, `src/output/artifacts.rs:50`). After the rewrite, new
artifacts carry `@x` URLs while the existing 880,387 carry share URLs.

This is **lossless** — the snapshot census confirmed all 2,982,461 rows are
`canonical = 1`, all form-1, no query strings, every URL containing its own
`video_id`, so either form is exactly reconstructible from the primary key —
and the DDP originals remain in Yoda. But it is researcher-visible and should
be a deliberate decision, not a side effect. The scoped rewrite deliberately
leaves `succeeded` and `failed_terminal` rows alone.

Consequence: `backfill-metadata` (which reads `source_url` on *succeeded*
rows — the rc1 10,235-video cohort) will 403 until either those rows are
rewritten too or the pipeline-side fix lands.

## What this validates, and what it costs

- **ADR-0033, twice in five days.** Both waves fell to retryable catch-alls
  and wrote **zero** terminal rows across 1.8M and 1.4k failures
  respectively. An "HTTP 403 / Forbidden means give up" or "unparseable
  response means dead video" classifier would have destroyed the campaign
  twice.
- **ADR-0038's download-first selector** is what makes the canonical URL
  usable: `download` (h264/aac, watermarked) is offered unimpersonated, and
  pinning it avoids the h265 1080p stream that default selection grabs.
- **The cost is three invisible states.** The campaign's correct behavior now
  depends on (a) a package that must stay *absent*, (b) a URL form living in
  a data column, and (c) a deleted config file. None appears in
  `batch_runs.params_json`, the logged argv, or `--version`. That is the
  argument for the pipeline-side fix being the next thing that happens, not a
  followup: ADR-0013 exists because an unverifiable claim about the GPU
  backend is worthless, and the same reasoning applies to "am I
  impersonating?" and "which URL form am I fetching?".

## Durability — this is a rule we are on the right side of, not a fixed bug

We are not waiting for an upstream parser fix. We are on the currently
permitted side of an actively tuned WAF rule, and upstream reports that
workarounds fail *with some probability* — consistent with a rule carrying
reputation or rate components. Implications:

1. **Run in `--max-videos`-capped batches**, not one unattended multi-day run.
2. **The mass-instant-failure circuit breaker is now the highest-value
   engineering item** on the list (already filed from incident 1). Incident 1
   spent 60 hours retrying into a refusing endpoint at ~8/s — precisely the
   behavior that earns reputation-based blocks. The breaker protects us, not
   just our attempt counters.
3. **Add the hourly census check now** (zero successes, or nonzero
   `YtDlpOther`, in the last hour). Two incidents in a row where detection
   delay cost more than the fault.
4. Fallback if the unimpersonated path is blocked next: upstream reports
   yt-dlp **2026.03.17** working — which is also the version ADR-0033's
   patterns are pinned to, so a downgrade would *close* the version-drift
   FOLLOWUPS entry rather than aggravate it. Untested here.

## Correction to incident 1's record

Incident 1 concluded "non-browser TLS fingerprint ∧ datacenter egress → 403".
That is too broad: on 2026-08-11 plain **`curl`** to the same share URL from
the campaign VM completed the redirect chain with HTTP 200 while plain
**yt-dlp** (python/OpenSSL ClientHello) got 403. The rule discriminates among
non-browser fingerprints — it was rejecting *python's* ClientHello
specifically. Corrected in that document.

Incident 1's probe matrix already recorded `canonical URL, plain → success`.
The impersonation config was adopted instead because it needed no code
change. That trade put the campaign inside the blast radius of a global
anti-bot deployment five days later, while the path it passed over is the one
we ended up at. The lesson for the pending ADR: prefer the remedy that
reduces how unusual you look, even when it costs a release.
