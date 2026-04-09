use crate::config::{load_ilhae_toml_config, save_ilhae_toml_config};
use crate::settings_store::SettingsStore;
use brain_rs::BrainService;
use brain_rs::schedule::Schedule;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const DEFAULT_HYGIENE_POLL_INTERVAL_SECS: u64 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HygieneLoopDriver {
    Worker,
    Kairos,
}

impl HygieneLoopDriver {
    fn as_str(self) -> &'static str {
        match self {
            HygieneLoopDriver::Worker => "worker",
            HygieneLoopDriver::Kairos => "kairos",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HygieneLoopOutcome {
    pub(crate) duplicate_groups: usize,
    pub(crate) duplicate_tasks_folded: usize,
    pub(crate) legacy_commands_normalized: usize,
    pub(crate) knowledge_reports_written: usize,
    pub(crate) memory_reports_written: usize,
    pub(crate) orphaned_source_pages: usize,
    pub(crate) orphaned_concept_pages: usize,
    pub(crate) stale_output_candidates: usize,
    pub(crate) duplicate_knowledge_items: usize,
    pub(crate) dream_duplicate_candidates: usize,
    pub(crate) dream_rare_candidates: usize,
    pub(crate) ignored_dream_candidates: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HygieneLoopState {
    #[serde(default)]
    version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_driver: Option<String>,
    #[serde(default)]
    last_outcome: HygieneLoopOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn state_path(ilhae_dir: &Path) -> PathBuf {
    ilhae_dir.join("brain").join("hygiene_loop_state.json")
}

fn read_state(ilhae_dir: &Path) -> HygieneLoopState {
    let path = state_path(ilhae_dir);
    fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str::<HygieneLoopState>(&body).ok())
        .unwrap_or_default()
}

fn write_state(ilhae_dir: &Path, state: &HygieneLoopState) -> Result<(), std::io::Error> {
    let path = state_path(ilhae_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(state)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    fs::write(path, body)
}

fn normalize_hygiene_mode(mode: &str) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "worker" => "worker",
        "kairos" => "kairos",
        "both" | "" => "both",
        "off" => "off",
        "true" | "enabled" => "both",
        "worker-only" => "worker",
        "kairos-only" => "kairos",
        _ => "both",
    }
}

pub fn hygiene_mode_includes_worker(mode: &str) -> bool {
    matches!(normalize_hygiene_mode(mode), "worker" | "both")
}

pub fn hygiene_mode_includes_kairos(mode: &str) -> bool {
    matches!(normalize_hygiene_mode(mode), "kairos" | "both")
}

fn publish_runtime_status(
    settings_store: &SettingsStore,
    driver: HygieneLoopDriver,
    outcome: &HygieneLoopOutcome,
    run_reason: &str,
    error: Option<&str>,
) {
    let status = crate::settings_types::HygieneRuntimeStatus {
        last_result: if error.is_some() {
            "error".to_string()
        } else if outcome.duplicate_tasks_folded > 0
            || outcome.legacy_commands_normalized > 0
            || outcome.knowledge_reports_written > 0
        {
            "changed".to_string()
        } else {
            "ok".to_string()
        },
        last_driver: Some(driver.as_str().to_string()),
        duplicate_tasks_folded: outcome.duplicate_tasks_folded,
        legacy_commands_normalized: outcome.legacy_commands_normalized,
        knowledge_reports_written: outcome.knowledge_reports_written,
        memory_reports_written: outcome.memory_reports_written,
        orphaned_source_pages: outcome.orphaned_source_pages,
        orphaned_concept_pages: outcome.orphaned_concept_pages,
        stale_output_candidates: outcome.stale_output_candidates,
        duplicate_knowledge_items: outcome.duplicate_knowledge_items,
        dream_duplicate_candidates: outcome.dream_duplicate_candidates,
        dream_rare_candidates: outcome.dream_rare_candidates,
        ignored_dream_candidates: outcome.ignored_dream_candidates,
        last_run_reason: Some(run_reason.to_string()),
        last_error: error.map(|value| value.to_string()),
        last_success_at: error.map(|_| None).unwrap_or_else(|| Some(now_secs())),
    };
    let _ = settings_store.set_value("agent.hygiene_runtime", serde_json::json!(status));
}

fn super_loop_tasks(brain: &BrainService) -> Vec<Schedule> {
    brain
        .schedule_list()
        .into_iter()
        .filter(|task| task.title.starts_with("[super-loop] "))
        .collect()
}

fn fold_duplicate_super_loop_tasks(
    brain: &BrainService,
    outcome: &mut HygieneLoopOutcome,
) -> Result<(), String> {
    let mut groups: BTreeMap<String, Vec<Schedule>> = BTreeMap::new();
    for task in super_loop_tasks(brain) {
        groups.entry(task.title.clone()).or_default().push(task);
    }

    for (title, mut tasks) in groups {
        if tasks.len() <= 1 {
            continue;
        }
        outcome.duplicate_groups += 1;
        tasks.sort_by(|left, right| {
            let left_done = left.done || left.status == "done";
            let right_done = right.done || right.status == "done";
            (left_done, left.created_at.as_str(), left.id.as_str()).cmp(&(
                right_done,
                right.created_at.as_str(),
                right.id.as_str(),
            ))
        });
        let canonical = tasks[0].clone();
        for duplicate in tasks.into_iter().skip(1) {
            if !(duplicate.done || duplicate.status == "done") {
                let detail = format!(
                    "Folded duplicate super-loop task into {} ({})",
                    canonical.id, canonical.title
                );
                brain.schedule_update_full(
                    &duplicate.id,
                    None,
                    None,
                    Some(true),
                    Some("done"),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                let _ = brain.schedule_add_history(
                    &duplicate.id,
                    "hygiene_dedupe_folded",
                    Some(&detail),
                    None,
                );
                let _ = brain.schedule_add_history(
                    &canonical.id,
                    "hygiene_dedupe_canonical",
                    Some(&format!(
                        "Retained canonical task for duplicate title: {title}"
                    )),
                    None,
                );
                outcome.duplicate_tasks_folded += 1;
            }
        }
    }

    Ok(())
}

fn normalize_runtime_command(settings_store: &SettingsStore) -> Result<usize, String> {
    let settings = settings_store.get();
    let command = settings.agent.command.trim();
    if command == "ilhae" || !command.starts_with("codex-ilhae") {
        return Ok(0);
    }
    settings_store
        .set_value("agent.command", serde_json::json!("ilhae"))
        .map_err(|err| err.to_string())?;
    Ok(1)
}

fn normalize_active_profile_command() -> Result<usize, String> {
    let mut config = load_ilhae_toml_config();
    let Some(active_profile) = config.profile.active.clone() else {
        return Ok(0);
    };
    let Some(profile) = config.profiles.get_mut(&active_profile) else {
        return Ok(0);
    };
    let command = profile.agent.command.as_deref().unwrap_or("");
    if command == "ilhae" || !command.starts_with("codex-ilhae") {
        return Ok(0);
    }
    profile.agent.command = Some("ilhae".to_string());
    save_ilhae_toml_config(&config)?;
    Ok(1)
}

fn active_workspace_root(
    ilhae_dir: &Path,
    settings_store: &SettingsStore,
) -> Result<Option<PathBuf>, String> {
    let settings = settings_store.get();
    let workspace_id = settings.agent.knowledge_workspace_id.as_deref().or(settings
        .agent
        .knowledge_runtime
        .last_workspace_id
        .as_deref());
    match crate::admin_builtins::kb::resolve_workspace(ilhae_dir, workspace_id) {
        Ok((_registry, workspace)) => Ok(Some(PathBuf::from(workspace.root_path))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

fn extract_concepts_for_hygiene(source: &crate::IlhaeAppKbSourceDto) -> Vec<String> {
    let mut concepts = std::collections::BTreeSet::new();
    for token in source
        .relative_path
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .chain(
            source
                .title
                .as_deref()
                .unwrap_or_default()
                .split(|ch: char| !ch.is_ascii_alphanumeric()),
        )
    {
        let token = token.trim().to_ascii_lowercase();
        if token.len() < 4 || token.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        concepts.insert(token);
        if concepts.len() >= 8 {
            break;
        }
    }
    concepts.into_iter().collect()
}

fn markdown_files_under(root: &Path) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect()
}

fn write_raw_inventory(root: &Path, sources: &[crate::IlhaeAppKbSourceDto]) -> Result<(), String> {
    let inventory_path = root.join("index").join("raw_inventory.json");
    let body = serde_json::to_vec_pretty(sources).map_err(|err| err.to_string())?;
    fs::write(inventory_path, body).map_err(|err| err.to_string())
}

fn run_knowledge_hygiene(
    ilhae_dir: &Path,
    settings_store: &SettingsStore,
    outcome: &mut HygieneLoopOutcome,
) -> Result<(), String> {
    let Some(root) = active_workspace_root(ilhae_dir, settings_store)? else {
        return Ok(());
    };
    crate::admin_builtins::kb::ensure_workspace_dirs(&root).map_err(|err| err.to_string())?;

    let sources =
        crate::admin_builtins::kb::collect_sources(&root).map_err(|err| err.to_string())?;
    write_raw_inventory(&root, &sources)?;

    let source_index_path = root.join("index").join("sources.md");
    let concept_index_path = root.join("index").join("concepts.md");
    if !source_index_path.exists() || !concept_index_path.exists() {
        let _ =
            crate::admin_builtins::kb::compile_workspace(&root).map_err(|err| err.to_string())?;
    }

    let lint_issues =
        crate::admin_builtins::kb::lint_workspace(&root).map_err(|err| err.to_string())?;
    let expected_source_slugs = sources
        .iter()
        .map(|source| crate::admin_builtins::kb::slugify(&source.relative_path))
        .collect::<std::collections::BTreeSet<_>>();
    let expected_concepts = sources
        .iter()
        .flat_map(extract_concepts_for_hygiene)
        .collect::<std::collections::BTreeSet<_>>();

    let orphaned_source_pages = markdown_files_under(&root.join("wiki").join("sources"))
        .into_iter()
        .filter_map(|path| {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.to_string())?;
            Some((stem, path))
        })
        .filter(|(stem, _)| !expected_source_slugs.contains(stem))
        .map(|(_, path)| {
            path.strip_prefix(&root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();
    let orphaned_concept_pages = markdown_files_under(&root.join("wiki").join("concepts"))
        .into_iter()
        .filter_map(|path| {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.to_string())?;
            Some((stem, path))
        })
        .filter(|(stem, _)| !expected_concepts.contains(stem))
        .map(|(_, path)| {
            path.strip_prefix(&root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();

    let latest_source_modified = sources
        .iter()
        .filter_map(|source| fs::metadata(root.join(&source.relative_path)).ok())
        .filter_map(|meta| meta.modified().ok())
        .max();
    let stale_output_candidates = markdown_files_under(&root.join("output"))
        .into_iter()
        .filter(|path| {
            let Some(latest_source_modified) = latest_source_modified else {
                return false;
            };
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .map(|modified| modified < latest_source_modified)
                .unwrap_or(false)
        })
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();

    let mut report = String::from("# Knowledge Hygiene Report\n\n");
    report.push_str(&format!("- Sources: {}\n", sources.len()));
    report.push_str(&format!("- Lint issues: {}\n", lint_issues.len()));
    report.push_str(&format!(
        "- Orphaned source pages: {}\n",
        orphaned_source_pages.len()
    ));
    report.push_str(&format!(
        "- Orphaned concept pages: {}\n",
        orphaned_concept_pages.len()
    ));
    report.push_str(&format!(
        "- Stale output candidates: {}\n\n",
        stale_output_candidates.len()
    ));

    report.push_str("## Lint Issues\n\n");
    if lint_issues.is_empty() {
        report.push_str("- None\n");
    } else {
        for issue in &lint_issues {
            report.push_str(&format!(
                "- `{}` `{}`: {}\n",
                issue.kind, issue.path, issue.message
            ));
        }
    }

    report.push_str("\n## Orphaned Source Pages\n\n");
    if orphaned_source_pages.is_empty() {
        report.push_str("- None\n");
    } else {
        for path in &orphaned_source_pages {
            report.push_str(&format!("- `{path}`\n"));
        }
    }

    report.push_str("\n## Orphaned Concept Pages\n\n");
    if orphaned_concept_pages.is_empty() {
        report.push_str("- None\n");
    } else {
        for path in &orphaned_concept_pages {
            report.push_str(&format!("- `{path}`\n"));
        }
    }

    report.push_str("\n## Stale Output Candidates\n\n");
    if stale_output_candidates.is_empty() {
        report.push_str("- None\n");
    } else {
        for path in &stale_output_candidates {
            report.push_str(&format!("- `{path}`\n"));
        }
    }

    crate::admin_builtins::kb::write_markdown(
        &root.join("index").join("knowledge_hygiene.md"),
        &report,
    )
    .map_err(|err| err.to_string())?;

    outcome.knowledge_reports_written += 1;
    outcome.orphaned_source_pages += orphaned_source_pages.len();
    outcome.orphaned_concept_pages += orphaned_concept_pages.len();
    outcome.stale_output_candidates += stale_output_candidates.len();
    Ok(())
}

fn normalize_text_for_hygiene(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_ascii_lowercase()
}

fn run_memory_hygiene(
    brain: &BrainService,
    ilhae_dir: &Path,
    settings_store: &SettingsStore,
    outcome: &mut HygieneLoopOutcome,
) -> Result<(), String> {
    let Some(root) = active_workspace_root(ilhae_dir, settings_store)? else {
        return Ok(());
    };

    let items = brain.memory_list_items().map_err(|err| err.to_string())?;
    let mut duplicate_groups: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    for item in items {
        let id = item
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let title = item
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let summary = item
            .get("summary")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let normalized_title = normalize_text_for_hygiene(&title);
        let normalized_summary = normalize_text_for_hygiene(&summary)
            .chars()
            .take(180)
            .collect::<String>();
        if normalized_title.is_empty() {
            continue;
        }
        duplicate_groups
            .entry((normalized_title, normalized_summary))
            .or_default()
            .push((id, title));
    }
    let duplicate_candidates = duplicate_groups
        .into_iter()
        .filter(|(_, items)| items.len() > 1)
        .collect::<Vec<_>>();

    let dream_analysis = brain
        .memory_dream_analyze(&root, 8)
        .unwrap_or_else(|_| serde_json::json!({}));
    let dream_duplicate_candidates = dream_analysis
        .get("duplicate_candidates")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let dream_rare_candidates = dream_analysis
        .get("rare_candidates")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let ignored_preview = brain
        .memory_dream_ignored_preview(8)
        .unwrap_or_else(|_| serde_json::json!({}));
    let ignored_dream_candidates = ignored_preview
        .get("groups")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut report = String::from("# Memory Hygiene Report\n\n");
    report.push_str(&format!(
        "- Duplicate knowledge item groups: {}\n",
        duplicate_candidates.len()
    ));
    report.push_str(&format!(
        "- Dream duplicate candidates: {}\n",
        dream_duplicate_candidates.len()
    ));
    report.push_str(&format!(
        "- Dream rare candidates: {}\n\n",
        dream_rare_candidates.len()
    ));
    report.push_str(&format!(
        "- Ignored dream review candidates: {}\n\n",
        ignored_dream_candidates.len()
    ));

    report.push_str("## Duplicate Knowledge Item Candidates\n\n");
    if duplicate_candidates.is_empty() {
        report.push_str("- None\n");
    } else {
        for ((_title_key, summary_key), items) in &duplicate_candidates {
            report.push_str(&format!(
                "- signature: `{}` / {} items\n",
                summary_key,
                items.len()
            ));
            for (id, title) in items {
                report.push_str(&format!("  - `{id}` — {}\n", title));
            }
        }
    }

    report.push_str("\n## Dream Duplicate Candidates\n\n");
    if dream_duplicate_candidates.is_empty() {
        report.push_str("- None\n");
    } else {
        for candidate in &dream_duplicate_candidates {
            let left = candidate
                .get("left_path")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let right = candidate
                .get("right_path")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let score = candidate
                .get("score")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            report.push_str(&format!("- `{left}` <-> `{right}` (score {:.3})\n", score));
        }
    }

    report.push_str("\n## Dream Rare Candidates\n\n");
    if dream_rare_candidates.is_empty() {
        report.push_str("- None\n");
    } else {
        for candidate in &dream_rare_candidates {
            let path = candidate
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let rarity = candidate
                .get("rarity_score")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            report.push_str(&format!("- `{path}` (rarity {:.3})\n", rarity));
        }
    }

    report.push_str("\n## Ignored Dream Review Candidates\n\n");
    if ignored_dream_candidates.is_empty() {
        report.push_str("- None\n");
    } else {
        for group in &ignored_dream_candidates {
            let path = group
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let chunk_count = group
                .get("chunk_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let title = group
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or("untitled");
            report.push_str(&format!(
                "- `{path}` — {} chunks — {}\n",
                chunk_count, title
            ));
        }
    }

    crate::admin_builtins::kb::write_markdown(
        &root.join("index").join("memory_hygiene.md"),
        &report,
    )
    .map_err(|err| err.to_string())?;

    outcome.memory_reports_written += 1;
    outcome.duplicate_knowledge_items += duplicate_candidates.len();
    outcome.dream_duplicate_candidates += dream_duplicate_candidates.len();
    outcome.dream_rare_candidates += dream_rare_candidates.len();
    outcome.ignored_dream_candidates += ignored_dream_candidates.len();
    Ok(())
}

pub(crate) fn run_embedded_cycle(
    driver: HygieneLoopDriver,
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    ilhae_dir: PathBuf,
) -> Result<HygieneLoopOutcome, String> {
    let settings = settings_store.get();
    let mode = settings.agent.hygiene_mode.clone();
    let should_run = match driver {
        HygieneLoopDriver::Worker => hygiene_mode_includes_worker(&mode),
        HygieneLoopDriver::Kairos => hygiene_mode_includes_kairos(&mode),
    };
    if !should_run {
        return Ok(HygieneLoopOutcome::default());
    }

    let mut state = read_state(&ilhae_dir);
    let mut outcome = HygieneLoopOutcome::default();
    let run_reason = format!("{}:safe_cleanup", driver.as_str());

    let cycle_result = (|| -> Result<(), String> {
        fold_duplicate_super_loop_tasks(&brain, &mut outcome)?;
        outcome.legacy_commands_normalized += normalize_runtime_command(&settings_store)?;
        outcome.legacy_commands_normalized += normalize_active_profile_command()?;
        run_knowledge_hygiene(&ilhae_dir, &settings_store, &mut outcome)?;
        run_memory_hygiene(&brain, &ilhae_dir, &settings_store, &mut outcome)?;
        Ok(())
    })();

    state.version = state.version.saturating_add(1);
    state.last_run_at = Some(now_secs());
    state.last_driver = Some(driver.as_str().to_string());
    state.last_outcome = outcome.clone();

    match cycle_result {
        Ok(()) => {
            state.last_error = None;
            publish_runtime_status(&settings_store, driver, &outcome, &run_reason, None);
            if outcome.duplicate_tasks_folded > 0
                || outcome.legacy_commands_normalized > 0
                || outcome.knowledge_reports_written > 0
                || outcome.memory_reports_written > 0
            {
                info!(
                    driver = driver.as_str(),
                    duplicate_groups = outcome.duplicate_groups,
                    duplicate_tasks_folded = outcome.duplicate_tasks_folded,
                    legacy_commands_normalized = outcome.legacy_commands_normalized,
                    knowledge_reports_written = outcome.knowledge_reports_written,
                    memory_reports_written = outcome.memory_reports_written,
                    orphaned_source_pages = outcome.orphaned_source_pages,
                    orphaned_concept_pages = outcome.orphaned_concept_pages,
                    stale_output_candidates = outcome.stale_output_candidates,
                    duplicate_knowledge_items = outcome.duplicate_knowledge_items,
                    dream_duplicate_candidates = outcome.dream_duplicate_candidates,
                    dream_rare_candidates = outcome.dream_rare_candidates,
                    ignored_dream_candidates = outcome.ignored_dream_candidates,
                    "[HygieneLoop] applied safe cleanup"
                );
            }
        }
        Err(err) => {
            state.last_error = Some(err.clone());
            publish_runtime_status(&settings_store, driver, &outcome, &run_reason, Some(&err));
            write_state(&ilhae_dir, &state).map_err(|io_err| io_err.to_string())?;
            return Err(err);
        }
    }

    write_state(&ilhae_dir, &state).map_err(|err| err.to_string())?;
    Ok(outcome)
}

fn run_cycle_blocking(
    driver: HygieneLoopDriver,
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    ilhae_dir: PathBuf,
) -> Result<HygieneLoopOutcome, String> {
    run_embedded_cycle(driver, brain, settings_store, ilhae_dir)
}

pub async fn maybe_run_cycle(
    driver: HygieneLoopDriver,
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    ilhae_dir: PathBuf,
) -> Option<HygieneLoopOutcome> {
    let result = tokio::task::spawn_blocking(move || {
        run_cycle_blocking(driver, brain, settings_store, ilhae_dir)
    })
    .await;
    match result {
        Ok(Ok(outcome)) => Some(outcome),
        Ok(Err(err)) => {
            warn!(driver = driver.as_str(), error = %err, "[HygieneLoop] cycle failed");
            None
        }
        Err(err) => {
            warn!(driver = driver.as_str(), error = %err, "[HygieneLoop] worker join failed");
            None
        }
    }
}

pub async fn run_worker_loop(
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    ilhae_dir: PathBuf,
) {
    info!("[HygieneLoop] worker loop started");
    // Initial delay before first hygiene cycle
    tokio::time::sleep(Duration::from_secs(75)).await;
    loop {
        let outcome = maybe_run_cycle(
            HygieneLoopDriver::Worker,
            brain.clone(),
            settings_store.clone(),
            ilhae_dir.clone(),
        )
        .await;
        
        // Spawn background LLM Dream if candidates are found and dream mode is enabled
        if settings_store.get().agent.dream_mode {
            if let Some(out) = outcome {
                if out.dream_duplicate_candidates > 0 || out.dream_rare_candidates > 0 {
                    info!("[HygieneLoop] Discovered dream candidates. Spawning background autonomous Dream Agent...");
                    let exe_path = std::env::current_exe().unwrap_or_else(|_| "ilhae".into());
                let dream_prompt = "너는 백그라운드 지식 정리(Dream) 에이전트야. \
                    [CRITICAL RULE: 절대 실제 소스코드(.ts, .rs, .py 등)나 프로젝트 파일을 수정/삭제하지 마라! 오직 지식 금고(.brain/ 또는 글로벌 메모리)의 마크다운(.md) 파일만 다루어야 한다.] \
                    사용자와 직접 소통하지 말고 즉시 `brain_memory_ops`의 `dream_preview`를 호출해서 \
                    산발된 기억 조각들과 중복 청크들을 진단해. \
                    그 후 `brain_artifact_ops`를 사용해 의미 있는 마크다운 파일(index.md 등)로 융합하고 중복을 제거해. \
                    모든 정리가 끝나면 마지막으로 반드시 `propose` 도구를 호출하여 agent: 'ilhae' (의식/메인 에이전트) 에게 '🌙 무의식(Dream) 에이전트가 백그라운드 지식 융합 및 정리를 완료했습니다!' 라고 A2A 보고 메시지를 전송한 뒤 세션을 종료해.";
                
                tokio::spawn(async move {
                    match tokio::process::Command::new(exe_path)
                        .arg("exec")
                        .arg(dream_prompt)
                        .env("ILHAE_DREAM_MODE", "1")
                        .output()
                        .await
                    {
                        Ok(output) => {
                            if output.status.success() {
                                info!("[HygieneLoop] Background Dream Agent completed successfully.");
                            } else {
                                warn!("[HygieneLoop] Background Dream Agent failed with exit code: {:?}", output.status.code());
                            }
                        }
                        Err(e) => {
                            warn!("[HygieneLoop] Failed to spawn Background Dream Agent: {}", e);
                        }
                    }
                });
            }
        }
        }

        tokio::time::sleep(Duration::from_secs(DEFAULT_HYGIENE_POLL_INTERVAL_SECS)).await;
    }
}
