use crate::admin_builtins::kb;
use crate::config;
use crate::settings_store::SettingsStore;
use crate::settings_types::{
    KnowledgeRuntimeStatus, default_knowledge_report_relative_path, default_knowledge_report_target,
};

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const DEFAULT_FAILURE_COOLDOWN_SECS: u64 = 300;
const LOCK_STALE_AFTER_SECS: u64 = 7200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeLoopDriver {
    Worker,
    Kairos,
}

impl KnowledgeLoopDriver {
    fn as_str(self) -> &'static str {
        match self {
            KnowledgeLoopDriver::Worker => "worker",
            KnowledgeLoopDriver::Kairos => "kairos",
        }
    }
}

#[derive(Debug, Clone)]
struct KnowledgeLoopRuntimeConfig {
    mode: String,
    workspace_id: Option<String>,
    periodic_interval_secs: u64,
    report_target: String,
    report_relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct KnowledgeLoopState {
    #[serde(default)]
    version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_success_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_success_raw_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_attempt_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_report_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_workspace_name: Option<String>,
    #[serde(default)]
    last_source_count: usize,
    #[serde(default)]
    last_compiled_sources: usize,
    #[serde(default)]
    last_concept_count: usize,
    #[serde(default)]
    last_issue_count: usize,
}

struct KnowledgeLoopLock {
    lock_path: PathBuf,
    _file: std::fs::File,
}

impl Drop for KnowledgeLoopLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn normalize_report_target(target: &str) -> String {
    match target.trim().to_ascii_lowercase().as_str() {
        "index" => "index".to_string(),
        "output" => "output".to_string(),
        _ => default_knowledge_report_target(),
    }
}

fn state_path(root: &Path) -> PathBuf {
    root.join("index").join("knowledge_loop_state.json")
}

fn lock_path(root: &Path) -> PathBuf {
    root.join("index").join("knowledge_loop.lock")
}

fn read_state(root: &Path) -> KnowledgeLoopState {
    let path = state_path(root);
    fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str::<KnowledgeLoopState>(&body).ok())
        .unwrap_or_default()
}

fn write_state(root: &Path, state: &KnowledgeLoopState) -> Result<(), std::io::Error> {
    let path = state_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(state)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    fs::write(path, body)
}

fn runtime_status_from_state(state: &KnowledgeLoopState) -> KnowledgeRuntimeStatus {
    KnowledgeRuntimeStatus {
        last_result: state.last_result.clone().unwrap_or_default(),
        last_driver: state.last_driver.clone(),
        last_workspace_id: state.last_workspace_id.clone(),
        last_workspace_name: state.last_workspace_name.clone(),
        last_issue_count: state.last_issue_count,
        last_report_path: state.last_report_path.clone(),
        last_error: state.last_error.clone(),
        last_run_reason: state.last_run_reason.clone(),
        last_success_at: state.last_success_at,
    }
}

fn publish_runtime_status(
    settings_store: &SettingsStore,
    state: &KnowledgeLoopState,
) -> Result<(), String> {
    settings_store.set_value(
        "agent.knowledge_runtime",
        serde_json::to_value(runtime_status_from_state(state)).map_err(|err| err.to_string())?,
    )
}

fn lock_age_secs(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| epoch_secs().saturating_sub(duration.as_secs()))
}

fn acquire_lock(root: &Path) -> Result<Option<KnowledgeLoopLock>, std::io::Error> {
    let path = lock_path(root);
    if let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(&path) {
        let _ = writeln!(
            &mut file,
            "{{\"pid\":{},\"created_at\":{}}}",
            std::process::id(),
            epoch_secs()
        );
        return Ok(Some(KnowledgeLoopLock {
            lock_path: path,
            _file: file,
        }));
    }

    if let Some(age_secs) = lock_age_secs(&path) {
        if age_secs > LOCK_STALE_AFTER_SECS {
            let _ = fs::remove_file(&path);
            if let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(&path) {
                let _ = writeln!(
                    &mut file,
                    "{{\"pid\":{},\"created_at\":{}}}",
                    std::process::id(),
                    epoch_secs()
                );
                return Ok(Some(KnowledgeLoopLock {
                    lock_path: path,
                    _file: file,
                }));
            }
        }
    }

    Ok(None)
}

fn compute_raw_fingerprint(sources: &[crate::IlhaeAppKbSourceDto]) -> String {
    let mut hasher = DefaultHasher::new();
    for source in sources {
        source.source_id.hash(&mut hasher);
        source.relative_path.hash(&mut hasher);
        source.kind.hash(&mut hasher);
        source.size.hash(&mut hasher);
        source.modified_at.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

fn build_report_markdown(
    driver: KnowledgeLoopDriver,
    mode: &str,
    workspace_id: &str,
    workspace_name: &str,
    root: &Path,
    fingerprint: &str,
    raw_source_count: usize,
    compiled_sources: usize,
    concept_count: usize,
    generated_files: &[String],
    issues: &[crate::IlhaeAppKbLintIssueDto],
    report_target: &str,
    report_relative_path: &str,
    run_reason: &str,
) -> String {
    let mut body = String::new();
    body.push_str("# Knowledge Loop Health Report\n\n");
    body.push_str(&format!("- Driver: `{}`\n", driver.as_str()));
    body.push_str(&format!("- Mode: `{}`\n", mode));
    body.push_str(&format!(
        "- Workspace: `{}` ({})\n",
        workspace_id, workspace_name
    ));
    body.push_str(&format!("- Root: `{}`\n", root.to_string_lossy()));
    body.push_str(&format!("- Trigger: `{}`\n", run_reason));
    body.push_str(&format!("- Raw fingerprint: `{}`\n", fingerprint));
    body.push_str(&format!("- Raw sources: `{}`\n", raw_source_count));
    body.push_str(&format!("- Compiled sources: `{}`\n", compiled_sources));
    body.push_str(&format!("- Concepts: `{}`\n", concept_count));
    body.push_str(&format!("- Issues: `{}`\n", issues.len()));
    body.push_str(&format!(
        "- Report target: `{} / {}`\n\n",
        report_target, report_relative_path
    ));

    body.push_str("## Generated Files\n\n");
    if generated_files.is_empty() {
        body.push_str("- None\n");
    } else {
        for file in generated_files {
            body.push_str(&format!("- `{}`\n", file));
        }
    }

    body.push_str("\n## Issues\n\n");
    if issues.is_empty() {
        body.push_str("- No lint issues detected.\n");
    } else {
        for issue in issues.iter().take(25) {
            body.push_str(&format!(
                "- [{}] `{}`: {}\n",
                issue.kind, issue.path, issue.message
            ));
        }
        if issues.len() > 25 {
            body.push_str(&format!("- ... and {} more\n", issues.len() - 25));
        }
    }

    body
}

fn runtime_config_from_settings(
    settings: &crate::settings_types::Settings,
) -> Option<KnowledgeLoopRuntimeConfig> {
    let raw_mode = if settings.agent.knowledge_mode.trim().is_empty() {
        "off"
    } else {
        settings.agent.knowledge_mode.as_str()
    };
    let mode = config::normalize_knowledge_mode(raw_mode);
    if mode == "off" {
        return None;
    }

    Some(KnowledgeLoopRuntimeConfig {
        mode,
        workspace_id: settings
            .agent
            .knowledge_workspace_id
            .clone()
            .filter(|workspace_id| !workspace_id.trim().is_empty()),
        periodic_interval_secs: settings.agent.knowledge_periodic_interval_secs.max(1),
        report_target: normalize_report_target(&settings.agent.knowledge_report_target),
        report_relative_path: settings
            .agent
            .knowledge_report_relative_path
            .trim()
            .to_string()
            .if_empty_then(default_knowledge_report_relative_path()),
    })
}

trait EmptyStringExt {
    fn if_empty_then(self, fallback: String) -> String;
}

impl EmptyStringExt for String {
    fn if_empty_then(self, fallback: String) -> String {
        if self.trim().is_empty() {
            fallback
        } else {
            self
        }
    }
}

fn should_run_cycle(
    state: &KnowledgeLoopState,
    fingerprint: &str,
    now: u64,
    periodic_interval_secs: u64,
) -> Option<&'static str> {
    if let Some(last_attempt_at) = state.last_attempt_at {
        if state.last_result.as_deref() == Some("error")
            && now.saturating_sub(last_attempt_at) < DEFAULT_FAILURE_COOLDOWN_SECS
        {
            return None;
        }
    }

    match state.last_success_at {
        None => Some("initial"),
        Some(_)
            if state
                .last_success_raw_fingerprint
                .as_deref()
                .map(|last| last != fingerprint)
                .unwrap_or(true) =>
        {
            Some("raw-changed")
        }
        Some(last_success_at) if now.saturating_sub(last_success_at) >= periodic_interval_secs => {
            Some("periodic")
        }
        _ => None,
    }
}

fn resolve_workspace_root(
    ilhae_dir: &Path,
    workspace_id: Option<&str>,
) -> Result<(String, String, PathBuf), std::io::Error> {
    let (_registry, workspace) = kb::resolve_workspace(ilhae_dir, workspace_id)?;
    Ok((
        workspace.id.clone(),
        workspace.name.clone(),
        PathBuf::from(workspace.root_path),
    ))
}

fn run_cycle_blocking(
    driver: KnowledgeLoopDriver,
    ilhae_dir: PathBuf,
    runtime: KnowledgeLoopRuntimeConfig,
    settings_store: Arc<SettingsStore>,
) -> anyhow::Result<()> {
    let (workspace_id, workspace_name, root) =
        resolve_workspace_root(&ilhae_dir, runtime.workspace_id.as_deref())?;
    kb::ensure_workspace_dirs(&root)?;

    let lock = match acquire_lock(&root)? {
        Some(lock) => lock,
        None => {
            info!(
                driver = driver.as_str(),
                workspace_id = %workspace_id,
                "[KnowledgeLoop] skipped because another loop already holds the workspace lock"
            );
            return Ok(());
        }
    };

    let mut state = read_state(&root);
    let now = epoch_secs();
    let sources = kb::collect_sources(&root)?;
    let fingerprint = compute_raw_fingerprint(&sources);
    let Some(run_reason) =
        should_run_cycle(&state, &fingerprint, now, runtime.periodic_interval_secs)
    else {
        info!(
            driver = driver.as_str(),
            workspace_id = %workspace_id,
            mode = %runtime.mode,
            "[KnowledgeLoop] skipped because raw fingerprint is unchanged and periodic interval has not elapsed"
        );
        drop(lock);
        return Ok(());
    };

    let run_result: anyhow::Result<(
        usize,
        usize,
        usize,
        Vec<String>,
        Vec<crate::IlhaeAppKbLintIssueDto>,
        String,
    )> = (|| {
        let inventory_path = root.join("index").join("raw_inventory.json");
        let inventory_body = serde_json::to_vec_pretty(&sources)?;
        fs::write(&inventory_path, inventory_body)?;

        let (compiled_sources, concept_count, generated_files) = kb::compile_workspace(&root)?;
        let issues = kb::lint_workspace(&root)?;
        let report_target = normalize_report_target(&runtime.report_target);
        let report_path =
            kb::resolve_relative_target(&root, &report_target, &runtime.report_relative_path)?;
        let report_content = build_report_markdown(
            driver,
            &runtime.mode,
            &workspace_id,
            &workspace_name,
            &root,
            &fingerprint,
            sources.len(),
            compiled_sources,
            concept_count,
            &generated_files,
            &issues,
            &report_target,
            &runtime.report_relative_path,
            run_reason,
        );
        kb::write_markdown(&report_path, &report_content)?;

        Ok((
            compiled_sources,
            concept_count,
            sources.len(),
            generated_files,
            issues,
            report_path
                .strip_prefix(&root)
                .unwrap_or(report_path.as_path())
                .to_string_lossy()
                .to_string(),
        ))
    })();

    match run_result {
        Ok((
            compiled_sources,
            concept_count,
            source_count,
            _generated_files,
            issues,
            report_path,
        )) => {
            state.version = state.version.saturating_add(1);
            state.last_success_at = Some(now);
            state.last_success_raw_fingerprint = Some(fingerprint);
            state.last_attempt_at = Some(now);
            state.last_driver = Some(driver.as_str().to_string());
            state.last_result = Some("ok".to_string());
            state.last_error = None;
            state.last_report_path = Some(report_path.clone());
            state.last_run_reason = Some(run_reason.to_string());
            state.last_workspace_id = Some(workspace_id.clone());
            state.last_workspace_name = Some(workspace_name.clone());
            state.last_source_count = source_count;
            state.last_compiled_sources = compiled_sources;
            state.last_concept_count = concept_count;
            state.last_issue_count = issues.len();
            write_state(&root, &state)?;
            let _ = publish_runtime_status(&settings_store, &state);

            info!(
                driver = driver.as_str(),
                workspace_id = %workspace_id,
                source_count = source_count,
                issue_count = issues.len(),
                report_path = %report_path,
                "[KnowledgeLoop] completed ingest → compile → lint → file_back"
            );

            Ok(())
        }
        Err(err) => {
            state.version = state.version.saturating_add(1);
            state.last_attempt_at = Some(now);
            state.last_driver = Some(driver.as_str().to_string());
            state.last_result = Some("error".to_string());
            state.last_error = Some(err.to_string());
            state.last_run_reason = Some(run_reason.to_string());
            state.last_workspace_id = Some(workspace_id.clone());
            state.last_workspace_name = Some(workspace_name.clone());
            state.last_source_count = sources.len();
            let _ = write_state(&root, &state);
            let _ = publish_runtime_status(&settings_store, &state);
            Err(err)
        }
    }
}

pub async fn maybe_run_cycle(
    driver: KnowledgeLoopDriver,
    settings_store: Arc<SettingsStore>,
    ilhae_dir: PathBuf,
) {
    let settings_snapshot = settings_store.get();
    let Some(runtime) = runtime_config_from_settings(&settings_snapshot) else {
        return;
    };

    let driver_enabled = match driver {
        KnowledgeLoopDriver::Worker => config::knowledge_mode_includes_worker(&runtime.mode),
        KnowledgeLoopDriver::Kairos => config::knowledge_mode_includes_kairos(&runtime.mode),
    };
    if !driver_enabled {
        return;
    }

    let result = tokio::task::spawn_blocking(move || {
        run_cycle_blocking(driver, ilhae_dir, runtime, settings_store)
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            warn!(
                driver = driver.as_str(),
                error = %err,
                "[KnowledgeLoop] cycle failed"
            );
        }
        Err(err) => {
            warn!(
                driver = driver.as_str(),
                error = %err,
                "[KnowledgeLoop] worker join failed"
            );
        }
    }
}

pub(crate) fn run_cleanup_cycle_now(
    settings_store: Arc<SettingsStore>,
    ilhae_dir: PathBuf,
) -> Result<bool, String> {
    let settings_snapshot = settings_store.get();
    let Some(runtime) = runtime_config_from_settings(&settings_snapshot) else {
        return Ok(false);
    };
    if !config::knowledge_mode_includes_worker(&runtime.mode) {
        return Ok(false);
    }
    run_cycle_blocking(
        KnowledgeLoopDriver::Worker,
        ilhae_dir,
        runtime,
        settings_store,
    )
    .map(|_| true)
    .map_err(|err| err.to_string())
}

pub async fn run_worker_loop(settings_store: Arc<SettingsStore>, ilhae_dir: PathBuf) {
    info!("[KnowledgeLoop] worker loop started");
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        maybe_run_cycle(
            KnowledgeLoopDriver::Worker,
            settings_store.clone(),
            ilhae_dir.clone(),
        )
        .await;

        let poll_interval_secs = settings_store
            .get()
            .agent
            .knowledge_poll_interval_secs
            .max(1);
        tokio::time::sleep(Duration::from_secs(poll_interval_secs)).await;
    }
}
