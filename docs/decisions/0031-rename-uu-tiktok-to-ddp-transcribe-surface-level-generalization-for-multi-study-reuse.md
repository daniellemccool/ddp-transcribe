---
status: accepted
date: "2026-06-11"
comments:
    - author: Danielle McCool
      date: "2026-06-11 12:53:07"
      text: marked decision as decided
---

# Rename uu-tiktok to ddp-transcribe: surface-level generalization for multi-study reuse

## Context and Problem Statement

The pipeline was named for a single study (uu-tiktok), but the generic use-case is a video-transcription pipeline for data-donation research, deployed via a SURF Research Cloud catalog item and eventually migrated to the d3i-infra org. The study-specific name would otherwise be baked into the catalog item, the Ansible component repo, the installed binary, and every operator-facing env var. How far should the generalization go now, given TikTok remains the only implemented source?

## Decision Drivers

* The SRC catalog item (researchcloud-ddp-transcribe) should provision a coherently named artifact, not a binary named for one study.
* Renaming ingest/fetcher modules or building a source abstraction now would falsely imply multi-source support that does not exist.
* Historical docs (ADRs, plans, bake notes) are dated records; bulk-renaming them rewrites history for zero operational benefit.
* Exactly one known operator has UU_TIKTOK_* in any environment, so a clean env-prefix break is cheap now and gets more expensive later.

## Considered Options

* Surface-level rename: repo, crate/binary, clap name, and env prefix UU_TIKTOK_* to DDP_TRANSCRIBE_*; TikTok-specific module names and all historical docs untouched
* Repo rename only, keeping crate/binary/env names
* Full pluggable-source refactor alongside the rename

## Decision Outcome

Chosen option: "Surface-level rename: repo, crate/binary, clap name, and env prefix UU_TIKTOK_* to DDP_TRANSCRIBE_*; TikTok-specific module names and all historical docs untouched", because coherent operator-facing naming for the catalog item at mechanical cost; deeper refactor deferred until a second source actually exists.

## Comments

* **2026-06-11 12:53:07 — @Danielle McCool:** marked decision as decided
