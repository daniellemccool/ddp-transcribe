---
status: accepted
date: "2026-04-16"
category: Code conventions
applies_to:
    - src/**/*.rs
priority: default
---

# Dead code is suppressed with #[allow(dead_code)] plus a justification comment

## Decision

Scaffolding items not yet consumed by the binary are suppressed with
`#[allow(dead_code)]` carrying a mandatory justification comment naming what
will consume them; the task that lands the consumer removes the stale allow
in the same change.

## Guidance

- Every `#[allow(dead_code)]` carries a comment saying why it exists and what removes it; review rejects bare allows, and rejects a change that starts consuming an item without deleting its now-stale allow.
- Don't switch to `#[expect(dead_code)]`: in this bin+lib structure, pub library items are exempt from the lint, so the expectation is unfulfilled in the lib compilation and `unfulfilled_lint_expectations` is fatal under `-D warnings` (empirically tested).
- The feature-unification wrinkle: building with `--features test-helpers` pulls cfg-gated helpers into the bin compilation where they're never called, firing dead_code — those allows name that dynamic in their comment.
- Periodic audit backstop: `rg 'allow\(dead_code\)' src/`.

## Why

The build stays green at `-D warnings` while per-task scaffolding lands ahead
of its consumers, and the justification-comment + removal discipline is what
keeps the allows from accumulating and masking genuine dead-code regressions.

## Context

A thin-binary/fat-library restructuring (which would eliminate the
suppression need entirely) was deliberately deferred at Plan A; it remains
the structural fix if the allow census ever stops shrinking on its own.

## Alternatives

- **`#[expect(dead_code)]`** — tested; fatal under `-D warnings` in this bin+lib shape (see Guidance).
- **Drop duplicate `mod` decls from main.rs** — works but leaves an inconsistent import pattern and keeps the duplicate-types footgun.
- **Thin-bin/fat-lib restructure now** — most sound, most invasive; deferred by design.
