---
status: accepted
date: "2026-07-30"
source: docs/superpowers/specs/2026-07-30-epic-5b-plan-b-closeout-design.md
category: Code conventions
applies_to:
    - src/main.rs
    - src/lib.rs
    - Cargo.toml
priority: invariant
checks:
    - desc: main.rs declares no modules — lib.rs is the single module root
      grep: '^\s*(pub )?mod '
      in: ["src/main.rs"]
      expect: absent
---

# Keep the binary thin, library fat

## Decision

`src/lib.rs` is the crate's single module root; `src/main.rs` declares no
modules and owns exactly four things — argument parsing, tracing init, error
rendering, and the final `std::process::exit`. Everything else is library
code, and the library never calls `process::exit`.

## Guidance

- The crate root's public façade is exactly `pub use cli::{Cli, LogFormat};` and `pub use commands::{dispatch, CommandExit};`, plus the items `tests/` imports and the `test-helpers`-gated scaffolding (that gating convention is unchanged); every other item is `pub(crate)` or private.
- A new module is declared in `lib.rs`, never in `main.rs` — main carries no `mod` line at all — and main reaches library code only through the façade. Exit semantics travel back to main as a `CommandExit` value; a library function that ends the process itself is a defect.
- `#[warn(unreachable_pub)]` is the backstop against `pub` drifting wider than the façade, and review rejects a new root-level `pub` item that arrives without a façade rationale. The fix is `pub(crate)` or one narrow accessor — `Cli::log_format()` is the pattern, serving main's pre-dispatch tracing init while `Cli`'s fields stay `pub(crate)`.
- `[profile.release] lto = "thin"` in `Cargo.toml` exists so cross-crate inlining across the bin/lib boundary is a non-question; the release build is part of the verification gate, so keep both the profile and the gate green.

## Checks

- `rg -n 'process::exit' src/ --glob '!src/main.rs' | rg -v ':\s*//'` — must return nothing outside comments (not grep-assertable: comments legitimately mention it); the one exit lives in `main`, reached only via a `CommandExit` value.
- `rg -n '^pub use' src/lib.rs` — the façade is exactly `cli::{Cli, LogFormat}` and `commands::{dispatch, CommandExit}`; a new name here needs a façade rationale.

## Why

With one module root every file compiles once — no inline unit tests running
twice, no duplicate-type footgun — and holding the façade to four names is
what keeps the binary boundary from silently widening the crate's public API.
Reachable `pub` items are exempt from `dead_code` in any library, so a narrow
façade policed by `unreachable_pub` is the only thing stopping `pub` from
becoming a place to park unused code.

## Alternatives

- **Keep both module roots and drop main's duplicate `mod` lines** — leaves an inconsistent import pattern and keeps the duplicate-types footgun.
- **Make the library broadly `pub` for the binary's convenience** — a package binary is a separate crate, so every item main touches would become public API; the façade plus `Cli::log_format()` buys the same access for four names.
