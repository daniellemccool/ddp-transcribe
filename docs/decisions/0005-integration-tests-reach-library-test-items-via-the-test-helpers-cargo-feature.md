---
status: accepted
date: "2026-04-16"
category: Code conventions
applies_to:
    - Cargo.toml
    - tests/**/*.rs
priority: default
---

# Integration tests reach library test items via the test-helpers Cargo feature

## Decision

Library items that exist only for tests are gated
`#[cfg(any(test, feature = "test-helpers"))]`, and an integration-test file
that consumes any gated item opts in whole-file with
`required-features = ["test-helpers"]` in its Cargo.toml `[[test]]` block.
Test helpers never appear in the un-featured public API.

## Guidance

- A new file under `tests/` that touches a gated helper adds its own `[[test]]` block with `required-features = ["test-helpers"]` — whole-file opt-in, never per-feature subsets within one file. Tests that use only the public API (e.g. `e2e_real_tools`) don't opt in.
- Production code must never call a test-helpers-gated item (the mutator-return-value convention exists precisely because that once happened); the helpers are for assertions, not for application logic.
- `#[cfg(test)]` alone cannot work here: integration tests are separate compilation units, so `cfg(test)` in the library is false when they build against it.
- Run the suite as `cargo test --features test-helpers` (see CLAUDE.md for the full verification line).

## Why

Downstream consumers of the library must not see assertion helpers in the
API, while integration tests — which exercise the real `Store::open` path,
PRAGMAs, FK enforcement, and the API surface as consumers see it — still
need them. The feature flag is the standard Cargo idiom that satisfies both
with zero new dependencies.

## Alternatives

- **A test-helpers sub-crate** — workspace cost unjustified at this scale; revisit if fixtures grow into a shared library.
- **`pub(crate)` + `#[cfg(test)]` re-export** — unreachable from `tests/` (separate compilation units).
- **Inline raw rusqlite queries per test** — leaks the schema into every test; assertion shapes drift from the implementation.
