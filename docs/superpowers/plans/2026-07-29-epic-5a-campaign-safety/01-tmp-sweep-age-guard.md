# Task 01: tmp-sweep age guard — never delete a live sibling's in-flight tmp

**Files:**
- Modify: `src/output/artifacts.rs` (`cleanup_tmp_files` signature + guard; inline tests)
- Modify: `src/main.rs` (sole call site, ~line 88, passes the threshold)

**Interfaces:**
- Consumes (existing): `cleanup_tmp_files(transcripts_root: &Path) -> Result<usize>` (artifacts.rs:183-211); tmp name shape `{file}.tmp-{pid}-{seq}`; caller `src/main.rs:88` in the Process arm; `cfg.stale_claim_threshold: Duration` (resolved in `Config`, default 30m).
- Produces: `pub fn cleanup_tmp_files(transcripts_root: &Path, older_than: std::time::Duration) -> Result<usize>` — deletes a matching tmp only when its mtime is older than `older_than`.

**Semantics (binding):**
- A tmp file is deleted only if BOTH: name contains `.tmp` (unchanged match) AND `now - mtime > older_than`. Rationale for the threshold choice: a tmp older than the stale-claim window cannot belong to a live claim — the claim itself would have been swept.
- **Unreadable mtime ⇒ skip + `tracing::warn!` + not counted** (destroy-on-uncertainty is forbidden — global constraint). Same for a clock anomaly (mtime in the future ⇒ elapsed is `Err`/zero ⇒ not older ⇒ skipped): the code must treat `SystemTime::elapsed()` errors as "fresh".
- Fresh orphans from a crashed process survive one sweep and are collected by any later start — accepted and documented in the doc comment.
- Counting semantics unchanged: only successful deletions count (964e9c2 behavior).

- [ ] **Step 1: Write the failing tests**

In `src/output/artifacts.rs`'s existing `#[cfg(test)] mod tests`, using `std::fs::FileTimes` (stable 1.75) to age files:

```rust
    fn set_mtime_secs_ago(path: &std::path::Path, secs: u64) {
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(t)).unwrap();
    }

    #[test]
    fn cleanup_spares_fresh_tmp_and_removes_old_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let shard = dir.path().join("ab");
        std::fs::create_dir_all(&shard).unwrap();
        let fresh = shard.join("v1.txt.tmp-1234-0");
        let old = shard.join("v2.txt.tmp-5678-0");
        std::fs::write(&fresh, b"x").unwrap();
        std::fs::write(&old, b"x").unwrap();
        set_mtime_secs_ago(&old, 3600);

        let removed = cleanup_tmp_files(dir.path(), std::time::Duration::from_secs(1800)).unwrap();
        assert_eq!(removed, 1, "only the old tmp is collected");
        assert!(fresh.exists(), "a fresh tmp may belong to a live sibling — spared");
        assert!(!old.exists());
    }

    #[test]
    fn cleanup_with_zero_threshold_keeps_prior_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let shard = dir.path().join("cd");
        std::fs::create_dir_all(&shard).unwrap();
        let tmp = shard.join("v3.json.tmp-1-1");
        std::fs::write(&tmp, b"x").unwrap();
        set_mtime_secs_ago(&tmp, 2);
        let removed = cleanup_tmp_files(dir.path(), std::time::Duration::ZERO).unwrap();
        assert_eq!(removed, 1);
        assert!(!tmp.exists());
    }
```

Also update every existing inline test that calls `cleanup_tmp_files(...)` (`cleanup_tmp_files_counts_only_real_deletions` :264, `cleanup_tmp_removes_tmp_files_in_shard_dirs` :414) to pass `std::time::Duration::ZERO` so they keep asserting the pre-existing delete behavior unchanged.

- [ ] **Step 2: Run to confirm failure** — `cargo test --lib output -- --test-threads=1` (or the crate's module filter): COMPILE FAIL (arity).

- [ ] **Step 3: Implement**

Modify `cleanup_tmp_files` (keep the existing walk/count/warn structure; add the guard where the name matches):

```rust
/// ... extend the existing doc comment with:
/// Only tmps whose mtime is older than `older_than` are deleted: a tmp
/// younger than the stale-claim window may belong to a live sibling
/// process mid-`atomic_write` (two-instance deployment), and deleting it
/// makes that sibling's rename fail — which aborts its whole batch run.
/// Unreadable mtime ⇒ skip + warn (never destroy on uncertainty). Fresh
/// orphans from a crash survive one sweep and are collected next start.
pub fn cleanup_tmp_files(transcripts_root: &Path, older_than: Duration) -> Result<usize> {
```

Guard logic at the deletion site (adapt names to the file's local style):

```rust
            let old_enough = match std::fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(mtime) => match mtime.elapsed() {
                    Ok(age) => age > older_than,
                    // mtime in the future / clock anomaly: treat as fresh.
                    Err(_) => false,
                },
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error,
                        "tmp mtime unreadable; sparing file (never destroy on uncertainty)");
                    false
                }
            };
            if !old_enough {
                continue;
            }
```

`src/main.rs:88` call site becomes:

```rust
        let removed = output::artifacts::cleanup_tmp_files(&cfg.transcripts, cfg.stale_claim_threshold)?;
```

(Match the actual local variable/log shape at the call site; `cfg.stale_claim_threshold` is already a resolved `Duration` — check `src/config.rs` for the field name and adjust if it differs.)

- [ ] **Step 4: Run tests to verify they pass** — the two new tests plus all pre-existing artifacts tests.

- [ ] **Step 5: Full verification**

`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` — green; record total (+2 expected per target; note the bin/lib double-compilation doubles inline-test deltas).

- [ ] **Step 6: Commit**

```bash
git add src/output/artifacts.rs src/main.rs
git commit -m "fix(output): tmp sweep only collects tmps older than the stale-claim threshold — never a live sibling's in-flight write"
```
