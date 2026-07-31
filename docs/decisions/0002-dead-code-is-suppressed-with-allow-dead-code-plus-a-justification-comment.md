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

Genuinely-forward scaffolding — an item that has a named consumer landing in
a later change — is suppressed with `#[allow(dead_code)]` plus a mandatory
justification comment naming that consumer, and the change that lands it
removes the stale allow. Every other dead-code warning is answered by
narrowing visibility, not by an allow.

## Guidance

- Reach for visibility before suppression: with `lib.rs` the single module root, an unused item is `pub(crate)` or private rather than `pub`-plus-allow, and `#[warn(unreachable_pub)]` is the backstop that stops `pub` being used as a dead-code dodge.
- Every surviving `#[allow(dead_code)]` carries a comment saying why it exists and which change removes it; review rejects bare allows, allows with no named consumer, and changes that start consuming an item without deleting its now-stale allow.
- Don't switch to `#[expect(dead_code)]`: deadness here is configuration-dependent — an item consumed only by `test-helpers`-gated code is dead in a plain build and live under `--features test-helpers`, so an expectation fulfilled in one configuration is unfulfilled in the other and `unfulfilled_lint_expectations` is fatal under `-D warnings`.
- Periodic audit backstop: `rg 'allow\(dead_code\)' src/` — the census should shrink, and an allow whose comment still cites double compilation is stale by definition.

## Why

The build stays green at `-D warnings` while per-task scaffolding lands ahead
of its consumers, but now that every file compiles once an allow is no longer
the cheap answer: an unjustified one hides either an item that should have
been `pub(crate)` or a genuine dead-code regression.

## Context

A thin-binary/fat-library restructuring was deliberately deferred at Plan A
and taken in Epic 5b: `lib.rs` is now the single module root, so files no
longer compile twice. Externally reachable `pub` library items remain outside
`dead_code` — the restructure removed the second compilation context, not the
exemption — so visibility discipline, not the lint, is now the only protection
against `pub` being used to hide dead code. The allows that existed only to
absorb the duplicate compilation were removed with the restructure.

## Alternatives

- **`#[expect(dead_code)]`** — rejected; unfulfilled expectations are fatal under `-D warnings`, and two things make them unfulfillable here: a reachable `pub` item never fires the lint at all, and feature gating makes deadness configuration-dependent (see Guidance).
- **Blanket `#[allow(dead_code)]` at the crate root** — silences the lint everywhere and forfeits its regression value.
