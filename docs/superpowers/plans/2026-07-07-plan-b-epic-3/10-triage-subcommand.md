# Task 10: `triage` subcommand — classify → probe → mutate → census

**Files:**
- Create: `src/triage.rs`
- Modify: `src/cli.rs` (new `Triage` variant), `src/main.rs` (dispatch arm)
- Test: new `tests/triage.rs` (+ `Cargo.toml` `[[test]] name = "triage"` with `required-features = ["test-helpers"]`)

**Interfaces:**
- Consumes: Task 03's `classify_message` / `MessageVerdict`; Task 05's `list_failed_retryable` / `triage_mark_terminal` / `requeue_retryable`; Task 09's `ProbeOracle` / `ProbeVerdict` / `CurlProber`.
- Produces:
  - `pub struct TriageOptions { pub dry_run: bool, pub rate_per_sec: f64, pub max_attempts: i64 }`
  - `pub async fn run_triage(store: &mut Store, oracle: &dyn ProbeOracle, opts: &TriageOptions) -> Result<TriageStats>`
  - `pub struct TriageStats` (0007: input-side counters, verb-named) with per-kind census `BTreeMap<String, KindCounts>`; `Display` renders the attrition table.

**Per-row decision procedure (the load-bearing logic):**

1. `classify_message(last_retryable_message)` on the *stored* message.
2. `MessageVerdict::Unavailable(reason)` → **no probe** (write-off ruling; saves ~3.9k probes on the production DB) → `triage_mark_terminal(id, reason.tag(), "triage: message-class write-off")`.
3. `MessageVerdict::Retryable(kind)` → probe:
   - `Dead` → `triage_mark_terminal(id, "ProbeDead", "triage: oEmbed probe returned dead")`.
   - `Alive` → if `attempt_count < max_attempts`: `requeue_retryable(id, kind.tag(), max_attempts)` (kind write-back normalizes historical `"Fetch"` rows); else count as `kept_capped`.
   - `Unreachable(_)` → row untouched, count `kept_unreachable`.
4. `dry_run` → same classification + probing, **zero mutations** (counters prefixed `would_`... no — same counter names; the header line of the census says `DRY RUN`).
5. Sleep `1.0 / rate_per_sec` seconds between probes (not between fast-path rows).

- [ ] **Step 1: Write the failing integration tests** (`tests/triage.rs`; copy clippy-allow header + fixture-store helper pattern from `tests/state_triage.rs`)

```rust
struct FakeOracle {
    verdicts: std::collections::HashMap<String, ddp_transcribe::probe::ProbeVerdict>,
    probed: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ddp_transcribe::probe::ProbeOracle for FakeOracle {
    async fn probe(&self, video_id: &str) -> ddp_transcribe::probe::ProbeVerdict {
        self.probed.lock().unwrap().push(video_id.to_string());
        self.verdicts.get(video_id).cloned()
            .unwrap_or(ddp_transcribe::probe::ProbeVerdict::Unreachable("unconfigured".into()))
    }
}

#[tokio::test]
async fn triage_routes_all_four_verdicts() {
    let (mut store, _tmp) = fresh_store();
    // Row A: stored message is a write-off class → terminal WITHOUT probe.
    seed_failed(&mut store, "7000000000000000021", "Fetch", "ERROR: Your IP address is blocked from accessing this post");
    // Row B: retryable message, probe says Dead → terminal.
    seed_failed(&mut store, "7000000000000000022", "Fetch", "ERROR: Did not get any data blocks");
    // Row C: retryable message, probe says Alive → requeued with normalized kind.
    seed_failed(&mut store, "7000000000000000023", "Fetch", "ERROR: [TikTok] x: This post may not be comfortable for some audiences. Log in for access.");
    // Row D: probe Unreachable → untouched.
    seed_failed(&mut store, "7000000000000000024", "Fetch", "ERROR: Did not get any data blocks");

    let oracle = FakeOracle {
        verdicts: [
            ("7000000000000000022".to_string(), ProbeVerdict::Dead),
            ("7000000000000000023".to_string(), ProbeVerdict::Alive),
            ("7000000000000000024".to_string(), ProbeVerdict::Unreachable("timeout".into())),
        ].into_iter().collect(),
        probed: Default::default(),
    };
    let stats = run_triage(&mut store, &oracle, &TriageOptions {
        dry_run: false, rate_per_sec: 1000.0, max_attempts: 3,
    }).await.unwrap();

    assert_eq!(status_of(&store, "7000000000000000021"), "failed_terminal");
    assert!(!oracle.probed.lock().unwrap().contains(&"7000000000000000021".to_string()),
        "write-off class must not be probed");
    assert_eq!(status_of(&store, "7000000000000000022"), "failed_terminal");
    assert_eq!(status_of(&store, "7000000000000000023"), "pending");
    assert_eq!(kind_of(&store, "7000000000000000023").as_deref(), Some("SensitiveLoginGated"),
        "requeue must normalize the historical placeholder kind");
    assert_eq!(status_of(&store, "7000000000000000024"), "failed_retryable");

    assert_eq!(stats.examined, 4);
    assert_eq!(stats.marked_terminal, 2);
    assert_eq!(stats.requeued, 1);
    assert_eq!(stats.kept_unreachable, 1);
}

#[tokio::test]
async fn triage_dry_run_mutates_nothing() {
    let (mut store, _tmp) = fresh_store();
    seed_failed(&mut store, "7000000000000000025", "Fetch", "ERROR: Your IP address is blocked");
    let oracle = FakeOracle { verdicts: Default::default(), probed: Default::default() };
    let stats = run_triage(&mut store, &oracle, &TriageOptions {
        dry_run: true, rate_per_sec: 1000.0, max_attempts: 3,
    }).await.unwrap();
    assert_eq!(stats.marked_terminal, 1, "dry run still REPORTS the verdict");
    assert_eq!(status_of(&store, "7000000000000000025"), "failed_retryable", "…but mutates nothing");
}

#[tokio::test]
async fn triage_respects_attempt_cap() {
    let (mut store, _tmp) = fresh_store();
    seed_failed(&mut store, "7000000000000000026", "Fetch", "ERROR: Did not get any data blocks"); // attempt_count = 1
    let oracle = FakeOracle {
        verdicts: [("7000000000000000026".to_string(), ProbeVerdict::Alive)].into_iter().collect(),
        probed: Default::default(),
    };
    let stats = run_triage(&mut store, &oracle, &TriageOptions {
        dry_run: false, rate_per_sec: 1000.0, max_attempts: 1,
    }).await.unwrap();
    assert_eq!(stats.kept_capped, 1);
    assert_eq!(status_of(&store, "7000000000000000026"), "failed_retryable");
}
```

Helpers `seed_failed` / `status_of` / `kind_of` mirror `tests/state_triage.rs`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test triage -- --test-threads=1`
Expected: compile failure (module + Cargo target missing).

- [ ] **Step 3: Implement `src/triage.rs`**

```rust
//! Operator triage pass (ADR 0034): the retry executor. Classifies stored
//! failure messages, probes ambiguous rows via the oEmbed oracle, drains
//! dead rows to failed_terminal, requeues recoverable rows under an attempt
//! cap. The census it prints doubles as the study's attrition documentation.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use anyhow::Result;

use crate::failure::{classify_message, MessageVerdict};
use crate::probe::{ProbeOracle, ProbeVerdict};
use crate::state::Store;

pub struct TriageOptions {
    pub dry_run: bool,
    pub rate_per_sec: f64,
    pub max_attempts: i64,
}

#[derive(Debug, Default)]
pub struct KindCounts {
    pub examined: usize,
    pub marked_terminal: usize,
    pub requeued: usize,
    pub kept_unreachable: usize,
    pub kept_capped: usize,
}

/// 0007: input-side counters, verb-named. `examined = marked_terminal +
/// requeued + kept_unreachable + kept_capped` holds by construction.
#[derive(Debug, Default)]
pub struct TriageStats {
    pub examined: usize,
    pub marked_terminal: usize,
    pub requeued: usize,
    pub kept_unreachable: usize,
    pub kept_capped: usize,
    /// Census keyed by normalized kind tag (write-off rows keyed by the
    /// UnavailableReason tag). Attrition table for the paper.
    pub by_kind: BTreeMap<String, KindCounts>,
}

pub async fn run_triage(
    store: &mut Store,
    oracle: &dyn ProbeOracle,
    opts: &TriageOptions,
) -> Result<TriageStats> {
    let rows = store.list_failed_retryable()?;
    let mut stats = TriageStats::default();
    let probe_gap = Duration::from_secs_f64(1.0 / opts.rate_per_sec.max(0.001));

    for row in rows {
        stats.examined += 1;
        let message = row.last_retryable_message.as_deref().unwrap_or("");
        match classify_message(message) {
            MessageVerdict::Unavailable(reason) => {
                let k = stats.by_kind.entry(reason.tag().to_string()).or_default();
                k.examined += 1;
                if !opts.dry_run {
                    store.triage_mark_terminal(
                        &row.video_id,
                        reason.tag(),
                        "triage: message-class write-off",
                    )?;
                }
                stats.marked_terminal += 1;
                k.marked_terminal += 1;
            }
            MessageVerdict::Retryable(kind) => {
                let verdict = oracle.probe(&row.video_id).await;
                tokio::time::sleep(probe_gap).await;
                let k = stats.by_kind.entry(kind.tag().to_string()).or_default();
                k.examined += 1;
                match verdict {
                    ProbeVerdict::Dead => {
                        if !opts.dry_run {
                            store.triage_mark_terminal(
                                &row.video_id,
                                "ProbeDead",
                                "triage: oEmbed probe returned dead",
                            )?;
                        }
                        stats.marked_terminal += 1;
                        k.marked_terminal += 1;
                    }
                    ProbeVerdict::Alive => {
                        if row.attempt_count < opts.max_attempts {
                            if !opts.dry_run {
                                store.requeue_retryable(
                                    &row.video_id,
                                    kind.tag(),
                                    opts.max_attempts,
                                )?;
                            }
                            stats.requeued += 1;
                            k.requeued += 1;
                        } else {
                            stats.kept_capped += 1;
                            k.kept_capped += 1;
                        }
                    }
                    ProbeVerdict::Unreachable(why) => {
                        tracing::warn!(video_id = row.video_id.as_str(), why, "triage: probe unreachable; row untouched");
                        stats.kept_unreachable += 1;
                        k.kept_unreachable += 1;
                    }
                }
            }
        }
    }
    Ok(stats)
}

impl fmt::Display for TriageStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "kind                       examined  terminal  requeued  unreach  capped")?;
        for (kind, c) in &self.by_kind {
            writeln!(
                f,
                "{kind:<26} {:>8} {:>9} {:>9} {:>8} {:>7}",
                c.examined, c.marked_terminal, c.requeued, c.kept_unreachable, c.kept_capped
            )?;
        }
        writeln!(
            f,
            "TOTAL                      {:>8} {:>9} {:>9} {:>8} {:>7}",
            self.examined, self.marked_terminal, self.requeued, self.kept_unreachable, self.kept_capped
        )
    }
}
```

`src/cli.rs`:

```rust
/// Adjudicate failed_retryable rows: write-off classes → failed_terminal;
/// probe the rest via TikTok oEmbed (dead → terminal, alive → pending under
/// the attempt cap). Requires `curl` on PATH. Run `process` afterwards.
Triage {
    /// Probe and report the census without mutating any rows.
    #[arg(long)]
    dry_run: bool,
    /// oEmbed probes per second.
    #[arg(long, default_value_t = 1.0)]
    rate: f64,
    /// Rows at or above this attempt_count are not requeued.
    #[arg(long, default_value_t = 3)]
    max_attempts: i64,
},
```

`src/main.rs` — mirror the existing arm structure (open `Store` the way `Process` does):

```rust
cli::Command::Triage { dry_run, rate, max_attempts } => {
    let mut store = state::Store::open(&cfg.state_db)?;
    let oracle = probe::CurlProber { timeout: std::time::Duration::from_secs(15) };
    let opts = triage::TriageOptions { dry_run, rate_per_sec: rate, max_attempts };
    let stats = rt.block_on(triage::run_triage(&mut store, &oracle, &opts))?; // match main's existing async entry pattern — if main is #[tokio::main], just .await
    if dry_run {
        println!("DRY RUN — no rows were modified");
    }
    print!("{stats}");
    Ok(())
}
```

(Check how `main.rs` actually enters async for `Process` — `#[tokio::main]` vs explicit runtime — and copy that pattern exactly; the snippet's `rt.block_on` is illustrative.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --features test-helpers --test triage -- --test-threads=1`, then full suite + clippy + fmt.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/triage.rs src/cli.rs src/main.rs tests/triage.rs Cargo.toml
git commit -m "feat(triage): operator triage subcommand — message-class write-off, oEmbed probe, capped requeue, attrition census (ADR 0034)"
```
