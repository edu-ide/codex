use std::borrow::Cow;

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;

pub(crate) static STATE_MIGRATOR: Migrator = sqlx::migrate!("./migrations");
pub(crate) static LOGS_MIGRATOR: Migrator = sqlx::migrate!("./logs_migrations");
pub(crate) static GOALS_MIGRATOR: Migrator = sqlx::migrate!("./goals_migrations");
pub(crate) static ILHAE_GOALS_MIGRATOR: Migrator = sqlx::migrate!("./ilhae_goals_migrations");
static LEGACY_ILHAE_STATE_MIGRATOR: Migrator = sqlx::migrate!("./legacy_state_migrations");
pub(crate) static MEMORIES_MIGRATOR: Migrator = sqlx::migrate!("./memory_migrations");
pub(crate) static QUEUE_MIGRATOR: Migrator = sqlx::migrate!("./queue_migrations");
pub(crate) static THREAD_HISTORY_MIGRATOR: Migrator = sqlx::migrate!("./thread_history_migrations");

const ILHAE_GOALS_MIGRATIONS_TABLE: &str = "_ilhae_goals_migrations";

/// Allow an older Codex binary to open a database that has already been
/// migrated by a newer binary running in parallel.
///
/// We intentionally ignore applied migration versions that are newer than the
/// embedded migration set. Known migration versions are still validated by
/// checksum, so this only relaxes the "database is ahead of me" case.
fn runtime_migrator(base: &'static Migrator) -> Migrator {
    Migrator {
        migrations: Cow::Borrowed(base.migrations.as_ref()),
        ignore_missing: true,
        locking: base.locking,
        no_tx: base.no_tx,
        table_name: base.table_name.clone(),
        create_schemas: base.create_schemas.clone(),
    }
}

pub(crate) fn runtime_state_migrator() -> Migrator {
    runtime_migrator(&STATE_MIGRATOR)
}

pub(crate) fn runtime_logs_migrator() -> Migrator {
    runtime_migrator(&LOGS_MIGRATOR)
}

pub(crate) fn runtime_goals_migrator() -> Migrator {
    runtime_migrator(&GOALS_MIGRATOR)
}

pub(crate) fn runtime_ilhae_goals_migrator() -> Migrator {
    let mut migrator = runtime_migrator(&ILHAE_GOALS_MIGRATOR);
    migrator.table_name = Cow::Borrowed(ILHAE_GOALS_MIGRATIONS_TABLE);
    migrator
}

pub(crate) fn runtime_memories_migrator() -> Migrator {
    runtime_migrator(&MEMORIES_MIGRATOR)
}

pub(crate) fn runtime_queue_migrator() -> Migrator {
    runtime_migrator(&QUEUE_MIGRATOR)
}

// The paginated history projector will call this when it takes ownership of opening the database.
#[allow(dead_code)]
pub(crate) fn runtime_thread_history_migrator() -> Migrator {
    runtime_migrator(&THREAD_HISTORY_MIGRATOR)
}

pub(crate) async fn repair_legacy_recency_migration_version(
    pool: &SqlitePool,
    migrator: &Migrator,
) -> anyhow::Result<()> {
    let Some(recency_migration) = migrator
        .migrations
        .iter()
        .find(|migration| migration.version == 39)
    else {
        return Ok(());
    };
    let migrations_table_exists = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?
    .is_some();
    if !migrations_table_exists {
        return Ok(());
    }

    let legacy_recency_needs_repair = sqlx::query_scalar::<_, i64>(
        r#"
SELECT 1
FROM _sqlx_migrations
WHERE version = ?
  AND checksum = ?
  AND NOT EXISTS (
      SELECT 1 FROM _sqlx_migrations WHERE version = ?
  )
        "#,
    )
    .bind(38_i64)
    .bind(recency_migration.checksum.as_ref())
    .bind(recency_migration.version)
    .fetch_optional(pool)
    .await?
    .is_some();
    if !legacy_recency_needs_repair {
        return Ok(());
    }

    sqlx::query(
        r#"
UPDATE _sqlx_migrations
SET version = ?, description = ?
WHERE version = ?
  AND checksum = ?
  AND NOT EXISTS (
      SELECT 1 FROM _sqlx_migrations WHERE version = ?
  )
        "#,
    )
    .bind(recency_migration.version)
    .bind(recency_migration.description.as_ref())
    .bind(38_i64)
    .bind(recency_migration.checksum.as_ref())
    .bind(recency_migration.version)
    .execute(pool)
    .await?;
    Ok(())
}

fn applied_migration_matches(
    row: &(i64, String, bool, Vec<u8>),
    migration: &sqlx::migrate::Migration,
) -> bool {
    row.0 == migration.version
        && row.1 == migration.description
        && row.2
        && row.3.as_slice() == migration.checksum.as_ref()
}

pub(crate) async fn repair_legacy_ilhae_state_migrations(
    pool: &SqlitePool,
    migrator: &Migrator,
) -> anyhow::Result<()> {
    let migrations_table_exists = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?
    .is_some();
    if !migrations_table_exists {
        return Ok(());
    }

    let applied = sqlx::query_as::<_, (i64, String, bool, Vec<u8>)>(
        r#"
SELECT version, description, success, checksum
FROM _sqlx_migrations
WHERE version BETWEEN 33 AND 52
ORDER BY version
        "#,
    )
    .fetch_all(pool)
    .await?;
    let Some(first_legacy_migration) = LEGACY_ILHAE_STATE_MIGRATOR.migrations.first() else {
        return Ok(());
    };
    if !applied
        .iter()
        .any(|row| applied_migration_matches(row, first_legacy_migration))
    {
        return Ok(());
    }
    let all_rows_are_known = applied.iter().all(|row| {
        LEGACY_ILHAE_STATE_MIGRATOR
            .migrations
            .iter()
            .any(|migration| applied_migration_matches(row, migration))
            || migrator.migrations.iter().any(|migration| {
                (35..=47).contains(&migration.version)
                    && row.0 == migration.version + 5
                    && row.1 == migration.description
                    && row.2
                    && row.3.as_slice() == migration.checksum.as_ref()
            })
    });
    if !all_rows_are_known {
        return Ok(());
    }

    let mut transaction = pool.begin().await?;
    sqlx::query("DROP TABLE IF EXISTS thread_goal_loop_history")
        .execute(&mut *transaction)
        .await?;
    for migration in LEGACY_ILHAE_STATE_MIGRATOR.migrations.iter() {
        sqlx::query(
            "DELETE FROM _sqlx_migrations WHERE version = ? AND description = ? AND checksum = ?",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&mut *transaction)
        .await?;
    }
    for migration in migrator
        .migrations
        .iter()
        .filter(|migration| (35..=47).contains(&migration.version))
    {
        sqlx::query(
            r#"
UPDATE _sqlx_migrations
SET version = ?
WHERE version = ?
  AND description = ?
  AND checksum = ?
            "#,
        )
        .bind(migration.version)
        .bind(migration.version + 5)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn repair_legacy_goals_migrations(
    pool: &SqlitePool,
    upstream_migrator: &Migrator,
    ilhae_migrator: &Migrator,
) -> anyhow::Result<()> {
    const CONTINUATION_VERSION: i64 = 2;

    let Some(continuation_migration) = upstream_migrator
        .migrations
        .iter()
        .find(|migration| migration.version == CONTINUATION_VERSION)
    else {
        return Ok(());
    };
    let Some(loop_history_migration) = ilhae_migrator.migrations.first() else {
        return Ok(());
    };
    let migrations_table_exists = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?
    .is_some();
    if !migrations_table_exists {
        return Ok(());
    }

    let applied = sqlx::query_as::<_, (i64, String, bool, Vec<u8>)>(
        r#"
SELECT version, description, success, checksum
FROM _sqlx_migrations
WHERE version IN (2, 3)
        "#,
    )
    .fetch_all(pool)
    .await?;
    let custom_version = applied.iter().find_map(|row| {
        let mut expected = loop_history_migration.clone();
        expected.version = row.0;
        applied_migration_matches(row, &expected).then_some(row.0)
    });
    let Some(custom_version) = custom_version else {
        return Ok(());
    };
    let continuation_version = applied.iter().find_map(|row| {
        let mut expected = continuation_migration.clone();
        expected.version = row.0;
        applied_migration_matches(row, &expected).then_some(row.0)
    });
    if applied.iter().any(|row| {
        row.0 != custom_version && continuation_version != Some(row.0) && (row.0 == 2 || row.0 == 3)
    }) {
        return Ok(());
    }

    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS _ilhae_goals_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
)
        "#,
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
INSERT OR IGNORE INTO _ilhae_goals_migrations (
    version, description, installed_on, success, checksum, execution_time
)
SELECT ?, description, installed_on, success, checksum, execution_time
FROM _sqlx_migrations
WHERE version = ?
        "#,
    )
    .bind(loop_history_migration.version)
    .bind(custom_version)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = ?")
        .bind(custom_version)
        .execute(&mut *transaction)
        .await?;
    if let Some(continuation_version) = continuation_version
        && continuation_version != continuation_migration.version
    {
        sqlx::query("UPDATE _sqlx_migrations SET version = ? WHERE version = ?")
            .bind(continuation_migration.version)
            .bind(continuation_version)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod tests;
