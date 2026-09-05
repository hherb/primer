//! The additive schema-migration chain, one submodule per version step.
//!
//! Every `apply_vN_migrations` is idempotent (guarded `ALTER`s and
//! `CREATE … IF NOT EXISTS`) and wraps its whole body in a single
//! transaction, so a partial failure rolls back to the pre-migration
//! state rather than leaving a half-migrated database. `open()` runs the
//! whole chain on every open, which is what brings an older database up
//! to [`USER_VERSION`](super::USER_VERSION).
//!
//! Adding a version means: add `vN.rs` here, declare + re-export it
//! below, call it from the open path, and bump `USER_VERSION`.

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

mod v2;
mod v3;
mod v4;
mod v5;
mod v6;
mod v7;
mod v8;

pub use v2::apply_v2_migrations;
pub(crate) use v3::apply_v3_migrations;
pub(crate) use v4::apply_v4_migrations;
pub(crate) use v5::apply_v5_migrations;
pub(crate) use v6::apply_v6_migrations;
pub(crate) use v7::apply_v7_migrations;
pub(crate) use v8::apply_v8_migrations;

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let sql = format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1");
    let count: i64 = conn
        .query_row(&sql, rusqlite::params![column], |r| r.get(0))
        .map_err(|e| PrimerError::Storage(format!("table_info({table}): {e}")))?;
    Ok(count > 0)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .map_err(|e| PrimerError::Storage(format!("check table {name}: {e}")))?;
    Ok(count > 0)
}
