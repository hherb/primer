//! Validate-and-seed pass for the closed-vocabulary lookup tables.
//!
//! The Rust enums (`Speaker`, `PedagogicalIntent`, `EngagementState`,
//! `UnderstandingDepth`) are the single source of truth; the on-disk
//! tables are a derived projection this module regenerates and
//! validates on every `open()`. Drift is a hard error — there is
//! deliberately no API for mutating a lookup table.

use std::collections::HashMap;

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// Validate-and-seed pass for a lookup table.
///
/// `expected` is the canonical source of truth (Rust enum projected to
/// `(id, name)` pairs). The function:
///
/// - Inserts any expected row missing from the table.
/// - Returns `Storage` error if a known id has the wrong name.
/// - Returns `Storage` error if the table contains an id not in `expected`.
///
/// Runs synchronously inside an `&Connection` borrow; caller is
/// responsible for transaction scope.
pub fn validate_and_seed_lookup(
    conn: &Connection,
    table: &str,
    expected: &[(i64, &str)],
) -> Result<()> {
    let actual: HashMap<i64, String> = {
        let sql = format!("SELECT id, name FROM {table}");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| PrimerError::Storage(format!("prepare for {table}: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| PrimerError::Storage(format!("query {table}: {e}")))?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name) =
                row.map_err(|e| PrimerError::Storage(format!("read {table} row: {e}")))?;
            map.insert(id, name);
        }
        map
    };

    let expected_ids: std::collections::HashSet<i64> = expected.iter().map(|(id, _)| *id).collect();

    // Step 1: detect unknown ids in the DB.
    for (id, name) in &actual {
        if !expected_ids.contains(id) {
            return Err(PrimerError::Storage(format!(
                "{table} has unknown row id={id} name={name:?} — database may be from a newer build"
            )));
        }
    }

    // Step 2: validate matches and insert missing.
    let insert_sql = format!("INSERT INTO {table} (id, name) VALUES (?1, ?2)");
    for (id, expected_name) in expected {
        match actual.get(id) {
            Some(actual_name) if actual_name == expected_name => {
                // match — nothing to do
            }
            Some(actual_name) => {
                return Err(PrimerError::Storage(format!(
                    "{table} row id={id} has name {actual_name:?}, this build expects {expected_name:?}"
                )));
            }
            None => {
                conn.execute(&insert_sql, rusqlite::params![id, expected_name])
                    .map_err(|e| PrimerError::Storage(format!("insert into {table}: {e}")))?;
            }
        }
    }

    Ok(())
}
