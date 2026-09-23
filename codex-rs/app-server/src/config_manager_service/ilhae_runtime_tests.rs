use super::*;
use anyhow::Result;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn transaction_retains_shared_lock_until_drop() -> Result<()> {
    let home = tempdir()?;
    let lock =
        acquire_ilhae_runtime_config_lock_blocking(home.path(), Duration::ZERO, Duration::ZERO)?
            .expect("uncontended lock");
    let transaction = RuntimeConfigTransaction {
        codex_home: home.path().to_path_buf(),
        lock: Some(lock),
        snapshot: None,
    };
    assert!(
        acquire_ilhae_runtime_config_lock_blocking(home.path(), Duration::ZERO, Duration::ZERO,)?
            .is_none()
    );
    drop(transaction);
    assert!(
        acquire_ilhae_runtime_config_lock_blocking(home.path(), Duration::ZERO, Duration::ZERO,)?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn transaction_rejects_external_config_changes_before_sidecar_update() -> Result<()> {
    let home = tempdir()?;
    let path = AbsolutePathBuf::from_absolute_path(home.path().join("config.toml"))?;
    let original_text = "model = \"local-model\"\n";
    std::fs::write(path.as_path(), original_text)?;
    std::fs::write(home.path().join(".config.toml.ilhae-lkg"), original_text)?;
    let original: TomlValue = toml::from_str(original_text)?;
    let updated: TomlValue = toml::from_str(
        "model = \"local-model\"\n[mcp_servers.office]\nurl = \"http://localhost/mcp\"\n",
    )?;
    let lock =
        acquire_ilhae_runtime_config_lock_blocking(home.path(), Duration::ZERO, Duration::ZERO)?
            .expect("uncontended lock");
    let mut transaction = RuntimeConfigTransaction {
        codex_home: home.path().to_path_buf(),
        lock: Some(lock),
        snapshot: None,
    };
    transaction.capture(&path, &original).await?;
    let external = "model = \"external-edit\"\n";
    std::fs::write(path.as_path(), external)?;
    let error = transaction
        .before_persist(&path, &original, &updated)
        .await
        .expect_err("external edit must prevent sidecar update");
    assert_eq!(
        error.write_error_code(),
        Some(ConfigWriteErrorCode::ConfigVersionConflict)
    );
    assert_eq!(std::fs::read_to_string(path.as_path())?, external);
    assert!(
        !home
            .path()
            .join(".config.toml.ilhae-runtime-ownership.json")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn transaction_without_runtime_lock_leaves_sidecars_untouched() -> Result<()> {
    let home = tempdir()?;
    let path = AbsolutePathBuf::from_absolute_path(home.path().join("config.toml"))?;
    let original: TomlValue = toml::from_str("model = \"local-model\"")?;
    let updated: TomlValue = toml::from_str(
        "model = \"local-model\"\n[mcp_servers.office]\nurl = \"http://localhost/mcp\"\n",
    )?;
    let mut transaction = RuntimeConfigTransaction {
        codex_home: home.path().to_path_buf(),
        lock: None,
        snapshot: None,
    };
    transaction.capture(&path, &original).await?;
    transaction
        .before_persist(&path, &original, &updated)
        .await?;
    assert_eq!(std::fs::read_dir(home.path())?.count(), 0);
    Ok(())
}
