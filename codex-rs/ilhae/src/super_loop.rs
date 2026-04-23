use crate::context_proxy::autonomy::state::{AutonomousPhase, AutonomousSessionState};
use crate::settings_store::SettingsStore;
use brain_rs::BrainService;
use codex_protocol::items::LoopLifecycleItem;
use codex_protocol::protocol::{LoopLifecycleKind, LoopLifecycleStatus};
use dsrs::{ChatAdapter, LM, Predict, configure};
use moka::sync::Cache;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const DEFAULT_SUPER_LOOP_COOLDOWN_SECS: u64 = 180;
const DEFAULT_SUPER_LOOP_POLL_INTERVAL_SECS: u64 = 90;
const DEFAULT_GEPA_SIDECAR_TIMEOUT_SECS: u64 = 10;
const DEFAULT_DSRS_SELF_IMPROVEMENT_MAX_TOKENS: u32 = 160;
const DEFAULT_DSRS_SELF_IMPROVEMENT_TEMPERATURE: f32 = 0.2;

static DSRS_SELF_IMPROVEMENT_CONFIG_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(dsrs::Signature, Clone, Debug)]
struct SelfImprovementFollowupGeneratorSignature {
    /// Rewrite the self-improvement follow-up prompt conservatively.

    #[input]
    subject: String,

    #[input]
    detail: String,

    #[input]
    default_prompt: String,

    #[input]
    default_instructions: String,

    #[output]
    prompt: String,

    #[output]
    instructions: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperLoopDriver {
    Worker,
    Kairos,
}

impl SuperLoopDriver {
    fn as_str(self) -> &'static str {
        match self {
            SuperLoopDriver::Worker => "worker",
            SuperLoopDriver::Kairos => "kairos",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SuperLoopFindingKind {
    KnowledgeLoopError,
    KnowledgeGap,
    SelfImprovementCandidate,
    IgnoredDreamReviewCandidate,
    ExecutionBlocked,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuperLoopFinding {
    kind: SuperLoopFindingKind,
    subject: String,
    detail: String,
    severity: String,
    signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SuperLoopActionKind {
    RunHygieneCycle,
    RunKnowledgeCycle,
    UpsertFollowupTask,
    RunFollowupTask,
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuperLoopAction {
    kind: SuperLoopActionKind,
    target: String,
    detail: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SuperLoopResolution {
    Resolved,
    Running,
    Retry,
    Escalated,
    Stale,
    Planned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuperLoopTaskScore {
    task_id: String,
    title: Option<String>,
    resolution: SuperLoopResolution,
    detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SuperLoopOutcome {
    findings_count: usize,
    actions_count: usize,
    created_tasks: usize,
    updated_tasks: usize,
    resolved_tasks: usize,
    running_tasks: usize,
    retry_tasks: usize,
    escalated_tasks: usize,
    stale_tasks: usize,
    planned_tasks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SuperLoopState {
    #[serde(default)]
    version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    last_findings: Vec<SuperLoopFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    last_actions: Vec<SuperLoopAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    last_scores: Vec<SuperLoopTaskScore>,
    #[serde(default)]
    last_outcome: SuperLoopOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfImprovementFollowupSpec {
    pub(crate) prompt: String,
    pub(crate) instructions: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GepaSidecarRequest {
    pub(crate) kind: String,
    pub(crate) preset: String,
    pub(crate) subject: String,
    pub(crate) detail: String,
    pub(crate) prompt: String,
    pub(crate) instructions: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) task_history: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) top_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group_count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct GepaSidecarResponse {
    #[serde(default)]
    pub(crate) optimized_prompt: Option<String>,
    #[serde(default)]
    pub(crate) optimized_instructions: Option<String>,
    #[serde(default)]
    pub(crate) optimization_status: Option<String>,
    #[serde(default)]
    pub(crate) optimizer: Option<String>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) score: Option<f64>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn loop_item_id(kind: LoopLifecycleKind, driver: SuperLoopDriver, epoch_ms: i64) -> String {
    let kind = match kind {
        LoopLifecycleKind::SuperLoop => "super_loop",
        LoopLifecycleKind::ImprovementLoop => "improvement_loop",
        _ => "background_loop",
    };
    format!("{kind}:{}:{epoch_ms}", driver.as_str())
}

fn loop_item(
    id: String,
    kind: LoopLifecycleKind,
    title: &str,
    summary: String,
    detail: Option<String>,
    status: LoopLifecycleStatus,
    reason: Option<String>,
    counts: Option<BTreeMap<String, i64>>,
    error: Option<String>,
    duration_ms: Option<i64>,
) -> LoopLifecycleItem {
    LoopLifecycleItem {
        id,
        kind,
        title: title.to_string(),
        summary,
        detail,
        status,
        reason,
        counts,
        error,
        duration_ms,
        target_profile: None,
    }
}

fn emit_loop_notification(notification: crate::IlhaeLoopLifecycleNotification) {
    crate::emit_native_loop_lifecycle(notification);
}

fn state_path(ilhae_dir: &Path) -> PathBuf {
    ilhae_dir.join("brain").join("super_loop_state.json")
}

fn read_state(ilhae_dir: &Path) -> SuperLoopState {
    let path = state_path(ilhae_dir);
    fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str::<SuperLoopState>(&body).ok())
        .unwrap_or_default()
}

fn write_state(ilhae_dir: &Path, state: &SuperLoopState) -> Result<(), std::io::Error> {
    let path = state_path(ilhae_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(state)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    fs::write(path, body)
}

fn super_loop_enabled(
    settings: &crate::settings_types::Settings,
    autonomous_sessions: &Arc<Cache<String, AutonomousSessionState>>,
) -> bool {
    let hygiene_enabled =
        crate::hygiene_loop::hygiene_mode_includes_worker(&settings.agent.hygiene_mode)
            || crate::hygiene_loop::hygiene_mode_includes_kairos(&settings.agent.hygiene_mode);
    settings.agent.knowledge_mode != "off"
        || hygiene_enabled
        || settings.agent.self_improvement_enabled
        || settings.agent.autonomous_mode
        || settings.agent.kairos_enabled
        || autonomous_sessions.iter().next().is_some()
}

fn build_signature(findings: &[SuperLoopFinding]) -> String {
    if findings.is_empty() {
        return "idle".to_string();
    }
    findings
        .iter()
        .map(|finding| finding.signature.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

fn should_skip(state: &SuperLoopState, signature: &str, now: u64) -> bool {
    if state.last_signature.as_deref() != Some(signature) {
        return false;
    }
    let Some(last_run_at) = state.last_run_at else {
        return false;
    };
    now.saturating_sub(last_run_at) < DEFAULT_SUPER_LOOP_COOLDOWN_SECS
}

fn make_followup_title(prefix: &str, subject: &str) -> String {
    format!("[super-loop] {prefix}: {subject}")
}

fn self_improvement_uses_gepa_sidecar(settings: &crate::settings_types::Settings) -> bool {
    settings.agent.self_improvement_enabled
        && settings
            .agent
            .self_improvement_preset
            .eq_ignore_ascii_case("gepa_sidecar")
}

fn self_improvement_uses_dsrs_runtime(settings: &crate::settings_types::Settings) -> bool {
    settings.agent.self_improvement_enabled
        && settings
            .agent
            .self_improvement_preset
            .eq_ignore_ascii_case("dsrs_runtime")
}

pub(crate) fn default_self_improvement_followup_spec_for_runtime() -> SelfImprovementFollowupSpec {
    SelfImprovementFollowupSpec {
        prompt:
            "Review pending dream groups, decide safe summarize/promote/extract/skill-candidate actions, and record the decision."
                .to_string(),
        instructions:
            "Use memory_dream_preview, memory_dream_analyze, memory_dream_summarize, memory_dream_promote, memory_promote, memory_extract, skills_list, skill_view, and skill_upsert as needed. Prefer memory_dream_promote for durable knowledge. When a repeated complex workflow or correction becomes a stable reusable procedure, first inspect existing skills, then create or intentionally update an agentskills/Codex-compatible SKILL.md under brain/skills/custom with YAML name/description and concise instructions. Do not duplicate skills or overwrite user-edited skills without explicit evidence."
                .to_string(),
    }
}

fn default_self_improvement_followup_spec(
    _finding: &SuperLoopFinding,
) -> SelfImprovementFollowupSpec {
    default_self_improvement_followup_spec_for_runtime()
}

fn dsrs_self_improvement_model() -> Option<String> {
    std::env::var("ILHAE_DSRS_SELF_IMPROVEMENT_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn dsrs_self_improvement_base_url() -> Option<String> {
    std::env::var("ILHAE_DSRS_SELF_IMPROVEMENT_BASE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn dsrs_self_improvement_api_key() -> Option<String> {
    std::env::var("ILHAE_DSRS_SELF_IMPROVEMENT_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn dsrs_self_improvement_max_tokens() -> u32 {
    std::env::var("ILHAE_DSRS_SELF_IMPROVEMENT_MAX_TOKENS")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_DSRS_SELF_IMPROVEMENT_MAX_TOKENS)
}

fn dsrs_self_improvement_temperature() -> f32 {
    std::env::var("ILHAE_DSRS_SELF_IMPROVEMENT_TEMPERATURE")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(DEFAULT_DSRS_SELF_IMPROVEMENT_TEMPERATURE)
}

fn dsrs_self_improvement_instruction() -> &'static str {
    "Generate a conservative self-improvement follow-up spec for ilhae's super-loop. Keep the tool scope aligned with the provided defaults, preserve review-first behavior, preserve memory_dream and skill_upsert guidance, and prefer minimal edits over novelty. Return concise prompt and instructions only."
}

fn generate_dsrs_self_improvement_followup(
    finding: &SuperLoopFinding,
    default_spec: &SelfImprovementFollowupSpec,
) -> Option<SelfImprovementFollowupSpec> {
    let model = dsrs_self_improvement_model()?;
    let _guard = DSRS_SELF_IMPROVEMENT_CONFIG_LOCK.lock().ok()?;
    let subject = finding.subject.clone();
    let detail = finding.detail.clone();
    let default_prompt = default_spec.prompt.clone();
    let default_instructions = default_spec.instructions.clone();
    let base_url = dsrs_self_improvement_base_url();
    let api_key = dsrs_self_improvement_api_key();
    let temperature = dsrs_self_improvement_temperature();
    let max_tokens = dsrs_self_improvement_max_tokens();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            warn!(error = %error, "[SuperLoop] failed to initialize DSRs runtime");
            return None;
        }
    };

    let result = runtime.block_on(async move {
        let lm = LM::builder()
            .model(model)
            .temperature(temperature)
            .max_tokens(max_tokens)
            .maybe_base_url(base_url)
            .maybe_api_key(api_key)
            .build()
            .await
            .map_err(|error| error.to_string())?;
        configure(lm, ChatAdapter);

        let predictor = Predict::<SelfImprovementFollowupGeneratorSignature>::builder()
            .instruction(dsrs_self_improvement_instruction())
            .build();
        let response = predictor
            .call(SelfImprovementFollowupGeneratorSignatureInput {
                subject,
                detail,
                default_prompt,
                default_instructions,
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok::<SelfImprovementFollowupSpec, String>(SelfImprovementFollowupSpec {
            prompt: response.prompt.trim().to_string(),
            instructions: response.instructions.trim().to_string(),
        })
    });

    match result {
        Ok(spec) if !spec.prompt.is_empty() && !spec.instructions.is_empty() => Some(spec),
        Ok(_) => {
            warn!("[SuperLoop] DSRs runtime returned an empty self-improvement follow-up");
            None
        }
        Err(error) => {
            warn!(error = %error, "[SuperLoop] DSRs runtime follow-up generation failed");
            None
        }
    }
}

pub(crate) fn parse_gepa_sidecar_timeout_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_GEPA_SIDECAR_TIMEOUT_SECS)
}

pub(crate) fn gepa_sidecar_timeout_secs() -> u64 {
    parse_gepa_sidecar_timeout_secs(
        std::env::var("ILHAE_GEPA_SIDECAR_TIMEOUT_SECS")
            .ok()
            .as_deref(),
    )
}

pub(crate) fn gepa_sidecar_script_path() -> PathBuf {
    std::env::var("ILHAE_GEPA_SIDECAR_SCRIPT")
        .ok()
        .map(|value| PathBuf::from(value.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("scripts")
                .join("gepa_self_improvement_sidecar.py")
        })
}

fn resolve_self_improvement_followup_with_generator(
    settings: &crate::settings_types::Settings,
    finding: &SuperLoopFinding,
    dsrs_generator: impl FnOnce() -> Option<SelfImprovementFollowupSpec>,
) -> SelfImprovementFollowupSpec {
    let default_spec = default_self_improvement_followup_spec(finding);
    if self_improvement_uses_dsrs_runtime(settings) {
        return dsrs_generator().unwrap_or(default_spec);
    }

    if !self_improvement_uses_gepa_sidecar(settings) {
        return default_spec;
    }

    let runtime = &settings.agent.self_improvement_runtime;
    let approved_prompt = runtime
        .approved_prompt
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let approved_instructions = runtime
        .approved_instructions
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if approved_prompt.is_empty() || approved_instructions.is_empty() {
        return default_spec;
    }

    info!(
        subject = %finding.subject,
        approved_score = runtime.approved_score.unwrap_or_default(),
        approved_at = runtime.approved_at.unwrap_or_default(),
        "[SuperLoop] applied approved self-improvement follow-up"
    );

    SelfImprovementFollowupSpec {
        prompt: approved_prompt.to_string(),
        instructions: approved_instructions.to_string(),
    }
}

fn resolve_self_improvement_followup(
    settings: &crate::settings_types::Settings,
    finding: &SuperLoopFinding,
) -> SelfImprovementFollowupSpec {
    resolve_self_improvement_followup_with_generator(settings, finding, || {
        generate_dsrs_self_improvement_followup(
            finding,
            &default_self_improvement_followup_spec(finding),
        )
    })
}

pub(crate) fn run_gepa_self_improvement_sidecar(
    request: &GepaSidecarRequest,
) -> Result<GepaSidecarResponse, String> {
    let script_path = gepa_sidecar_script_path();
    if !script_path.exists() {
        return Err(format!(
            "GEPA sidecar script not found at {}",
            script_path.display()
        ));
    }

    let python = ["python3", "python"]
        .into_iter()
        .find(|binary| {
            std::process::Command::new(binary)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        })
        .ok_or_else(|| "python interpreter not found (tried python3, python)".to_string())?;

    let mut child = std::process::Command::new(python)
        .arg(&script_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to spawn GEPA sidecar: {error}"))?;

    let request_bytes = serde_json::to_vec(request)
        .map_err(|error| format!("failed to encode request: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&request_bytes)
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|error| format!("failed to send request to GEPA sidecar: {error}"))?;
    } else {
        return Err("failed to open stdin for GEPA sidecar".to_string());
    }

    let timeout = Duration::from_secs(gepa_sidecar_timeout_secs());
    let started = Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|error| format!("failed to poll GEPA sidecar: {error}"))?
        {
            Some(_) => break,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let output = child.wait_with_output().map_err(|error| {
                    format!("failed to collect timed out GEPA sidecar output: {error}")
                })?;
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let detail = if stderr.is_empty() {
                    format!("timed out after {}s", timeout.as_secs())
                } else {
                    format!("timed out after {}s: {stderr}", timeout.as_secs())
                };
                return Err(format!("GEPA sidecar returned error: {detail}"));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for GEPA sidecar: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stderr
        };
        return Err(format!("GEPA sidecar returned error: {detail}"));
    }

    serde_json::from_slice::<GepaSidecarResponse>(&output.stdout)
        .map_err(|error| format!("failed to decode GEPA sidecar response: {error}"))
}

fn append_preferred_roles_hint(
    instructions: Option<&str>,
    preferred_roles: Option<&[&str]>,
) -> Option<String> {
    let preferred = preferred_roles
        .unwrap_or(&[])
        .iter()
        .map(|role| role.trim().to_ascii_lowercase())
        .filter(|role| !role.is_empty())
        .collect::<Vec<_>>();
    if preferred.is_empty() {
        return instructions.map(|value| value.to_string());
    }
    let marker = format!("[ilhae:preferred_roles={}]", preferred.join(","));
    match instructions
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(existing) if existing.contains("[ilhae:preferred_roles=") => {
            Some(existing.to_string())
        }
        Some(existing) => Some(format!("{existing}\n{marker}")),
        None => Some(marker),
    }
}

fn first_preferred_role(preferred_roles: Option<&[&str]>) -> Option<String> {
    preferred_roles.and_then(|roles| {
        roles
            .iter()
            .map(|role| role.trim().to_ascii_lowercase())
            .find(|role| !role.is_empty())
    })
}

fn upsert_followup_task(
    brain: &BrainService,
    title: &str,
    description: &str,
    prompt: Option<&str>,
    instructions: Option<&str>,
    preferred_roles: Option<&[&str]>,
    category: &str,
    action_detail: &str,
    outcome: &mut SuperLoopOutcome,
) -> Result<SuperLoopAction, String> {
    let preferred_agent = first_preferred_role(preferred_roles);
    let instructions = append_preferred_roles_hint(instructions, preferred_roles);
    if let Some(existing) = brain
        .schedule_list()
        .into_iter()
        .find(|task| task.title == title)
    {
        let existing = if preferred_agent.is_some() || instructions.is_some() {
            brain
                .schedule_update_full(
                    &existing.id,
                    None,
                    Some(description),
                    None,
                    None,
                    None,
                    Some(category),
                    None,
                    prompt,
                    None,
                    None,
                    instructions.as_deref(),
                    None,
                    preferred_agent.as_deref(),
                    None,
                    None,
                    None,
                )
                .unwrap_or(existing)
        } else {
            existing
        };
        let _ = brain.schedule_add_history(
            &existing.id,
            "super_loop_refresh",
            Some(action_detail),
            None,
        );
        outcome.updated_tasks += 1;
        return Ok(SuperLoopAction {
            kind: SuperLoopActionKind::UpsertFollowupTask,
            target: existing.id,
            detail: action_detail.to_string(),
            status: "updated".to_string(),
            source_signature: None,
        });
    }

    let created = brain
        .schedule_create(
            title,
            Some(description),
            None,
            Some(category),
            Vec::new(),
            prompt,
            None,
            None,
            instructions.as_deref(),
            Some(true),
        )
        .map_err(|err| err.to_string())?;
    let created = if preferred_agent.is_some() || instructions.is_some() {
        brain
            .schedule_update_full(
                &created.id,
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
                instructions.as_deref(),
                None,
                preferred_agent.as_deref(),
                None,
                None,
                None,
            )
            .unwrap_or(created)
    } else {
        created
    };
    let _ =
        brain.schedule_add_history(&created.id, "super_loop_created", Some(action_detail), None);
    outcome.created_tasks += 1;
    Ok(SuperLoopAction {
        kind: SuperLoopActionKind::UpsertFollowupTask,
        target: created.id,
        detail: action_detail.to_string(),
        status: "created".to_string(),
        source_signature: None,
    })
}

fn evaluate_knowledge_findings(
    settings: &crate::settings_types::Settings,
) -> Vec<SuperLoopFinding> {
    if settings.agent.knowledge_mode == "off" {
        return Vec::new();
    }
    let runtime = &settings.agent.knowledge_runtime;
    let workspace_id = runtime
        .last_workspace_id
        .clone()
        .or_else(|| settings.agent.knowledge_workspace_id.clone())
        .unwrap_or_else(|| "default".to_string());

    let mut findings = Vec::new();
    if runtime.last_result == "error" {
        findings.push(SuperLoopFinding {
            kind: SuperLoopFindingKind::KnowledgeLoopError,
            subject: workspace_id.clone(),
            detail: runtime
                .last_error
                .clone()
                .unwrap_or_else(|| "knowledge loop failed".to_string()),
            severity: "high".to_string(),
            signature: format!(
                "kb:error:{workspace_id}:{}",
                runtime.last_error.clone().unwrap_or_default()
            ),
        });
    }
    if runtime.last_issue_count > 0 {
        findings.push(SuperLoopFinding {
            kind: SuperLoopFindingKind::KnowledgeGap,
            subject: workspace_id.clone(),
            detail: format!(
                "{} lint issues remain (report: {})",
                runtime.last_issue_count,
                runtime
                    .last_report_path
                    .clone()
                    .unwrap_or_else(|| "index/knowledge_loop_health.md".to_string())
            ),
            severity: "medium".to_string(),
            signature: format!("kb:issues:{workspace_id}:{}", runtime.last_issue_count),
        });
    }
    findings
}

fn evaluate_self_improvement_findings(
    settings: &crate::settings_types::Settings,
    brain: &BrainService,
) -> Vec<SuperLoopFinding> {
    let mut findings = Vec::new();
    if !settings.agent.self_improvement_enabled {
        return findings;
    }
    let Ok(preview) = brain.memory_dream_preview(3) else {
        return findings;
    };
    let group_count = preview
        .get("group_count")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as usize;
    if group_count > 0 {
        findings.push(SuperLoopFinding {
            kind: SuperLoopFindingKind::SelfImprovementCandidate,
            subject: settings
                .agent
                .active_profile
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            detail: format!(
                "{} dream groups pending under preset {}",
                group_count, settings.agent.self_improvement_preset
            ),
            severity: "medium".to_string(),
            signature: format!(
                "improve:{}:{}",
                settings.agent.self_improvement_preset, group_count
            ),
        });
    }
    let ignored_count = settings.agent.hygiene_runtime.ignored_dream_candidates;
    if ignored_count > 0 {
        let subject = settings
            .agent
            .active_profile
            .clone()
            .unwrap_or_else(|| "default".to_string());
        findings.push(SuperLoopFinding {
            kind: SuperLoopFindingKind::IgnoredDreamReviewCandidate,
            subject,
            detail: format!(
                "{} ignored dream groups should be reviewed before they go stale",
                ignored_count
            ),
            severity: "medium".to_string(),
            signature: format!("ignored_dream_review:{}", ignored_count),
        });
    }
    findings
}

fn evaluate_autonomous_findings(
    autonomous_sessions: &Arc<Cache<String, AutonomousSessionState>>,
) -> Vec<SuperLoopFinding> {
    let mut findings = Vec::new();
    for (session_id, snapshot) in autonomous_sessions.iter() {
        let session_id = session_id.to_string();
        let phase = snapshot.phase.clone();
        let stop_reason = snapshot.stop_reason.clone().unwrap_or_default();
        let blocked = matches!(phase, AutonomousPhase::Failed | AutonomousPhase::Cancelled)
            || stop_reason.contains("blocked")
            || stop_reason.contains("stall")
            || stop_reason.contains("budget")
            || stop_reason.contains("approval");
        if !blocked {
            continue;
        }
        findings.push(SuperLoopFinding {
            kind: SuperLoopFindingKind::ExecutionBlocked,
            subject: session_id.clone(),
            detail: if stop_reason.trim().is_empty() {
                format!("autonomous phase {:?}", phase)
            } else {
                format!("autonomous stop reason: {stop_reason}")
            },
            severity: "high".to_string(),
            signature: format!("exec:{session_id}:{phase:?}:{stop_reason}"),
        });
    }
    findings
}

fn evaluate_findings(
    settings: &crate::settings_types::Settings,
    brain: &BrainService,
    autonomous_sessions: &Arc<Cache<String, AutonomousSessionState>>,
) -> Vec<SuperLoopFinding> {
    let mut findings = Vec::new();
    findings.extend(evaluate_knowledge_findings(settings));
    findings.extend(evaluate_self_improvement_findings(settings, brain));
    findings.extend(evaluate_autonomous_findings(autonomous_sessions));
    if findings.is_empty() {
        findings.push(SuperLoopFinding {
            kind: SuperLoopFindingKind::Idle,
            subject: "super-loop".to_string(),
            detail: "no actionable findings".to_string(),
            severity: "info".to_string(),
            signature: "idle".to_string(),
        });
    }
    findings
}

fn maybe_run_followup_task(
    driver: SuperLoopDriver,
    brain: &BrainService,
    action: &SuperLoopAction,
) -> Result<Option<SuperLoopAction>, String> {
    if driver != SuperLoopDriver::Worker {
        return Ok(None);
    }
    if action.kind != SuperLoopActionKind::UpsertFollowupTask {
        return Ok(None);
    }
    if action.target.trim().is_empty() {
        return Ok(None);
    }
    let triggers = brain.schedule_run_with_scope(None, Some(action.target.as_str()));
    let started = !triggers.is_empty();
    Ok(Some(SuperLoopAction {
        kind: SuperLoopActionKind::RunFollowupTask,
        target: action.target.clone(),
        detail: format!("execute follow-up task: {}", action.detail),
        status: if started { "started" } else { "noop" }.to_string(),
        source_signature: action.source_signature.clone(),
    }))
}

fn previous_score_for_signature<'a>(
    state: &'a SuperLoopState,
    signature: &str,
) -> Option<&'a SuperLoopTaskScore> {
    state
        .last_scores
        .iter()
        .find(|score| score.source_signature.as_deref() == Some(signature))
}

fn score_followup_tasks(
    brain: &BrainService,
    actions: &[SuperLoopAction],
) -> Vec<SuperLoopTaskScore> {
    let tasks = brain.schedule_list();
    let task_ids = actions
        .iter()
        .filter(|action| {
            matches!(
                action.kind,
                SuperLoopActionKind::UpsertFollowupTask | SuperLoopActionKind::RunFollowupTask
            )
        })
        .map(|action| action.target.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let started_ids = actions
        .iter()
        .filter(|action| {
            action.kind == SuperLoopActionKind::RunFollowupTask && action.status == "started"
        })
        .map(|action| action.target.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let signature_by_task_id = actions
        .iter()
        .filter_map(|action| {
            action
                .source_signature
                .as_ref()
                .map(|signature| (action.target.clone(), signature.clone()))
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    task_ids
        .into_iter()
        .map(|task_id| {
            let source_signature = signature_by_task_id.get(&task_id).cloned();
            let Some(task) = tasks.iter().find(|task| task.id == task_id) else {
                return SuperLoopTaskScore {
                    task_id,
                    title: None,
                    resolution: SuperLoopResolution::Stale,
                    detail: "follow-up task missing from schedule store".to_string(),
                    source_signature,
                };
            };

            let last_run_status = task
                .last_run_status
                .clone()
                .unwrap_or_default()
                .to_lowercase();
            let status = task.status.to_lowercase();
            let done = task.done
                || matches!(status.as_str(), "done" | "completed" | "complete")
                || last_run_status.contains("success")
                || last_run_status.contains("completed")
                || last_run_status.contains("resolved");
            if done {
                return SuperLoopTaskScore {
                    task_id,
                    title: Some(task.title.clone()),
                    resolution: SuperLoopResolution::Resolved,
                    detail: format!(
                        "status={}, last_run_status={}",
                        task.status,
                        task.last_run_status.as_deref().unwrap_or("none")
                    ),
                    source_signature,
                };
            }

            let failed = last_run_status.contains("fail")
                || last_run_status.contains("error")
                || last_run_status.contains("blocked")
                || matches!(status.as_str(), "failed" | "error" | "blocked");
            if failed {
                let resolution = if task.retry_count < task.max_retries {
                    SuperLoopResolution::Retry
                } else {
                    SuperLoopResolution::Escalated
                };
                return SuperLoopTaskScore {
                    task_id,
                    title: Some(task.title.clone()),
                    resolution,
                    detail: format!(
                        "status={}, last_run_status={}, retries={}/{}",
                        task.status,
                        task.last_run_status.as_deref().unwrap_or("none"),
                        task.retry_count,
                        task.max_retries
                    ),
                    source_signature,
                };
            }

            let resolution = if started_ids.contains(task.id.as_str()) {
                SuperLoopResolution::Running
            } else {
                SuperLoopResolution::Planned
            };
            SuperLoopTaskScore {
                task_id,
                title: Some(task.title.clone()),
                resolution,
                detail: format!(
                    "status={}, last_run_status={}",
                    task.status,
                    task.last_run_status.as_deref().unwrap_or("none")
                ),
                source_signature,
            }
        })
        .collect()
}

fn summarize_outcome(
    findings: &[SuperLoopFinding],
    actions: &[SuperLoopAction],
    scores: &[SuperLoopTaskScore],
) -> SuperLoopOutcome {
    let mut outcome = SuperLoopOutcome {
        findings_count: findings.len(),
        actions_count: actions.len(),
        created_tasks: actions
            .iter()
            .filter(|action| action.status == "created")
            .count(),
        updated_tasks: actions
            .iter()
            .filter(|action| action.status == "updated")
            .count(),
        ..Default::default()
    };
    for score in scores {
        match score.resolution {
            SuperLoopResolution::Resolved => outcome.resolved_tasks += 1,
            SuperLoopResolution::Running => outcome.running_tasks += 1,
            SuperLoopResolution::Retry => outcome.retry_tasks += 1,
            SuperLoopResolution::Escalated => outcome.escalated_tasks += 1,
            SuperLoopResolution::Stale => outcome.stale_tasks += 1,
            SuperLoopResolution::Planned => outcome.planned_tasks += 1,
        }
    }
    outcome
}

fn apply_resolved_cleanup(
    driver: SuperLoopDriver,
    scores: &[SuperLoopTaskScore],
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    ilhae_dir: PathBuf,
) -> Vec<SuperLoopAction> {
    if driver != SuperLoopDriver::Worker {
        return Vec::new();
    }

    let mut needs_knowledge_cleanup = false;
    let mut needs_improvement_cleanup = false;
    for score in scores {
        if score.resolution != SuperLoopResolution::Resolved {
            continue;
        }
        let Some(signature) = score.source_signature.as_deref() else {
            continue;
        };
        if signature.starts_with("kb:") {
            needs_knowledge_cleanup = true;
        } else if signature.starts_with("improve:")
            || signature.starts_with("ignored_dream_review:")
        {
            needs_improvement_cleanup = true;
        }
    }

    let mut actions = Vec::new();
    if needs_knowledge_cleanup {
        match crate::knowledge_loop::run_cleanup_cycle_now(
            settings_store.clone(),
            ilhae_dir.clone(),
        ) {
            Ok(true) => actions.push(SuperLoopAction {
                kind: SuperLoopActionKind::RunKnowledgeCycle,
                target: "knowledge-loop".to_string(),
                detail: "post-resolution knowledge cleanup completed".to_string(),
                status: "completed".to_string(),
                source_signature: None,
            }),
            Ok(false) => actions.push(SuperLoopAction {
                kind: SuperLoopActionKind::RunKnowledgeCycle,
                target: "knowledge-loop".to_string(),
                detail:
                    "post-resolution knowledge cleanup skipped because knowledge loop is disabled"
                        .to_string(),
                status: "skipped".to_string(),
                source_signature: None,
            }),
            Err(err) => actions.push(SuperLoopAction {
                kind: SuperLoopActionKind::RunKnowledgeCycle,
                target: "knowledge-loop".to_string(),
                detail: err,
                status: "error".to_string(),
                source_signature: None,
            }),
        }
    }
    if needs_improvement_cleanup {
        match crate::hygiene_loop::run_embedded_cycle(
            crate::hygiene_loop::HygieneLoopDriver::Worker,
            brain,
            settings_store,
            ilhae_dir,
        ) {
            Ok(outcome) => actions.push(SuperLoopAction {
                kind: SuperLoopActionKind::RunHygieneCycle,
                target: "hygiene-loop".to_string(),
                detail: format!(
                    "post-resolution improvement cleanup completed (memory_reports={}, ignored_dream_candidates={})",
                    outcome.memory_reports_written, outcome.ignored_dream_candidates
                ),
                status: if outcome.memory_reports_written > 0 || outcome.ignored_dream_candidates > 0 {
                    "changed".to_string()
                } else {
                    "completed".to_string()
                },
                source_signature: None,
            }),
            Err(err) => actions.push(SuperLoopAction {
                kind: SuperLoopActionKind::RunHygieneCycle,
                target: "hygiene-loop".to_string(),
                detail: err,
                status: "error".to_string(),
                source_signature: None,
            }),
        }
    }
    actions
}

fn execute_plan(
    driver: SuperLoopDriver,
    brain: Arc<BrainService>,
    settings: &crate::settings_types::Settings,
    state: &SuperLoopState,
    findings: &[SuperLoopFinding],
) -> Result<Vec<SuperLoopAction>, String> {
    let mut actions = Vec::new();
    let mut outcome = SuperLoopOutcome {
        findings_count: findings.len(),
        ..Default::default()
    };

    for finding in findings {
        if let Some(previous) = previous_score_for_signature(state, &finding.signature) {
            match previous.resolution {
                SuperLoopResolution::Running | SuperLoopResolution::Planned => {
                    actions.push(SuperLoopAction {
                        kind: SuperLoopActionKind::Noop,
                        target: previous.task_id.clone(),
                        detail: format!(
                            "skip duplicate follow-up while existing task is {}",
                            match previous.resolution {
                                SuperLoopResolution::Running => "running",
                                SuperLoopResolution::Planned => "planned",
                                _ => "active",
                            }
                        ),
                        status: "skipped".to_string(),
                        source_signature: Some(finding.signature.clone()),
                    });
                    continue;
                }
                SuperLoopResolution::Escalated => {
                    actions.push(SuperLoopAction {
                        kind: SuperLoopActionKind::Noop,
                        target: previous.task_id.clone(),
                        detail: "skip duplicate follow-up while escalated review is pending"
                            .to_string(),
                        status: "skipped".to_string(),
                        source_signature: Some(finding.signature.clone()),
                    });
                    continue;
                }
                SuperLoopResolution::Retry => {
                    let title = make_followup_title("advisor", &finding.subject);
                    let description = format!(
                        "Repeated super-loop follow-up retry for `{}`.\n\nCurrent issue: {}\nPrevious outcome: {}",
                        finding.subject, finding.detail, previous.detail
                    );
                    let prompt = "Review the repeated super-loop failure, decide whether to change approach, reassign work, or stop safely.";
                    let instructions = "Inspect the prior follow-up task history and propose the next safe action. Prefer explanation and escalation over repeating the same step.";
                    let followup = upsert_followup_task(
                        &brain,
                        &title,
                        &description,
                        Some(prompt),
                        Some(instructions),
                        Some(&["reviewer", "planner", "leader"]),
                        "advisor",
                        &finding.detail,
                        &mut outcome,
                    )?;
                    let followup = SuperLoopAction {
                        source_signature: Some(finding.signature.clone()),
                        ..followup
                    };
                    if let Some(run_action) = maybe_run_followup_task(driver, &brain, &followup)? {
                        actions.push(run_action);
                    }
                    actions.push(followup);
                    continue;
                }
                SuperLoopResolution::Resolved | SuperLoopResolution::Stale => {}
            }
        }

        match finding.kind {
            SuperLoopFindingKind::KnowledgeGap | SuperLoopFindingKind::KnowledgeLoopError => {
                actions.push(SuperLoopAction {
                    kind: SuperLoopActionKind::RunKnowledgeCycle,
                    target: settings
                        .agent
                        .knowledge_workspace_id
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                    detail: "refresh knowledge workspace".to_string(),
                    status: "planned".to_string(),
                    source_signature: Some(finding.signature.clone()),
                });
                let title = make_followup_title("knowledge", &finding.subject);
                let description = format!(
                    "Knowledge compiler follow-up for workspace `{}`.\n\n{}",
                    finding.subject, finding.detail
                );
                let prompt = "Review the knowledge workspace, resolve lint issues, and file back the result.";
                let instructions = "Use kb_query, kb_lint, kb_compile, and kb_file_back as needed. Leave a concise summary in task history.";
                let followup = upsert_followup_task(
                    &brain,
                    &title,
                    &description,
                    Some(prompt),
                    Some(instructions),
                    Some(&["researcher", "reviewer"]),
                    "knowledge",
                    &finding.detail,
                    &mut outcome,
                )?;
                let followup = SuperLoopAction {
                    source_signature: Some(finding.signature.clone()),
                    ..followup
                };
                if let Some(run_action) = maybe_run_followup_task(driver, &brain, &followup)? {
                    actions.push(run_action);
                }
                actions.push(followup);
            }
            SuperLoopFindingKind::SelfImprovementCandidate => {
                let followup_spec = resolve_self_improvement_followup(settings, finding);
                let title = make_followup_title("improvement", &finding.subject);
                let description = format!(
                    "Self-improvement review needed for profile `{}`.\n\n{}",
                    finding.subject, finding.detail
                );
                let followup = upsert_followup_task(
                    &brain,
                    &title,
                    &description,
                    Some(followup_spec.prompt.as_str()),
                    Some(followup_spec.instructions.as_str()),
                    Some(&["reviewer", "researcher"]),
                    "improvement",
                    &finding.detail,
                    &mut outcome,
                )?;
                let followup = SuperLoopAction {
                    source_signature: Some(finding.signature.clone()),
                    ..followup
                };
                if let Some(run_action) = maybe_run_followup_task(driver, &brain, &followup)? {
                    actions.push(run_action);
                }
                actions.push(followup);
            }
            SuperLoopFindingKind::IgnoredDreamReviewCandidate => {
                let title = make_followup_title("ignored-dream-review", &finding.subject);
                let description = format!(
                    "Ignored dream groups need review for profile `{}`.\n\n{}",
                    finding.subject, finding.detail
                );
                let prompt = "Review ignored dream groups, inspect the ignored preview, and requeue only the groups that are clearly safe to revisit. Leave everything else ignored or create explicit follow-up work.";
                let instructions = "Read index/memory_hygiene.md first. Use memory_dream_ignored_preview to inspect candidate groups. Only use memory_dream_requeue for low-risk groups that should go back to pending review immediately. Keep ambiguous groups ignored and record the reason.";
                let followup = upsert_followup_task(
                    &brain,
                    &title,
                    &description,
                    Some(prompt),
                    Some(instructions),
                    Some(&["reviewer", "planner"]),
                    "improvement",
                    &finding.detail,
                    &mut outcome,
                )?;
                let followup = SuperLoopAction {
                    source_signature: Some(finding.signature.clone()),
                    ..followup
                };
                if let Some(run_action) = maybe_run_followup_task(driver, &brain, &followup)? {
                    actions.push(run_action);
                }
                actions.push(followup);
            }
            SuperLoopFindingKind::ExecutionBlocked => {
                let title = make_followup_task("execution", &finding.subject);
                let description = format!(
                    "Autonomous execution follow-up for session `{}`.\n\n{}",
                    finding.subject, finding.detail
                );
                let prompt = "Investigate the blocked autonomous session, replan the next step, and continue only if safe.";
                let instructions = "Inspect the prior session outcome, identify the blocking reason, and create or update a concrete next task.";
                let followup = upsert_followup_task(
                    &brain,
                    &title,
                    &description,
                    Some(prompt),
                    Some(instructions),
                    Some(&["planner", "leader"]),
                    "execution",
                    &finding.detail,
                    &mut outcome,
                )?;
                let followup = SuperLoopAction {
                    source_signature: Some(finding.signature.clone()),
                    ..followup
                };
                if let Some(run_action) = maybe_run_followup_task(driver, &brain, &followup)? {
                    actions.push(run_action);
                }
                actions.push(followup);
            }
            SuperLoopFindingKind::Idle => {
                actions.push(SuperLoopAction {
                    kind: SuperLoopActionKind::Noop,
                    target: "super-loop".to_string(),
                    detail: finding.detail.clone(),
                    status: "skipped".to_string(),
                    source_signature: Some(finding.signature.clone()),
                });
            }
        }
    }

    outcome.actions_count = actions.len();
    let _ = outcome;
    Ok(actions)
}

fn make_followup_task(prefix: &str, subject: &str) -> String {
    make_followup_title(prefix, subject)
}

fn run_cycle_blocking(
    driver: SuperLoopDriver,
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    autonomous_sessions: Arc<Cache<String, AutonomousSessionState>>,
    ilhae_dir: PathBuf,
) -> Result<(), String> {
    let started_at = Instant::now();
    let super_loop_item_id = loop_item_id(LoopLifecycleKind::SuperLoop, driver, now_millis());
    emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Started {
        session_id: "native-runtime".to_string(),
        item: loop_item(
            super_loop_item_id.clone(),
            LoopLifecycleKind::SuperLoop,
            "Running Super Loop",
            format!("Scanning background follow-ups ({})", driver.as_str()),
            None,
            LoopLifecycleStatus::InProgress,
            Some("cycle_started".to_string()),
            None,
            None,
            None,
        ),
    });

    let settings = settings_store.get();
    if !super_loop_enabled(&settings, &autonomous_sessions) {
        emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Completed {
            session_id: "native-runtime".to_string(),
            item: loop_item(
                super_loop_item_id,
                LoopLifecycleKind::SuperLoop,
                "Running Super Loop",
                "Super loop skipped".to_string(),
                Some("No enabled background loop inputs were detected".to_string()),
                LoopLifecycleStatus::Completed,
                Some("skipped_disabled".to_string()),
                None,
                None,
                Some(started_at.elapsed().as_millis() as i64),
            ),
        });
        return Ok(());
    }

    let mut state = read_state(&ilhae_dir);
    let findings = evaluate_findings(&settings, &brain, &autonomous_sessions);
    let findings_count = findings.len() as i64;
    let improvement_findings = findings
        .iter()
        .filter(|finding| {
            matches!(
                finding.kind,
                SuperLoopFindingKind::SelfImprovementCandidate
                    | SuperLoopFindingKind::IgnoredDreamReviewCandidate
            )
        })
        .count() as i64;
    let signature = build_signature(&findings);
    let now = now_secs();
    if should_skip(&state, &signature, now) {
        emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Completed {
            session_id: "native-runtime".to_string(),
            item: loop_item(
                super_loop_item_id,
                LoopLifecycleKind::SuperLoop,
                "Running Super Loop",
                "Super loop skipped".to_string(),
                Some("Cooldown active for the current finding signature".to_string()),
                LoopLifecycleStatus::Completed,
                Some("skipped_cooldown".to_string()),
                Some(BTreeMap::from([(
                    "findings".to_string(),
                    findings.len() as i64,
                )])),
                None,
                Some(started_at.elapsed().as_millis() as i64),
            ),
        });
        return Ok(());
    }

    emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Progress {
        session_id: "native-runtime".to_string(),
        item_id: super_loop_item_id.clone(),
        kind: LoopLifecycleKind::SuperLoop,
        summary: format!("Evaluated {findings_count} findings"),
        detail: Some(format!("driver={}, signature={signature}", driver.as_str())),
        counts: Some(BTreeMap::from([
            ("findings".to_string(), findings_count),
            ("improvement_findings".to_string(), improvement_findings),
        ])),
    });

    let improvement_item_id = (improvement_findings > 0).then(|| {
        loop_item_id(
            LoopLifecycleKind::ImprovementLoop,
            driver,
            now_millis().saturating_add(1),
        )
    });
    if let Some(improvement_item_id) = improvement_item_id.as_ref() {
        emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Started {
            session_id: "native-runtime".to_string(),
            item: loop_item(
                improvement_item_id.clone(),
                LoopLifecycleKind::ImprovementLoop,
                "Reviewing Improvement Loop",
                format!(
                    "Reviewing {} self-improvement findings",
                    improvement_findings
                ),
                None,
                LoopLifecycleStatus::InProgress,
                Some("improvement_review_started".to_string()),
                Some(BTreeMap::from([(
                    "findings".to_string(),
                    improvement_findings,
                )])),
                None,
                None,
            ),
        });
    }

    match execute_plan(driver, brain.clone(), &settings, &state, &findings) {
        Ok(mut actions) => {
            let hygiene_driver = match driver {
                SuperLoopDriver::Worker => crate::hygiene_loop::HygieneLoopDriver::Worker,
                SuperLoopDriver::Kairos => crate::hygiene_loop::HygieneLoopDriver::Kairos,
            };
            match crate::hygiene_loop::run_embedded_cycle(
                hygiene_driver,
                brain.clone(),
                settings_store.clone(),
                ilhae_dir.clone(),
            ) {
                Ok(hygiene_outcome) => {
                    if hygiene_outcome.duplicate_tasks_folded > 0
                        || hygiene_outcome.legacy_commands_normalized > 0
                        || hygiene_outcome.knowledge_reports_written > 0
                        || hygiene_outcome.memory_reports_written > 0
                    {
                        actions.push(SuperLoopAction {
                            kind: SuperLoopActionKind::RunHygieneCycle,
                            target: "hygiene-loop".to_string(),
                            detail: format!(
                                "safe cleanup applied: folded {} duplicate tasks, normalized {} legacy commands, wrote {} knowledge reports, wrote {} memory reports",
                                hygiene_outcome.duplicate_tasks_folded,
                                hygiene_outcome.legacy_commands_normalized,
                                hygiene_outcome.knowledge_reports_written,
                                hygiene_outcome.memory_reports_written,
                            ),
                            status: "changed".to_string(),
                            source_signature: None,
                        });
                    }
                }
                Err(err) => {
                    actions.push(SuperLoopAction {
                        kind: SuperLoopActionKind::RunHygieneCycle,
                        target: "hygiene-loop".to_string(),
                        detail: err,
                        status: "error".to_string(),
                        source_signature: None,
                    });
                }
            }
            state.version = state.version.saturating_add(1);
            state.last_run_at = Some(now);
            state.last_driver = Some(driver.as_str().to_string());
            state.last_signature = Some(signature);
            state.last_findings = findings.clone();
            let scores = score_followup_tasks(&brain, &actions);
            let cleanup_actions = apply_resolved_cleanup(
                driver,
                &scores,
                brain.clone(),
                settings_store.clone(),
                ilhae_dir.clone(),
            );
            actions.extend(cleanup_actions);
            state.last_actions = actions.clone();
            state.last_scores = scores.clone();
            state.last_outcome = summarize_outcome(&findings, &actions, &scores);
            state.last_error = None;
            write_state(&ilhae_dir, &state).map_err(|err| err.to_string())?;
            let elapsed_ms = started_at.elapsed().as_millis() as i64;
            if let Some(improvement_item_id) = improvement_item_id {
                let improvement_actions = actions
                    .iter()
                    .filter(|action| {
                        action.target.contains("improvement") || action.target.contains("dream")
                    })
                    .count() as i64;
                emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Completed {
                    session_id: "native-runtime".to_string(),
                    item: loop_item(
                        improvement_item_id,
                        LoopLifecycleKind::ImprovementLoop,
                        "Reviewing Improvement Loop",
                        if improvement_actions > 0 {
                            format!("Applied {improvement_actions} improvement follow-ups")
                        } else {
                            "Improvement review completed".to_string()
                        },
                        Some(
                            "Self-improvement findings were folded into the super-loop plan"
                                .to_string(),
                        ),
                        LoopLifecycleStatus::Completed,
                        Some("improvement_review_completed".to_string()),
                        Some(BTreeMap::from([
                            ("findings".to_string(), improvement_findings),
                            ("actions".to_string(), improvement_actions),
                        ])),
                        None,
                        Some(elapsed_ms),
                    ),
                });
            }
            emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Completed {
                session_id: "native-runtime".to_string(),
                item: loop_item(
                    super_loop_item_id,
                    LoopLifecycleKind::SuperLoop,
                    "Running Super Loop",
                    "Super loop completed".to_string(),
                    Some(format!(
                        "planned {} actions from {} findings",
                        actions.len(),
                        findings_count
                    )),
                    LoopLifecycleStatus::Completed,
                    Some("cycle_completed".to_string()),
                    Some(BTreeMap::from([
                        ("findings".to_string(), findings_count),
                        ("actions".to_string(), actions.len() as i64),
                        (
                            "resolved_tasks".to_string(),
                            state.last_outcome.resolved_tasks as i64,
                        ),
                        (
                            "retry_tasks".to_string(),
                            state.last_outcome.retry_tasks as i64,
                        ),
                        (
                            "escalated_tasks".to_string(),
                            state.last_outcome.escalated_tasks as i64,
                        ),
                    ])),
                    None,
                    Some(elapsed_ms),
                ),
            });
            info!(
                driver = driver.as_str(),
                findings = findings_count,
                actions = actions.len(),
                resolved = state.last_outcome.resolved_tasks,
                running = state.last_outcome.running_tasks,
                retry = state.last_outcome.retry_tasks,
                escalated = state.last_outcome.escalated_tasks,
                "[SuperLoop] completed evaluation and follow-up planning"
            );
            Ok(())
        }
        Err(err) => {
            state.version = state.version.saturating_add(1);
            state.last_run_at = Some(now);
            state.last_driver = Some(driver.as_str().to_string());
            state.last_signature = Some(signature);
            state.last_findings = findings;
            state.last_actions.clear();
            state.last_scores.clear();
            state.last_outcome = SuperLoopOutcome::default();
            state.last_error = Some(err.clone());
            let _ = write_state(&ilhae_dir, &state);
            let elapsed_ms = started_at.elapsed().as_millis() as i64;
            if let Some(improvement_item_id) = improvement_item_id {
                emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Failed {
                    session_id: "native-runtime".to_string(),
                    item: loop_item(
                        improvement_item_id,
                        LoopLifecycleKind::ImprovementLoop,
                        "Reviewing Improvement Loop",
                        "Improvement review failed".to_string(),
                        None,
                        LoopLifecycleStatus::Failed,
                        Some("improvement_review_failed".to_string()),
                        Some(BTreeMap::from([(
                            "findings".to_string(),
                            improvement_findings,
                        )])),
                        Some(err.clone()),
                        Some(elapsed_ms),
                    ),
                });
            }
            emit_loop_notification(crate::IlhaeLoopLifecycleNotification::Failed {
                session_id: "native-runtime".to_string(),
                item: loop_item(
                    super_loop_item_id,
                    LoopLifecycleKind::SuperLoop,
                    "Running Super Loop",
                    "Super loop failed".to_string(),
                    Some(format!("driver={}", driver.as_str())),
                    LoopLifecycleStatus::Failed,
                    Some("cycle_failed".to_string()),
                    Some(BTreeMap::from([("findings".to_string(), findings_count)])),
                    Some(err.clone()),
                    Some(elapsed_ms),
                ),
            });
            Err(err)
        }
    }
}

pub async fn maybe_run_cycle(
    driver: SuperLoopDriver,
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    autonomous_sessions: Arc<Cache<String, AutonomousSessionState>>,
    ilhae_dir: PathBuf,
) {
    let result = tokio::task::spawn_blocking(move || {
        run_cycle_blocking(
            driver,
            brain,
            settings_store,
            autonomous_sessions,
            ilhae_dir,
        )
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => warn!(driver = driver.as_str(), error = %err, "[SuperLoop] cycle failed"),
        Err(err) => warn!(driver = driver.as_str(), error = %err, "[SuperLoop] worker join failed"),
    }
}

pub async fn run_worker_loop(
    brain: Arc<BrainService>,
    settings_store: Arc<SettingsStore>,
    autonomous_sessions: Arc<Cache<String, AutonomousSessionState>>,
    ilhae_dir: PathBuf,
) {
    info!("[SuperLoop] worker loop started");
    tokio::time::sleep(Duration::from_secs(45)).await;
    loop {
        maybe_run_cycle(
            SuperLoopDriver::Worker,
            brain.clone(),
            settings_store.clone(),
            autonomous_sessions.clone(),
            ilhae_dir.clone(),
        )
        .await;
        tokio::time::sleep(Duration::from_secs(DEFAULT_SUPER_LOOP_POLL_INTERVAL_SECS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn self_improvement_finding() -> SuperLoopFinding {
        SuperLoopFinding {
            kind: SuperLoopFindingKind::SelfImprovementCandidate,
            subject: "default".to_string(),
            detail: "3 dream groups pending under preset gepa_sidecar".to_string(),
            severity: "medium".to_string(),
            signature: "improve:gepa_sidecar:3".to_string(),
        }
    }

    #[test]
    fn gepa_sidecar_preset_uses_approved_runtime_prompt_when_available() {
        let mut settings = crate::settings_types::Settings::default();
        settings.agent.self_improvement_enabled = true;
        settings.agent.self_improvement_preset = "gepa_sidecar".to_string();
        settings.agent.self_improvement_runtime.approved_prompt =
            Some("Approved prompt".to_string());
        settings
            .agent
            .self_improvement_runtime
            .approved_instructions = Some("Approved instructions".to_string());
        settings.agent.self_improvement_runtime.approved_score = Some(0.9);
        let finding = self_improvement_finding();
        let default_spec = default_self_improvement_followup_spec(&finding);

        let optimized = resolve_self_improvement_followup(&settings, &finding);

        assert_eq!(optimized.prompt, "Approved prompt");
        assert_eq!(optimized.instructions, "Approved instructions");
        assert_ne!(optimized.prompt, default_spec.prompt);
    }

    #[test]
    fn gepa_sidecar_preset_falls_back_to_default_without_approved_runtime_prompt() {
        let mut settings = crate::settings_types::Settings::default();
        settings.agent.self_improvement_enabled = true;
        settings.agent.self_improvement_preset = "gepa_sidecar".to_string();
        let finding = self_improvement_finding();
        let default_spec = default_self_improvement_followup_spec(&finding);

        let optimized = resolve_self_improvement_followup(&settings, &finding);

        assert_eq!(optimized.prompt, default_spec.prompt);
        assert_eq!(optimized.instructions, default_spec.instructions);
    }

    #[test]
    fn non_gepa_preset_skips_sidecar_and_keeps_default_followup() {
        let mut settings = crate::settings_types::Settings::default();
        settings.agent.self_improvement_enabled = true;
        settings.agent.self_improvement_preset = "safe_summarize".to_string();
        let finding = self_improvement_finding();
        let default_spec = default_self_improvement_followup_spec(&finding);
        settings.agent.self_improvement_runtime.approved_prompt =
            Some("Should not be used".to_string());
        settings
            .agent
            .self_improvement_runtime
            .approved_instructions = Some("Should not be used".to_string());

        let optimized = resolve_self_improvement_followup(&settings, &finding);

        assert_eq!(optimized.prompt, default_spec.prompt);
        assert_eq!(optimized.instructions, default_spec.instructions);
    }

    #[test]
    fn dsrs_runtime_preset_uses_generated_followup_when_available() {
        let mut settings = crate::settings_types::Settings::default();
        settings.agent.self_improvement_enabled = true;
        settings.agent.self_improvement_preset = "dsrs_runtime".to_string();
        let finding = self_improvement_finding();

        let optimized =
            resolve_self_improvement_followup_with_generator(&settings, &finding, || {
                Some(SelfImprovementFollowupSpec {
                    prompt: "Generated prompt".to_string(),
                    instructions: "Generated instructions".to_string(),
                })
            });

        assert_eq!(optimized.prompt, "Generated prompt");
        assert_eq!(optimized.instructions, "Generated instructions");
    }

    #[test]
    fn dsrs_runtime_preset_falls_back_to_default_when_generator_returns_none() {
        let mut settings = crate::settings_types::Settings::default();
        settings.agent.self_improvement_enabled = true;
        settings.agent.self_improvement_preset = "dsrs_runtime".to_string();
        let finding = self_improvement_finding();
        let default_spec = default_self_improvement_followup_spec(&finding);

        let optimized =
            resolve_self_improvement_followup_with_generator(&settings, &finding, || None);

        assert_eq!(optimized.prompt, default_spec.prompt);
        assert_eq!(optimized.instructions, default_spec.instructions);
    }

    #[test]
    fn dsrs_runtime_preset_falls_back_to_default_without_model_configuration() {
        unsafe {
            std::env::remove_var("ILHAE_DSRS_SELF_IMPROVEMENT_MODEL");
            std::env::remove_var("ILHAE_DSRS_SELF_IMPROVEMENT_BASE_URL");
            std::env::remove_var("ILHAE_DSRS_SELF_IMPROVEMENT_API_KEY");
        }

        let mut settings = crate::settings_types::Settings::default();
        settings.agent.self_improvement_enabled = true;
        settings.agent.self_improvement_preset = "dsrs_runtime".to_string();
        let finding = self_improvement_finding();
        let default_spec = default_self_improvement_followup_spec(&finding);

        let optimized = resolve_self_improvement_followup(&settings, &finding);

        assert_eq!(optimized.prompt, default_spec.prompt);
        assert_eq!(optimized.instructions, default_spec.instructions);
    }

    #[test]
    fn parse_gepa_sidecar_timeout_uses_default_for_missing_or_invalid_values() {
        assert_eq!(
            parse_gepa_sidecar_timeout_secs(None),
            DEFAULT_GEPA_SIDECAR_TIMEOUT_SECS
        );
        assert_eq!(
            parse_gepa_sidecar_timeout_secs(Some("")),
            DEFAULT_GEPA_SIDECAR_TIMEOUT_SECS
        );
        assert_eq!(
            parse_gepa_sidecar_timeout_secs(Some("0")),
            DEFAULT_GEPA_SIDECAR_TIMEOUT_SECS
        );
        assert_eq!(
            parse_gepa_sidecar_timeout_secs(Some("abc")),
            DEFAULT_GEPA_SIDECAR_TIMEOUT_SECS
        );
    }

    #[test]
    fn parse_gepa_sidecar_timeout_honors_positive_override() {
        assert_eq!(parse_gepa_sidecar_timeout_secs(Some("7")), 7);
    }

    #[test]
    #[ignore = "requires local LLM server at 127.0.0.1:8081"]
    fn dsrs_runtime_live_generation_smoke_test() {
        unsafe {
            std::env::set_var(
                "ILHAE_DSRS_SELF_IMPROVEMENT_MODEL",
                "nvidia_Nemotron-Cascade-2-30B-A3B-IQ4_XS.gguf",
            );
            std::env::set_var(
                "ILHAE_DSRS_SELF_IMPROVEMENT_BASE_URL",
                "http://127.0.0.1:8081/v1",
            );
            std::env::remove_var("ILHAE_DSRS_SELF_IMPROVEMENT_API_KEY");
            std::env::set_var("ILHAE_DSRS_SELF_IMPROVEMENT_MAX_TOKENS", "128");
            std::env::set_var("ILHAE_DSRS_SELF_IMPROVEMENT_TEMPERATURE", "0.3");
        }

        let finding = self_improvement_finding();
        let default_spec = default_self_improvement_followup_spec(&finding);

        eprintln!("[smoke] calling generate_dsrs_self_improvement_followup ...");
        let result = generate_dsrs_self_improvement_followup(&finding, &default_spec);

        match result {
            Some(spec) => {
                eprintln!("[smoke] ✅ generated prompt: {}", spec.prompt);
                eprintln!("[smoke] ✅ generated instructions: {}", spec.instructions);
                assert!(!spec.prompt.is_empty(), "prompt should not be empty");
                assert!(
                    !spec.instructions.is_empty(),
                    "instructions should not be empty"
                );
            }
            None => {
                panic!(
                    "[smoke] ❌ generation returned None — check local LLM server at 127.0.0.1:8081"
                );
            }
        }
    }
}
