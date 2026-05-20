//! Schema-version handling on Store::open. Per 0022, mismatches are
//! a typed error that directs the operator to `uu-tiktok migrate`.

use anyhow::Result;
use rusqlite::Connection;
use tempfile::TempDir;
use uu_tiktok::state::{StateError, Store};

fn open_raw(path: &std::path::Path) -> Connection {
    Connection::open(path).unwrap()
}

#[test]
fn open_on_fresh_db_succeeds_and_records_version() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let store = Store::open(&path)?;
    let v = store
        .read_meta("schema_version")?
        .expect("recorded on first run");
    assert_eq!(v, uu_tiktok::state::SCHEMA_VERSION);
    Ok(())
}

#[test]
fn open_on_mismatched_version_returns_typed_error() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");

    let _ = Store::open(&path)?;

    let raw = open_raw(&path);
    raw.execute(
        "UPDATE meta SET value = '0' WHERE key = 'schema_version'",
        [],
    )?;
    drop(raw);

    // .err().expect() rather than .expect_err() because `Store` doesn't derive
    // Debug (rusqlite::Connection doesn't impl Debug); .expect_err requires
    // Debug on the Ok type, .err().expect does not.
    let err = Store::open(&path)
        .err()
        .expect("expected schema-version mismatch");
    let state_err = err
        .downcast_ref::<StateError>()
        .expect("error chain contains StateError");
    match state_err {
        StateError::SchemaVersionMismatch { expected, found } => {
            assert_eq!(expected, uu_tiktok::state::SCHEMA_VERSION);
            assert_eq!(found, "0");
        }
    }

    let display = format!("{}", state_err);
    assert!(
        display.contains("uu-tiktok migrate"),
        "operator-readable instruction missing from Display: {}",
        display
    );

    Ok(())
}
