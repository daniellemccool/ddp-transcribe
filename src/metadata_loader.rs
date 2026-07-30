//! Post-run metadata loader (Epic 4c): parses `video_metadata_raw`
//! envelopes into typed `videos` columns. Streaming (keyset pages),
//! batched (one tx per page), idempotent, replayable — a parse bug is
//! fixed by re-running, never by re-fetching.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::state::{MetadataColumns, Store};

/// Raw rows pulled per keyset page. The whole table is 3–6 GB at
/// production scale, so the loader never materializes more than a page.
const PAGE_SIZE: usize = 10_000;

/// Loader stats: input-side counters, verb-named (ADR-0007).
#[derive(Debug, Default, Serialize)]
pub(crate) struct LoadStats {
    /// Raw rows examined this pass.
    pub rows_examined: u64,
    /// Rows whose columns were written (dry-run counts them as loadable).
    pub rows_loaded: u64,
    /// Rows skipped because the envelope or its printed line failed to
    /// parse (or carried an unknown schema version). Never fatal.
    pub rows_skipped_unparseable: u64,
    /// Parsed rows whose videos row no longer exists (UPDATE matched 0).
    pub rows_without_video: u64,
}

impl std::fmt::Display for LoadStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "examined {} / loaded {} / skipped-unparseable {} / without-video {}",
            self.rows_examined,
            self.rows_loaded,
            self.rows_skipped_unparseable,
            self.rows_without_video
        )
    }
}

/// The capture-time envelope written by the fetcher (Task 02/03): a
/// version tag plus yt-dlp's `--print` line, stored unparsed.
#[derive(Deserialize)]
struct Envelope {
    schema: u32,
    printed: String,
}

/// The typed subset of yt-dlp's printed fields the loader maps to columns.
/// Everything else in `printed` (`title`, `channel_id`, `duration`,
/// `repost_count`) stays raw-only by design — the print set is
/// deliberately wider than the column set so a later column addition is a
/// re-run, not a re-fetch.
#[derive(Deserialize)]
struct PrintedFields {
    description: Option<String>,
    uploader: Option<String>,
    uploader_id: Option<String>,
    timestamp: Option<i64>,
    view_count: Option<i64>,
    like_count: Option<i64>,
    comment_count: Option<i64>,
}

/// Parse one envelope into column values. `None` = unparseable (caller
/// counts + warns; never fatal per the epic invariant).
fn parse_envelope(video_id: &str, fetched_at: i64, raw_json: &str) -> Option<MetadataColumns> {
    let env: Envelope = serde_json::from_str(raw_json).ok()?;
    if env.schema != 1 {
        return None;
    }
    let printed: PrintedFields = serde_json::from_str(&env.printed).ok()?;
    Some(MetadataColumns {
        video_id: video_id.to_string(),
        video_description: printed.description,
        uploader: printed.uploader,
        uploader_id: printed.uploader_id,
        video_created_at: printed.timestamp,
        view_count: printed.view_count,
        like_count: printed.like_count,
        comment_count: printed.comment_count,
        metadata_fetched_at: fetched_at,
    })
}

/// One full pass over `video_metadata_raw`. Streaming keyset pagination;
/// one write transaction per page via [`Store::apply_metadata_batch`].
/// Idempotent and replayable: every pass overwrites the typed columns from
/// the current blobs (last-write-wins). `dry_run` does the full
/// examine/parse pass and reports real counts without writing.
pub(crate) fn load_metadata(store: &mut Store, dry_run: bool) -> Result<LoadStats> {
    let mut stats = LoadStats::default();
    let mut after: Option<String> = None;

    loop {
        let page = store.metadata_raw_page(after.as_deref(), PAGE_SIZE)?;
        let Some(last) = page.last() else { break };
        after = Some(last.video_id.clone());

        let mut batch: Vec<MetadataColumns> = Vec::with_capacity(page.len());
        for row in &page {
            stats.rows_examined += 1;
            match parse_envelope(&row.video_id, row.fetched_at, &row.raw_json) {
                Some(cols) => batch.push(cols),
                None => {
                    stats.rows_skipped_unparseable += 1;
                    tracing::warn!(
                        video_id = row.video_id.as_str(),
                        "unparseable metadata envelope; skipped"
                    );
                }
            }
        }

        if dry_run {
            stats.rows_loaded += batch.len() as u64;
        } else {
            let changed = store.apply_metadata_batch(&batch)?;
            stats.rows_loaded += changed as u64;
            // A parsed row whose videos row is gone matches 0 — counted,
            // never an error (the raw table outlives no-longer-tracked ids).
            stats.rows_without_video += (batch.len() - changed) as u64;
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_envelope_maps_printed_fields_to_columns() {
        let envelope = r#"{"schema":1,"printed":"{\"id\":\"v1\",\"description\":\"hello #tag\",\"uploader\":\"acct\",\"uploader_id\":\"123\",\"timestamp\":1768924271,\"view_count\":9900000,\"like_count\":572300,\"comment_count\":865}"}"#;
        let cols = parse_envelope("v1", 1_753_700_000, envelope).expect("parses");
        assert_eq!(cols.video_id, "v1");
        assert_eq!(cols.video_description.as_deref(), Some("hello #tag"));
        assert_eq!(cols.uploader.as_deref(), Some("acct"));
        assert_eq!(cols.uploader_id.as_deref(), Some("123"));
        assert_eq!(cols.video_created_at, Some(1_768_924_271));
        assert_eq!(cols.view_count, Some(9_900_000));
        assert_eq!(cols.like_count, Some(572_300));
        assert_eq!(cols.comment_count, Some(865));
        assert_eq!(cols.metadata_fetched_at, 1_753_700_000);
    }

    #[test]
    fn parse_envelope_absent_fields_become_null() {
        let envelope = r#"{"schema":1,"printed":"{\"id\":\"v1\"}"}"#;
        let cols = parse_envelope("v1", 1, envelope).expect("parses");
        assert!(cols.video_description.is_none() && cols.view_count.is_none());
    }

    #[test]
    fn parse_envelope_rejects_garbage_and_bad_printed() {
        assert!(parse_envelope("v1", 1, "not json").is_none());
        assert!(parse_envelope("v1", 1, r#"{"schema":1,"printed":"not json"}"#).is_none());
        // Unknown future schema version: skip, don't guess.
        assert!(parse_envelope("v1", 1, r#"{"schema":2,"printed":"{}"}"#).is_none());
    }
}
