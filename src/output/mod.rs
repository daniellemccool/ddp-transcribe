pub mod artifacts;

/// Returns the shard segment for a video_id: the last two characters.
/// Snowflake low digits are essentially random, giving uniform 100-bucket
/// distribution. The single source of truth for path layout — no other
/// module hard-codes a path scheme.
pub fn shard(video_id: &str) -> &str {
    let len = video_id.len();
    if len < 2 {
        return video_id;
    }
    &video_id[len - 2..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_uses_last_two_chars() {
        assert_eq!(shard("7234567890123456789"), "89");
        assert_eq!(shard("0000000000000000001"), "01");
    }

    #[test]
    fn shard_handles_short_ids() {
        assert_eq!(shard("7"), "7");
        assert_eq!(shard("12"), "12");
    }

    /// Distribution test — synthesise IDs and verify no shard is wildly under
    /// or over-represented. Catches a regression where someone uses the high
    /// digits (which are time-clustered) instead of the low digits.
    #[test]
    fn shard_distributes_uniformly() {
        use std::collections::HashMap;

        let mut counts: HashMap<String, usize> = HashMap::new();
        // Synthetic IDs: monotonically increasing 19-digit numbers.
        let base: u64 = 7_000_000_000_000_000_000;
        for i in 0..10_000u64 {
            let id = format!("{}", base + i);
            *counts.entry(shard(&id).to_string()).or_default() += 1;
        }

        // 10000 / 100 buckets = 100 mean, and a monotonic counter hits every
        // bucket EXACTLY 100 times — so the ±50% band (50..=150) passes with
        // a 0% margin here and is decorative for this input, not "lenient".
        // Real Snowflake low bits are pseudorandom, so their per-bucket counts
        // are Poisson-like (~10% sd over 10k samples): looser than this input,
        // not tighter. The load-bearing assertion is `counts.len() == 100`
        // below — a high-digits implementation is time-clustered and would
        // collapse to one or two buckets. Swapping this input for a PRNG
        // sample is what would make the band mean something.
        for (bucket, n) in &counts {
            assert!(
                (50..=150).contains(n),
                "bucket {bucket} has {n} items, outside 50..=150"
            );
        }
        assert_eq!(counts.len(), 100, "expected 100 distinct buckets");
    }
}
