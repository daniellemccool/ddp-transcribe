# Incident 2026-08-06 — campaign-wide 403 wave (TikTok TLS-fingerprint gate)

**Status: RESOLVED 2026-08-09** (mitigation live on the campaign VM; pipeline
run resumed after a clean 50-video validation batch). Written same-day from
the live investigation; all timestamps UTC (DB epoch times). Read with
ADR-0033 (evidence-derived classification — this incident is a textbook
instance), ADR-0035 (cookie scoping — untouched here), and
`docs/operations/cookied-runs.md` §2 (the retry arithmetic the recovery had
to protect).

## Summary

At **2026-08-06 23:34:50** every fetch on the campaign VM began failing
instantly with `HTTP Error 403: Forbidden` on the first webpage request.
The pipeline ran unattended for ~60 hours, failing ~8 claims/second across
4 download workers, and burned one retry attempt on **1,806,618 videos**
before the operator killed it 2026-08-09 ~10:55. Root cause: TikTok's edge
began rejecting **non-browser TLS fingerprints** on `www.tiktokv.com` (the
share-redirect host every DDP-export URL hits first) from datacenter
egresses. Not an IP-reputation block, not an extractor breakage, not
rate-limiting. Zero rows were misclassified, zero cap-exhausted; the whole
wave remains recoverable. Mitigation: a yt-dlp user config on the VM makes
every invocation impersonate a Chrome TLS fingerprint.

## Timeline

| When (UTC) | What |
|---|---|
| 2026-07-29 15:40:34 | Run 5 starts (`retries: 1`, 4 workers, no cookies) — the long campaign run |
| 2026-08-06 23:34:39 | Last inline-terminal classification (ordinary `IpBlockedMessage`) |
| 2026-08-06 23:34:50 | **Last success.** Wave begins the same minute — a cliff, not a ramp |
| 2026-08-07 → 08-08 | 725k / 732k failures per day, all `HttpError`, zero successes |
| 2026-08-09 ~10:55 | Operator kills the run (Ctrl-C) with 17 claims in flight |
| 2026-08-09 | Probe matrix run (below); root cause isolated; config fix applied |
| 2026-08-09 ~11:32 | Validation batch: 50 claims, 45 succeeded, 5 known-class terminal, 0 `HttpError` |

## DB evidence (state.sqlite pulled 2026-08-09)

- Failure signature: `ERROR: [generic] <id>: Unable to download webpage:
  HTTP Error 403: Forbidden` — **1,806,618 of 1,806,619** wave rows
  byte-identical in shape (one stray 400). Classified `HttpError` →
  retryable (correct). Every fetch also logged
  `no metadata envelope captured`: yt-dlp died before extraction, so the
  403 was on the *first* HTTP request.
- Background `HttpError` rate before onset: ~3k/day. Aug 7: 725,189.
  Aug 8: 732,337. Aug 9 (partial): 335,423.
- Pool at stop: pending 1,980,835 (= 133,933 at attempt 0 +
  1,846,902 at attempt 1), succeeded 821,786, failed_terminal 161,123
  (all three probe-confirmed classes: IpBlockedMessage 100,657 /
  10240 49,984 / 10231 10,482 — no new terminal class), failed_retryable
  18,700 (18,461 SensitiveLoginGated + 239 Fetch, all attempt 1 —
  the mop-up cohort, untouched by the wave).
- Terminal counts stopped moving at onset — further confirmation nothing
  got past the first request after 23:34.

## Probe matrix (2026-08-09, all on yt-dlp `--skip-download` of the same
share URL, `https://www.tiktokv.com/share/video/7657241061911186701/`)

| Egress | Client stack | Variant | Result |
|---|---|---|---|
| Campaign VM (145.38.207.45, SURF) | py3.12.3/OpenSSL 3.0.13, yt-dlp 2026.07.04 | plain | **403** (re-verified twice, hours apart) |
| Campaign VM | same | `--impersonate chrome` | success |
| Campaign VM | same | canonical `www.tiktok.com/@/video/<id>` URL, plain | success |
| Old VM (`ddptranscribe`, different SURF IP, idle for weeks) | py3.12.3/OpenSSL 3.0.13, yt-dlp **2026.06.09** | plain | **403** |
| Old VM | same | `--impersonate chrome` | success |
| Desktop (residential NL) | py3.14.6/OpenSSL 3.6.3, yt-dlp 2026.07.04, **no curl_cffi** | plain | success |

## Mechanism

The TikTok extractor **auto-impersonates wherever curl_cffi is installed**
(both VMs have it; the desktop probe's warning — "extractor is attempting
impersonation, but no impersonate target is available" — is what exposed
this). The one request that is *never* impersonated unless asked is the
**generic extractor's first hop to `www.tiktokv.com`**, made with the plain
python/OpenSSL ClientHello. That is exactly the request that started 403ing,
and it is the first request of every pipeline fetch, hence 100% failure.

The matrix pins the rule: non-browser TLS fingerprint ∧ (at least)
datacenter egress → 403 on `www.tiktokv.com`. Two different SURF IPs (one
idle for weeks — rules out per-IP reputation earned by this campaign's
~900k fetches), two yt-dlp versions (rules out a version-keyed extractor
break), residential passes unimpersonated, impersonation clears it from the
blocked IPs. The cliff onset at a precise second mid-run is a rule
deployment signature, not a gradually-earned threshold.

**Open forensic question (does not affect the fix):** whether the rule's
egress condition is "datacenter IP" or whether the newer OpenSSL 3.6
ClientHello simply isn't matched. Discriminator, if ever wanted: an
ubuntu:24.04 container (py3.12/OpenSSL 3.0.13, yt-dlp 2026.07.04) probing
plain from a residential egress.

## Mitigation (live on the campaign VM since 2026-08-09)

```
mkdir -p ~/.config/yt-dlp && echo '--impersonate chrome' > ~/.config/yt-dlp/config
```

yt-dlp reads the user config on every invocation and the pipeline never
passes `--ignore-config`, so this applies to every fetch (including the
generic first hop) with the pinned v0.3.x binary untouched — no release, no
ADR-0043 promotion question. **The file exists only as a hand-applied
change on the VM** — a re-provision silently loses it (and with it, all
fetching); persisting it in the deploy repo is the handoff below. The
proper pipeline-side fix (`--impersonate` in `build_yt_dlp_args`, with its
own ADR) is a FOLLOWUPS entry.

## Handoff → `d3i-infra/researchcloud-ddp-transcribe` (not actioned here)

For the agent working the deploy repo; nothing in that repo has been
changed as part of this incident.

1. **`roles/ytdlp`: install the impersonation config at provisioning.**
   Two tasks after the existing pipx install (same `become_user:
   {{ pipeline_user }}` pattern): create `{{ pipeline_home }}/.config/yt-dlp`
   (mode 0755), then copy a config file to
   `{{ pipeline_home }}/.config/yt-dlp/config` with content
   `--impersonate chrome` (mode 0644). Carry a comment explaining *why*
   (this incident, by name): the pipeline never passes `--ignore-config`,
   deleting the file reverts the VM to 100% fetch failure, and the task
   should be retired when `--impersonate` lands in the pipeline's
   `build_yt_dlp_args` (pipeline FOLLOWUPS) — two sources of one flag is
   drift. Note the interaction with the existing verify task ("Verify
   yt-dlp impersonation targets are available"), which until now checked a
   capability nothing explicitly used.
2. **Run-log persistence** (deploy-side FOLLOWUPS candidate): this outage
   ran 60 h unobserved partly because `run-pipeline.sh` output exists only
   in terminal scrollback (~2 min of buffer at wave rates). Candidate: tee
   run output to a dated file in the template, or document a capture
   ritual. Pairs with the pipeline-side circuit-breaker followup.

Validation batch (`process --max-videos 50 --retries 2`, 2026-08-09 11:32):
45/50 succeeded; 5 failures all inline-terminal to the three known dead
classes (3 IpBlockedMessage / 1 ×10231 / 1 ×10240 — the normal pre-outage
mix); zero `HttpError`; metadata envelopes captured again. Sweep census:
18,700 examined, 18,700 parked, `requeued_for_retry 0`, `kept_capped 0` —
the gated mop-up cohort still sits at attempt 1, so cookied-runs §2's
arithmetic survives the incident. (Observed detail: the 239 `Fetch` rows
*parked* in this cookie-less sweep rather than requeueing as
cookied-runs §1 expected — verify sweep semantics against the v0.3.x
binary before the mop-up rather than trusting either document.)

## Recovery arithmetic (why the restart must carry `--retries 2`)

Run 5 ran with `retries: 1` → claim/requeue cap at attempt 2. The wave left
1.85M rows at attempt 1: under `retries 1`, any genuine transient failure
on the rerun would cap-exhaust with no re-park headroom, stranding rows
behind default-deny `requeue-failures` (ADR-0046). `--retries 2` gives the
wave rows one re-park before capping at 3. Claim order (`attempt_count
ASC`) means the 134k attempt-0 rows go first either way. The operator
killed the run within hours of the attempt-0 pool draining — no row was
claimed twice during the wave (`max(attempt_count) = 1` across the pool).

## Detection gap (the expensive part)

The outage cost nothing terminal but ran **60 hours unobserved** because
the only failure signal was tracing output inside an SSH session (kitty
scrollback, ~2 minutes of buffer at wave rates — same operational-
invisibility class as the tmux ruling recorded in
`docs/followups/production-run.md`, `swept_stale` entry). The census that
would have shown `succeeded: 0` only writes at run end; the run never
ended. Followups filed: a mass-instant-failure circuit breaker
(pipeline-side), and run-log persistence (deploy-side). Until those land,
**the operator is the circuit breaker**: check the run daily; a
`--max-videos`-capped batch cadence also bounds the unattended blast
radius.

## What this incident validates

- **ADR-0033's core claim, again:** the message said "Forbidden", the label
  said `HttpError`, and the cause was a TLS-fingerprint rule that no amount
  of message-reading could reveal — the probe matrix found it in five
  commands. The retryable disposition of `HttpError` was exactly right: the
  entire wave recovered wholesale once the transport was fixed.
- **The classifier's terminal conservatism:** 60 hours of 100% failure
  wrote zero terminal rows. The failure-mode design (cautious retryable
  catch-alls, terminal only with probe evidence) converted a potential
  mass-data-loss event into a bounded attempt-count tax.

## Real-block signature vs `IpBlockedMessage` (the 3 a.m. discriminator)

This incident is the first census-scale observation of TikTok *genuinely
refusing to serve us*, and it settles what that looks like — by contrast
with the `IpBlockedMessage` misfire ADR-0033 has guarded against since May.
The two signatures are structurally disjoint:

| | Real egress-level rejection (observed 2026-08-06..09) | `IpBlockedMessage` ("Your IP address is blocked") |
|---|---|---|
| Scope | **every** video, uniformly | per-video, interleaved with successes |
| Success rate | pinned at zero from one second to the next | normal (~90%) around it |
| Where it dies | first HTTP request, pre-extraction (`[generic] Unable to download webpage`) | inside normal extraction — the connection works |
| Metadata envelope | never captured (`no metadata envelope captured` on every fetch) | captured or not per the usual rules |
| Error layer | transport (HTTP status: 403) | application (TikTok's per-video response body) |
| Classification | `HttpError`, retryable | `IpBlockedMessage`, terminal |
| Terminal writes | stop entirely at onset | continue at the normal attrition rate |

The negative result is the strongest part: in 60 hours and 1,806,618 real
rejections, the text "Your IP address is blocked" appeared **zero times** —
while during *healthy* operation that text accumulated 100,657 terminal
rows from the very IP that produced 821,786 successes. When TikTok actually
blocks a client it says nothing about IPs; when it says "your IP is
blocked" it is serving a per-video removal response over a working
connection.

Precision for the next reader: what was observed here is egress-level
**client rejection keyed on TLS fingerprint**, not an IP-reputation block —
the weeks-idle second SURF IP was equally affected, and impersonation
cleared it without changing IP. A true IP-reputation block (still
unobserved) should present the same first-hop uniform signature but split
the probe matrix differently: `--impersonate chrome` would **not** clear
it, and an identical client stack from a different egress **would** pass.
Run the matrix; don't guess from the message.
