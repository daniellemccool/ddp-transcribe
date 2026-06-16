---
status: accepted
date: "2026-06-16"
comments:
    - author: Danielle McCool
      date: "2026-06-16 13:25:09"
      text: marked decision as decided
---

# Transcription hot path on boot disk; storage volume is seed-at-provision and sink-at-downtime

## Context and Problem Statement

The production target is **1,000,000 videos**; the 65k pilot is one batch. The 1M set is processed in **disjoint batches across replicated catalog-item workspaces** (shared-nothing): each workspace owns its batch, its state DB, its volume, and its transcripts. Operators deploy one batch, verify completion, then replicate (two workspaces, then expand), budgeting two weeks to a month+.

The SRC deployment initially pointed `transcripts_root` (`--transcripts`) and the whisper model (`--whisper-model`) at the NFS-like storage volume. Two problems:

1. **Correctness.** ADR 0008's crash-recovery contract relies on `atomic_write` (`src/output/artifacts.rs:128-159`): `.tmp` → `fsync` → `rename` → **fsync of the parent directory**. The parent-directory fsync and rename-durability are exactly the POSIX semantics that NFS-like network mounts implement weakly or unreliably. Writing artifacts to the volume puts the durability guarantee on a filesystem that cannot honor it — the same hazard class that already forced `state.sqlite` onto the boot disk (SQLite WAL).
2. **Throughput.** At ~2 s/video, per-video synchronous writes to the mount are latency-bound and serialize against the hot loop. At 1M videos this adds many hours of pure I/O wait.

## Considered Options

* **Hot path on the boot disk; volume is seed-at-provision and sink-at-downtime.** Model + `transcripts_root` + state DB on the local boot disk; the volume holds inbox, the durable transcript sink, archive, and a state snapshot. An operator-run script syncs to the volume at batch boundaries.
* **Keep transcripts and model on the storage volume (status quo).** Rejected: violates ADR 0008's durability contract on an NFS-like filesystem, and is latency-bound at 2 s/video × 1M.
* **Distributed shared state across workspaces.** Rejected: unnecessary. Batches are disjoint (shared-nothing), so there is no cross-workspace state to coordinate; it would add distributed-systems complexity the workload does not require.

## Decision Outcome

Chosen option: "**Hot path on the boot disk; volume is seed-at-provision and sink-at-downtime.** Model + `transcripts_root` + state DB on the local boot disk; the volume holds inbox, the durable transcript sink, archive, and a state snapshot. An operator-run script syncs to the volume at batch boundaries.", because Only option that preserves ADR 0008 durability (atomic_write on POSIX) and keeps the mount out of the 2s/video loop; shared-nothing batches make distributed state unnecessary..

## Comments

* **2026-06-16 13:25:09 — @Danielle McCool:** marked decision as decided
