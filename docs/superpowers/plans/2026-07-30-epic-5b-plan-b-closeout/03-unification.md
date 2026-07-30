# Task 03: the unification — single module root, `commands::dispatch`, thin main (Phase 1)

**Files:**
- Modify: `src/lib.rs` (declare the five bin-only modules; add the façade re-exports; new `pub mod commands`)
- Create: `src/commands.rs` (dispatch + `CommandExit`; receives main's `match` arms verbatim)
- Modify: `src/main.rs` (shrinks to parse/tracing/dispatch/exit)
- Modify: `src/cli.rs` (`Cli` fields → `pub(crate)`; `log_format()` accessor; `LogFormat` derives `Copy`)
- Modify: `Cargo.toml` (`[profile.release] lto = "thin"`)

**Interfaces:**
- Consumes: the Task-02 ADR (title: thin bin / fat lib crate shape).
- Produces (later tasks rely on these exact items):
  - crate root: `pub use cli::{Cli, LogFormat};` and `pub use commands::{dispatch, CommandExit};`
  - `pub async fn dispatch(cli: Cli) -> anyhow::Result<CommandExit>` in `src/commands.rs`
  - `pub enum CommandExit { Success, NoClaims, VerifyFailed }` with `pub fn code(&self) -> i32` → 0 / 3 / 1
  - `impl Cli { pub fn log_format(&self) -> LogFormat }`
  - `hostname_or_default()`, `init_tracing`-adjacent helpers move from main.rs into lib modules (`commands` or the module that owns their consumers) — Task 06 uses `hostname_or_default` from its lib location.

**Semantics (binding):**
- **Behavior-preserving.** No logic edits inside the moved `match` arms beyond the mechanical `std::process::exit(3)`/`exit(1)` → `return Ok(CommandExit::NoClaims)`/`Ok(CommandExit::VerifyFailed)` rewrites. The library never calls `process::exit`.
- `main()` becomes (shape, adapt imports):

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = ddp_transcribe::Cli::parse();
    init_tracing(cli.log_format());
    let exit = ddp_transcribe::dispatch(cli).await?;
    std::process::exit(exit.code());
}
```

  (If `init_tracing` moves into the lib, main calls it through the façade — add it to the façade re-exports and disclose per ADR-0003; the ADR's "no root pub without rationale" applies.)
- `src/cli.rs`: `pub struct Cli { pub(crate) global: GlobalArgs, pub(crate) command: Command }` + `impl Cli { pub fn log_format(&self) -> LogFormat { self.global.log_format } }`; add `Copy` to `LogFormat`'s derives. `GlobalArgs`/`Command` themselves become `pub(crate)`.
- `src/lib.rs` gains `pub(crate) mod backfill; pub(crate) mod cli; …` — visibility per module chosen minimally (`pub mod cli` only as far as the façade re-export requires; prefer `mod x;` + selective `pub use`). Full `pub(crate)`-narrowing of item visibility inside modules is Task 04, not here — this task only must compile cleanly with the duplicate `mod` declarations gone.
- `run_serial` is retained untouched.
- `[profile.release] lto = "thin"` added; `cargo build --release` joins this task's gate.

- [ ] **Step 1: Record the census baseline** — `cargo test --features test-helpers -- --list | grep -c ': test$'` (expect 355 = 345 runnable + 10 ignored; record actual).
- [ ] **Step 2: Move the modules.** Delete all 18 `mod` decls from `src/main.rs`; declare the five bin-only modules in `src/lib.rs`; create `src/commands.rs` and move `main`'s `match cli.command { … }` arms into `pub async fn dispatch(cli: Cli) -> Result<CommandExit>` verbatim, rewriting the two `exit` sites to `CommandExit` returns and the tail to `Ok(CommandExit::Success)`. Move `hostname_or_default`, `log_resolved_config`, and other main-resident helpers into `commands.rs` (or their consumer's module) unchanged.
- [ ] **Step 3: The façade.** Crate-root re-exports as specified; `Cli` field visibility + `log_format()` accessor + `LogFormat: Copy`; fix every `cli.global.…` reference inside the crate (now legal via `pub(crate)`).
- [ ] **Step 4: Add `[profile.release] lto = "thin"` to Cargo.toml.**
- [ ] **Step 5: Compile + suite.** `cargo build && cargo build --release` then the full gate. Expect ZERO test-content changes; integration tests compile against the same `ddp_transcribe::` paths (fix any `tests/` imports the moved items require — imports only, never assertions; disclose each per ADR-0003).
- [ ] **Step 6: Census evidence.** `cargo test --features test-helpers -- --list | grep -c ': test$'` — expect **271** (261 runnable + 10 ignored; the 84 duplicated library unit tests now run once). Record before/after counts in the commit body. If the delta differs, STOP and explain the difference before committing (a lost test is a task failure, not a rounding error).
- [ ] **Step 7: Commit**

```bash
git add src/ Cargo.toml Cargo.lock tests/
git commit -m "refactor(crate): single lib module root, thin main via commands::dispatch/CommandExit; lto=thin — 84 duplicated inline tests now run once (census 345->261 runnable)"
```
