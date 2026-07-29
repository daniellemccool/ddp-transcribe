# Capacity estimate — 2026-07-29 snapshot (WORKNOTE — hold)

**Status: HOLD.** Not PI-facing yet. Operator decision 2026-07-29: re-measure
after one GPU instance has run `--download-workers 4` for a while (A/B
against the other instance at the default 3), then update the numbers and
write the PI summary. Everything below is measured from the verified
run-boundary snapshot (`schema_version = 6`, events through 07:53 UTC
2026-07-29) plus live counts the operator read mid-morning.

## Measured throughput (2×A10, 3 download-workers each)

| Window | Claims/h (combined) | Successes/h | Mix |
|---|---|---|---|
| Jul 28 18:00–21:00 UTC (pre-upgrade) | ~3,250 | ~2,650 | retryables still mislabeled (see below) |
| Jul 29 06:00–07:53 UTC (v0.3.0, uncapped, sustained) | 3,360 | 2,735 | 81.4% succeed / 15.0% terminal |
| Jul 29 morning live delta (operator-read counts) | ~4,200 | ~80% of drain | 17.8% terminal |

- Working planning number: **~3,300–4,000 claims drained/h ≈ 79–96K/day** at 24/7.
- Terminal-share drift 15.0% → 17.8% is **classification reallocation, not
  throttling** (operator-confirmed 2026-07-29): errors previously labeled
  "IP blocked" now correctly class as video-removed → `failed_terminal`.
  Consequence: transcript **yield ≈ 80–82%** of any window's videos; the
  rest no longer exist on the platform. Frame corpus completeness as
  "every still-existing video."
- Bottleneck is **fetch, not GPU**: median claim→success 7 s (avg 8.9 s),
  ~1 s of it transcription (bake) — 3 workers ÷ ~6.4 s fetch = one video
  per ~2.1 s/instance = the observed 1,680/h/instance. Transcribe-bound
  ceiling ≈ 3,600/h/instance (~2.1× current). Hence the worker A/B.
- Event-log gap 22:00→05:00 (overnight stop) ≈ 23K videos/night of idle
  capacity; unattended overnight running is the free ~50%/day win.

## PI decision table (workload measured per candidate window)

Distinct videos with ≥1 watch in window (4.58M in-window watch rows total;
99.2% of watches fall in 2026-01→07 — the DDP export horizon — so a
"6-month window" is a false economy, saving only 11%):

| Analysis window | Unique videos | Still pending @snapshot | Runtime @24/7 @3,300/h | @~85% uptime |
|---|---|---|---|---|
| Full corpus (12 mo) | 2,982,461 | 2,964,110 | 37 d | ~44 d |
| ≥ 2026-02 (6 mo) | 2,658,071 | 2,639,720 | 33 d | ~39 d |
| ≥ 2026-04 (4 mo) | 1,684,594 | 1,669,374 | 21 d | ~25 d |
| ≥ 2026-05 (3 mo) | 1,201,154 | 1,189,917 | 15 d | ~18 d |
| ≥ 2026-06 (2 mo) | 721,907 | 717,175 | 9 d | ~11 d |

Levers stack: overnight running + workers at 4–5 puts even the FULL corpus
at ~2–2.5 weeks, which weakens the case for narrowing — present both to the
PI. Narrowing is a scheduling decision, not data destruction: `process`
drains whatever is pending, so a narrow window now + opportunistic tail
later is valid.

## Disk (non-issue)

- VM boot disk at measurement: 96G total, 20G used (21%), 77G free.
- Artifacts measured at ~11.6 KB/video (347 MB / 15.4K videos local) →
  ~34 GB full corpus. DB projects ~4–5 GB with all envelopes (consistent
  with the 3–6 GB followup estimate).

## Re-run recipe (next snapshot)

Verify `SELECT * FROM meta;` says `schema_version|6` first (stale-relay
lesson — the repo-root `ddp-run-export.sqlite` is a stale v3 pilot export;
the good snapshots land under
`~/data/d3i/uu_tiktok/research-tiktok-crime-policing/state-snapshot.sqlite`).

```sql
-- per-hour throughput + mix
SELECT strftime('%m-%d %Hh', at, 'unixepoch') hour,
       SUM(event_type='claimed') claimed,
       SUM(event_type='succeeded') succeeded,
       SUM(event_type='failed_terminal') term,
       SUM(event_type='failed_retryable') retry,
       SUM(event_type='swept_terminal') swept
FROM video_events GROUP BY 1 ORDER BY 1;

-- claim→success latency (bottleneck check); adjust the cutoff
WITH pairs AS (
  SELECT s.at - c.at AS lat
  FROM video_events c JOIN video_events s
    ON s.video_id = c.video_id AND s.event_type='succeeded' AND c.event_type='claimed'
  WHERE c.at >= strftime('%s','2026-07-29 06:00:00') AND s.at > c.at)
SELECT COUNT(*) n, ROUND(AVG(lat),1) avg_s, MIN(lat) mn, MAX(lat) mx FROM pairs;

-- PI window table
WITH w(label, cutoff) AS (VALUES
  ('full corpus', 0),
  ('>= 2026-02 (6 mo)', strftime('%s','2026-02-01')),
  ('>= 2026-04 (4 mo)', strftime('%s','2026-04-01')),
  ('>= 2026-05 (3 mo)', strftime('%s','2026-05-01')),
  ('>= 2026-06 (2 mo)', strftime('%s','2026-06-01')))
SELECT w.label,
       COUNT(DISTINCT wh.video_id) uniq_videos,
       COUNT(DISTINCT CASE WHEN v.status='pending' THEN wh.video_id END) still_pending
FROM w JOIN watch_history wh ON wh.watched_at >= w.cutoff
JOIN videos v ON v.video_id = wh.video_id
GROUP BY w.label ORDER BY uniq_videos DESC;
```

**A/B readout for the worker experiment:** per-instance split needs the
`worker_id` pid clusters (both instances report `worker_host: "host"`);
compare the two `batch_runs` rows' event rates over the same wall-clock
window, or simpler, restart boundaries partition the tally.
