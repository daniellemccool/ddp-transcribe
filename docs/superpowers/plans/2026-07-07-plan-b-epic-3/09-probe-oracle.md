# Task 09: Probe oracle — `ProbeOracle` trait + `CurlProber` via `process::run`

**Files:**
- Create: `src/probe.rs`
- Modify: module declarations (mirror how `src/failure.rs` was registered in Task 03)

**Interfaces:**
- Consumes: `process::{run, CommandSpec}` (Task 02's `CommandOutcome.signal` shape); tokio.
- Produces (Task 10 depends on these exact names):
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum ProbeVerdict { Alive, Dead, Unreachable(String) }`
  - `#[async_trait] pub trait ProbeOracle: Send + Sync { async fn probe(&self, video_id: &str) -> ProbeVerdict; }`
  - `pub struct CurlProber { pub timeout: Duration }` implementing it
  - `pub fn oembed_url(video_id: &str) -> String`
  - `pub fn verdict_from_http_code(code: &str) -> ProbeVerdict` (pure; unit-tested)

Transport is the system `curl` binary through the bounded subprocess infra — no reqwest (ADR 0034; keeps TLS/dep surface at zero and reuses 0021's capture + timeout semantics). Probes never run inside `process`; only `triage` constructs a `CurlProber`.

- [ ] **Step 1: Write the failing unit tests** (in-module; no network)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oembed_url_embeds_canonical_video_url() {
        assert_eq!(
            oembed_url("7411223393468566802"),
            "https://www.tiktok.com/oembed?url=https://www.tiktok.com/@x/video/7411223393468566802"
        );
    }

    #[test]
    fn verdicts_map_from_http_codes() {
        // Empirical basis (2026-07-06/07, n=36, perfect separation): oEmbed
        // returns 200 for live videos (incl. login-gated sensitive ones) and
        // 400 for deleted/private. 404 treated as dead defensively (same
        // resource-absent semantics). Everything else — 5xx, 429, curl
        // failure sentinel — is Unreachable: triage leaves the row untouched.
        assert_eq!(verdict_from_http_code("200"), ProbeVerdict::Alive);
        assert_eq!(verdict_from_http_code("400"), ProbeVerdict::Dead);
        assert_eq!(verdict_from_http_code("404"), ProbeVerdict::Dead);
        assert!(matches!(verdict_from_http_code("429"), ProbeVerdict::Unreachable(_)));
        assert!(matches!(verdict_from_http_code("500"), ProbeVerdict::Unreachable(_)));
        assert!(matches!(verdict_from_http_code(""), ProbeVerdict::Unreachable(_)));
        assert!(matches!(verdict_from_http_code("garbage"), ProbeVerdict::Unreachable(_)));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers probe:: --lib -- --test-threads=1`
Expected: compile failure — module absent.

- [ ] **Step 3: Implement `src/probe.rs`**

```rust
//! Liveness oracle for triage (ADR 0034). One GET against TikTok's oEmbed
//! endpoint per video; the HTTP status alone separates dead from alive
//! (validated 2026-07-06/07 against the 65k-run failure corpus). Transport
//! is the system `curl` binary via the bounded process::run infra — the
//! pipeline hot path never touches this module.

use std::time::Duration;

use async_trait::async_trait;

use crate::process::{run, CommandSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeVerdict {
    Alive,
    Dead,
    /// Probe could not produce a verdict (network failure, curl missing,
    /// unexpected status). Triage leaves the row untouched — default-cautious.
    Unreachable(String),
}

#[async_trait]
pub trait ProbeOracle: Send + Sync {
    async fn probe(&self, video_id: &str) -> ProbeVerdict;
}

pub fn oembed_url(video_id: &str) -> String {
    // The @x username placeholder is accepted by oEmbed; verdicts key on the
    // video id. Same URL form used in the 2026-07-06/07 validation.
    format!("https://www.tiktok.com/oembed?url=https://www.tiktok.com/@x/video/{video_id}")
}

pub fn verdict_from_http_code(code: &str) -> ProbeVerdict {
    match code {
        "200" => ProbeVerdict::Alive,
        "400" | "404" => ProbeVerdict::Dead,
        other => ProbeVerdict::Unreachable(format!("unexpected http code {other:?}")),
    }
}

pub struct CurlProber {
    pub timeout: Duration,
}

#[async_trait]
impl ProbeOracle for CurlProber {
    async fn probe(&self, video_id: &str) -> ProbeVerdict {
        let args: Vec<String> = vec![
            "-sS".into(),
            "-o".into(),
            "/dev/null".into(),
            "-w".into(),
            "%{http_code}".into(),
            "--max-time".into(),
            self.timeout.as_secs().max(1).to_string(),
            oembed_url(video_id),
        ];
        let outcome = match run(CommandSpec {
            program: "curl",
            args,
            // process::run's own timeout backstops curl's --max-time.
            timeout: self.timeout + Duration::from_secs(5),
            stderr_capture_bytes: 1024,
            stdout_capture_bytes: 16, // "%{http_code}" is 3 bytes
            redact_arg_indices: &[],
        })
        .await
        {
            Ok(o) => o,
            Err(e) => return ProbeVerdict::Unreachable(format!("curl run error: {e}")),
        };
        if outcome.exit_code != 0 {
            return ProbeVerdict::Unreachable(format!(
                "curl exit {}: {}",
                outcome.exit_code, outcome.stderr_excerpt
            ));
        }
        let code = outcome
            .stdout
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).trim().to_string())
            .unwrap_or_default();
        verdict_from_http_code(&code)
    }
}
```

Dead-code note (0002): consumed by Task 10; suppress with a pointer comment if the interim build flags it, lift in Task 10.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --features test-helpers probe:: --lib -- --test-threads=1`, then full suite + clippy.
Expected: PASS.

- [ ] **Step 5: Optional manual smoke (operator machine only, not CI/test suite)**

```bash
cargo run -- --help >/dev/null && echo 'build ok'
# one real probe against a known-dead id from the corpus:
curl -sS -o /dev/null -w '%{http_code}\n' --max-time 15 \
  'https://www.tiktok.com/oembed?url=https://www.tiktok.com/@x/video/7636789808341323039'
# expected: 400
```

- [ ] **Step 6: Commit**

```bash
git add src/probe.rs src/lib.rs src/main.rs
git commit -m "feat(probe): oEmbed liveness oracle via curl subprocess (ADR 0034)"
```
