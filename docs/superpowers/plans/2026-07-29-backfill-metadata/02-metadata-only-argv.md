# Task 02: Metadata-only argv builder + helper visibility

**Files:**
- Modify: `src/fetcher/ytdlp.rs` only

**Interfaces:**
- Consumes (existing in this file): `pub(crate) const METADATA_PRINT_TEMPLATE: &str` (~line 39); private `fn build_metadata_envelope(stdout: Option<&[u8]>, capture_cap: usize) -> Option<String>` (~line 177); function-local `const STDOUT_CAP: usize = 64 * 1024;` inside `acquire` (~line 227).
- Produces (Task 03 relies on these exact names, reached as `crate::fetcher::ytdlp::…` from the bin crate):
  - `pub(crate) const STDOUT_CAP: usize = 64 * 1024;` at module scope
  - `pub(crate) fn build_metadata_envelope(stdout: Option<&[u8]>, capture_cap: usize) -> Option<String>` (visibility widened; body unchanged)
  - `pub(crate) fn build_metadata_only_args(source_url: &str) -> Vec<String>`

**Semantics (binding):**
- `build_metadata_only_args` emits exactly, in order: `--no-playlist`, `--no-warnings`, `--quiet`, `--skip-download`, `--no-simulate`, `--print`, `METADATA_PRINT_TEMPLATE`, `<source_url>`. `--skip-download` suppresses the media transfer; `--no-simulate` keeps `--print` from downgrading the run to simulation (same reason `acquire` passes it). No media selectors, no output template, no postprocessor args.
- **Never a cookies parameter** (ADR-0035: cookies ride only `SensitiveLoginGated` retries; the backfill cohort is succeeded videos). Review rejects a `cookies` argument on this function.
- **No subtitle flags** and the template must not name `subtitles`/`automatic_captions` (ADR-0042 — there is an existing unit assertion for the template; the new argv gets its own).
- Hoisting `STDOUT_CAP` moves the existing constant to module scope with its doc comment; `acquire` uses the module const (delete the local one). No behavior change to `acquire`.
- Widening `build_metadata_envelope` to `pub(crate)` changes visibility only — body and its existing unit tests untouched.

- [ ] **Step 1: Write the failing unit tests**

In `src/fetcher/ytdlp.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn metadata_only_args_exact_shape() {
        let args = build_metadata_only_args("https://www.tiktok.com/@u/video/123");
        assert_eq!(
            args,
            vec![
                "--no-playlist".to_string(),
                "--no-warnings".to_string(),
                "--quiet".to_string(),
                "--skip-download".to_string(),
                "--no-simulate".to_string(),
                "--print".to_string(),
                METADATA_PRINT_TEMPLATE.to_string(),
                "https://www.tiktok.com/@u/video/123".to_string(),
            ]
        );
    }

    #[test]
    fn metadata_only_args_never_download_subtitle_or_cookie_flags() {
        let args = build_metadata_only_args("https://example/v");
        for forbidden in [
            "-x", "-f", "-S", "-o", "--postprocessor-args", "--audio-format",
            "--cookies",
            "--write-subs", "--write-auto-subs", "--sub-langs", "--list-subs",
        ] {
            assert!(
                !args.iter().any(|a| a == forbidden),
                "metadata-only argv must not contain {forbidden}"
            );
        }
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test metadata_only_args -- --test-threads=1`
Expected: COMPILE FAIL (`build_metadata_only_args` absent).

- [ ] **Step 3: Implement**

1. Move `STDOUT_CAP` to module scope (place near `METADATA_PRINT_TEMPLATE`), carrying its rationale:

```rust
/// Stdout capture cap for yt-dlp invocations that print the metadata
/// line (~615 B/video measured live 2026-07-28; at-cap means the head
/// was dropped, so the envelope builder treats a full buffer as
/// unparseable). Shared by `acquire` and the backfill-metadata path.
pub(crate) const STDOUT_CAP: usize = 64 * 1024;
```

Delete the local `const STDOUT_CAP` inside `acquire`; the call site keeps using the name unchanged.

2. Change `fn build_metadata_envelope(` to `pub(crate) fn build_metadata_envelope(` — nothing else.

3. Add, near `build_yt_dlp_args`:

```rust
/// Argv for a metadata-only invocation (backfill-metadata): print the
/// metadata line, transfer no media. `--skip-download` suppresses the
/// transfer; `--no-simulate` keeps `--print` from downgrading the run
/// to simulation (same as the fetch argv). Deliberately takes no
/// cookies parameter — the backfill cohort is succeeded videos, and
/// cookies ride only SensitiveLoginGated retries (ADR-0035).
pub(crate) fn build_metadata_only_args(source_url: &str) -> Vec<String> {
    vec![
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        "--quiet".to_string(),
        "--skip-download".to_string(),
        "--no-simulate".to_string(),
        "--print".to_string(),
        METADATA_PRINT_TEMPLATE.to_string(),
        source_url.to_string(),
    ]
}
```

If clippy flags the new `pub(crate)` items as dead code before Task 03 consumes them (the lib crate root never references them; the bin crate does only from Task 03 on), add `#[allow(dead_code)]` with the ADR-0002 justification comment naming Task 03 as the consumer, and note in the report that Task 03 must remove it.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib fetcher -- --test-threads=1` (or the module's test filter the file's existing tests use)
Expected: the 2 new tests pass alongside all existing ytdlp tests.

- [ ] **Step 5: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green, suite total = Task 01's total + 2.

- [ ] **Step 6: Commit**

```bash
git add src/fetcher/ytdlp.rs
git commit -m "feat(fetcher): metadata-only yt-dlp argv builder; hoist STDOUT_CAP and widen envelope builder for backfill"
```
