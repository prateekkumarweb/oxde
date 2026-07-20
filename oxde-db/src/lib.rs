use std::{collections::HashSet, path::Path};

pub mod models;

use models::User;
pub use toasty;

/// Opens (creating the file if needed) the SQLite-compatible database file
/// under `data_dir`. Does not touch schema - see [`apply_migrations`].
///
/// # Errors
///
/// Returns an error if the database file can't be opened.
pub async fn connect(data_dir: &Path) -> anyhow::Result<toasty::Db> {
    let db_path = data_dir.join("oxde.db");
    let url = format!("turso:{}", db_path.display());

    let mut builder = toasty::Db::builder();
    builder.models(toasty::models!(User));
    Ok(builder.connect(&url).await?)
}

/// Applies every migration under `toasty/migrations` that hasn't already
/// been applied to this database.
///
/// Reimplements `toasty-cli`'s `migration apply` using `toasty`'s public
/// primitives directly, since `toasty-cli` prints to stdout and pulls in
/// `clap`/`dialoguer` this path doesn't need.
///
/// # Errors
///
/// Returns an error if a pending migration fails to apply.
pub async fn apply_migrations(db: &toasty::Db) -> anyhow::Result<()> {
    let toasty_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../toasty");
    let history = toasty::migration::History::load_or_default(
        toasty_dir.join("history.toml").to_string_lossy().as_ref(),
    )?;

    let mut conn = db
        .driver()
        .connect(&toasty::db::ConnectContext::default())
        .await?;

    let applied_ids: HashSet<u64> = conn
        .applied_migrations()
        .await?
        .iter()
        .map(toasty_core::schema::db::AppliedMigration::id)
        .collect();

    let pending: Vec<_> = history
        .entries()
        .iter()
        .filter(|entry| !applied_ids.contains(&entry.id))
        .collect();

    if pending.is_empty() {
        tracing::info!("database schema is up to date");
        return Ok(());
    }

    for entry in pending {
        let sql = std::fs::read_to_string(toasty_dir.join("migrations").join(&entry.name))?;
        conn.apply_migration(
            entry.id,
            &entry.name,
            &toasty_core::schema::db::Migration::new_sql(sql),
        )
        .await?;
        tracing::info!(migration = entry.name, "applied database migration");
    }

    Ok(())
}
