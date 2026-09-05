//! A `PRAGMA user_version` migration stepper.
//!
//! Deliberately hand-rolled rather than `refinery`: refinery-core 0.9.2 caps at
//! `rusqlite <= 0.39`, and this workspace is on 0.40. Adopting it would mean
//! either a downgrade or linking two copies of SQLite, both worse than sixty
//! lines of stepper.
//!
//! Migrations are embedded in the binary with `include_str!`, applied in order,
//! and each runs inside a transaction with `user_version` bumped in the same
//! transaction — so a failure leaves the database at the previous version
//! rather than half-migrated.

use rusqlite::Connection;

use crate::error::{Error, Result};

struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("migrations/001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "agent_attribution",
        sql: include_str!("migrations/002_agent_attribution.sql"),
    },
    Migration {
        version: 3,
        name: "rule_dismissals",
        sql: include_str!("migrations/003_rule_dismissals.sql"),
    },
    Migration {
        version: 4,
        name: "integrity_chain",
        sql: include_str!("migrations/004_integrity_chain.sql"),
    },
    Migration {
        version: 5,
        name: "deletion_log",
        sql: include_str!("migrations/005_deletion_log.sql"),
    },
];

/// The schema version this build knows how to produce.
#[must_use]
pub fn latest_version() -> u32 {
    MIGRATIONS.last().map_or(0, |m| m.version)
}

/// Read the schema version currently stored in the database.
pub fn current_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Bring `conn` up to [`latest_version`], returning the version it ended at.
///
/// Idempotent: running against an already-current database applies nothing.
/// Refuses to touch a database written by a newer build rather than risk
/// corrupting it.
pub fn migrate(conn: &Connection) -> Result<u32> {
    let from = current_version(conn)?;
    let known = latest_version();

    if from > known {
        return Err(Error::SchemaFromTheFuture { found: from, known });
    }

    for m in MIGRATIONS.iter().filter(|m| m.version > from) {
        tracing::info!(version = m.version, name = m.name, "applying migration");

        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(m.sql).map_err(|source| Error::Migration {
            version: m.version,
            name: m.name,
            source,
        })?;
        // PRAGMA does not accept bound parameters; the value is a literal from
        // the table above, never user input.
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
    }

    current_version(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[test]
    fn migrates_from_empty() {
        let db = Db::open_in_memory().expect("open");
        assert_eq!(
            current_version(db.conn()).expect("version"),
            latest_version()
        );
        assert!(latest_version() >= 1);
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let db = Db::open_in_memory().expect("open");
        let first = current_version(db.conn()).expect("version");
        let second = migrate(db.conn()).expect("re-migrate");
        assert_eq!(first, second);
    }

    #[test]
    fn migrating_a_populated_database_preserves_data() {
        let db = Db::open_in_memory().expect("open");
        db.conn()
            .execute(
                "INSERT INTO raw_event (lane, source_ref, content_sha256, ingested_at, body)
                 VALUES ('transcript', 'x.jsonl', 'deadbeef', 1, '{}')",
                [],
            )
            .expect("insert");

        migrate(db.conn()).expect("re-migrate");

        let n: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM raw_event", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 1, "re-migration must not disturb existing rows");
    }

    /// With more than one migration the stepper can be tested part-way, which
    /// a single-migration schema could not exercise.
    #[test]
    fn steps_forward_from_a_partial_schema() {
        let db = Db::open_in_memory().expect("open");
        assert!(
            latest_version() >= 2,
            "this test needs at least two migrations"
        );

        // Pretend only migration 001 had ever run.
        db.conn()
            .pragma_update(None, "user_version", 1)
            .expect("rewind");
        db.conn()
            .execute_batch(
                "DROP TABLE deletion;
                 DROP INDEX raw_event_unchained;
                 ALTER TABLE raw_event DROP COLUMN chain_sha256;
                 DROP TABLE rule_dismissal;
                 DROP INDEX tool_call_agent_id;
                 ALTER TABLE tool_call DROP COLUMN agent_id;
                 ALTER TABLE session DROP COLUMN slug;",
            )
            .expect("undo everything after 001");

        let ended = migrate(db.conn()).expect("step to latest");
        assert_eq!(ended, latest_version());

        // The column 002 adds is back.
        db.conn()
            .query_row("SELECT agent_id FROM tool_call LIMIT 1", [], |_| Ok(()))
            .or_else(|e| {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .expect("agent_id exists after stepping");
    }

    #[test]
    fn refuses_a_schema_from_the_future() {
        let db = Db::open_in_memory().expect("open");
        db.conn()
            .pragma_update(None, "user_version", latest_version() + 1)
            .expect("bump");

        match migrate(db.conn()) {
            Err(Error::SchemaFromTheFuture { .. }) => {}
            other => panic!("expected SchemaFromTheFuture, got {other:?}"),
        }
    }
}
