// Imports used inside macro expansion — suppress unused warnings.
#[allow(unused_imports)]
use sacp::{Client, Conductor, ConnectionTo, Responder};
#[allow(unused_imports)]
use std::collections::{BTreeMap, BTreeSet};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use tracing::{info, warn};

use serde::{Deserialize, Serialize};
use std::fs;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct KbWorkspaceRegistry {
    #[serde(default)]
    pub(crate) active_workspace: Option<String>,
    #[serde(default)]
    pub(crate) workspaces: Vec<KbWorkspaceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KbWorkspaceEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) root_path: String,
}

pub(crate) fn registry_path(ilhae_dir: &Path) -> PathBuf {
    ilhae_dir.join("kb_workspaces.json")
}

pub(crate) fn load_registry(ilhae_dir: &Path) -> Result<KbWorkspaceRegistry, std::io::Error> {
    let path = registry_path(ilhae_dir);
    if !path.exists() {
        return Ok(KbWorkspaceRegistry::default());
    }
    let body = fs::read_to_string(path)?;
    serde_json::from_str(&body)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

pub(crate) fn save_registry(
    ilhae_dir: &Path,
    registry: &KbWorkspaceRegistry,
) -> Result<(), std::io::Error> {
    let path = registry_path(ilhae_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(registry)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    fs::write(path, body)
}

pub(crate) fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_dash = false;
    for ch in value.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            last_dash = false;
            ch.to_ascii_lowercase()
        } else {
            if last_dash {
                continue;
            }
            last_dash = true;
            '-'
        };
        out.push(mapped);
    }
    out.trim_matches('-').to_string()
}

fn timestamp_string(time: std::time::SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn workspace_dirs(root: &Path) -> [PathBuf; 4] {
    [
        root.join("raw"),
        root.join("wiki"),
        root.join("output"),
        root.join("index"),
    ]
}

pub(crate) fn ensure_workspace_dirs(root: &Path) -> Result<(), std::io::Error> {
    for dir in workspace_dirs(root) {
        fs::create_dir_all(dir)?;
    }
    fs::create_dir_all(root.join("wiki").join("sources"))?;
    fs::create_dir_all(root.join("wiki").join("concepts"))?;
    Ok(())
}

pub(crate) fn workspace_to_dto(
    entry: &KbWorkspaceEntry,
    active_workspace: Option<&str>,
) -> crate::IlhaeAppKbWorkspaceDto {
    crate::IlhaeAppKbWorkspaceDto {
        id: entry.id.clone(),
        name: entry.name.clone(),
        root_path: entry.root_path.clone(),
        active: active_workspace == Some(entry.id.as_str()),
    }
}

pub(crate) fn resolve_workspace(
    ilhae_dir: &Path,
    workspace_id: Option<&str>,
) -> Result<(KbWorkspaceRegistry, KbWorkspaceEntry), std::io::Error> {
    let registry = load_registry(ilhae_dir)?;
    let selected = workspace_id
        .map(str::to_owned)
        .or_else(|| registry.active_workspace.clone())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no knowledge workspace configured",
            )
        })?;
    let workspace = registry
        .workspaces
        .iter()
        .find(|entry| entry.id == selected)
        .cloned()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unknown workspace: {selected}"),
            )
        })?;
    Ok((registry, workspace))
}

fn detect_kind(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "markdown" | "txt" | "rst" => "markdown",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "image",
        "pdf" => "pdf",
        "json" | "yaml" | "yml" | "csv" | "tsv" => "data",
        _ => "file",
    }
}

fn extract_title(path: &Path, body: Option<&str>) -> Option<String> {
    if let Some(text) = body {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix('#') {
                let title = rest.trim_start_matches('#').trim();
                if !title.is_empty() {
                    return Some(title.to_string());
                }
            }
            if trimmed.len() >= 4 {
                return Some(trimmed.to_string());
            }
        }
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.replace(['_', '-'], " "))
}

fn read_text_preview(path: &Path) -> Option<String> {
    let kind = detect_kind(path);
    if !(kind == "markdown" || kind == "data" || kind == "file") {
        return None;
    }
    let body = fs::read_to_string(path).ok()?;
    let lines = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(8)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" "))
    }
}

fn source_slug(relative_path: &str) -> String {
    slugify(relative_path)
}

pub(crate) fn collect_sources(
    root: &Path,
) -> Result<Vec<crate::IlhaeAppKbSourceDto>, std::io::Error> {
    let raw_dir = root.join("raw");
    if !raw_dir.exists() {
        return Ok(vec![]);
    }
    let mut sources = walkdir::WalkDir::new(&raw_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let path = entry.into_path();
            let metadata = fs::metadata(&path)?;
            let relative_path = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .to_string();
            let preview = read_text_preview(&path);
            Ok(crate::IlhaeAppKbSourceDto {
                source_id: source_slug(&relative_path),
                relative_path,
                kind: detect_kind(&path).to_string(),
                title: extract_title(&path, preview.as_deref()),
                size: metadata.len(),
                modified_at: timestamp_string(metadata.modified().unwrap_or(UNIX_EPOCH)),
            })
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    sources.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(sources)
}

fn extract_concepts(source: &crate::IlhaeAppKbSourceDto) -> Vec<String> {
    let mut concepts = BTreeSet::new();
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

fn summary_for_source(root: &Path, source: &crate::IlhaeAppKbSourceDto) -> String {
    let full_path = root.join(&source.relative_path);
    if let Some(preview) = read_text_preview(&full_path) {
        return preview;
    }
    format!(
        "{} file at {} ({} bytes)",
        source.kind, source.relative_path, source.size
    )
}

pub(crate) fn write_markdown(path: &Path, body: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)
}

pub(crate) fn compile_workspace(
    root: &Path,
) -> Result<(usize, usize, Vec<String>), std::io::Error> {
    let sources = collect_sources(root)?;
    let mut concept_map: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut generated_files = Vec::new();
    let sources_dir = root.join("wiki").join("sources");
    let concepts_dir = root.join("wiki").join("concepts");
    let index_dir = root.join("index");

    for source in &sources {
        let slug = source_slug(&source.relative_path);
        let concepts = extract_concepts(source);
        let mut body = String::new();
        body.push_str(&format!(
            "# {}\n\n",
            source
                .title
                .clone()
                .unwrap_or_else(|| source.relative_path.clone())
        ));
        body.push_str(&format!("- Source: `{}`\n", source.relative_path));
        body.push_str(&format!("- Kind: `{}`\n", source.kind));
        body.push_str(&format!("- Modified: `{}`\n\n", source.modified_at));
        body.push_str("## Summary\n\n");
        body.push_str(&summary_for_source(root, source));
        body.push_str("\n\n## Concepts\n\n");
        if concepts.is_empty() {
            body.push_str("- _No concepts extracted yet._\n");
        } else {
            for concept in &concepts {
                body.push_str(&format!("- [{}](../concepts/{}.md)\n", concept, concept));
                concept_map.entry(concept.clone()).or_default().push((
                    slug.clone(),
                    source
                        .title
                        .clone()
                        .unwrap_or_else(|| source.relative_path.clone()),
                ));
            }
        }
        let output = sources_dir.join(format!("{slug}.md"));
        write_markdown(&output, &body)?;
        generated_files.push(
            output
                .strip_prefix(root)
                .unwrap_or(output.as_path())
                .to_string_lossy()
                .to_string(),
        );
    }

    for (concept, refs) in &concept_map {
        let mut body = String::new();
        body.push_str(&format!("# {}\n\n", concept));
        body.push_str("## Related Sources\n\n");
        for (slug, title) in refs {
            body.push_str(&format!("- [{}](../sources/{}.md)\n", title, slug));
        }
        let output = concepts_dir.join(format!("{concept}.md"));
        write_markdown(&output, &body)?;
        generated_files.push(
            output
                .strip_prefix(root)
                .unwrap_or(output.as_path())
                .to_string_lossy()
                .to_string(),
        );
    }

    let mut source_index = String::from("# Sources\n\n");
    for source in &sources {
        let slug = source_slug(&source.relative_path);
        source_index.push_str(&format!(
            "- [{}](../wiki/sources/{}.md) — `{}`\n",
            source
                .title
                .clone()
                .unwrap_or_else(|| source.relative_path.clone()),
            slug,
            source.relative_path
        ));
    }
    let source_index_path = index_dir.join("sources.md");
    write_markdown(&source_index_path, &source_index)?;
    generated_files.push(
        source_index_path
            .strip_prefix(root)
            .unwrap_or(source_index_path.as_path())
            .to_string_lossy()
            .to_string(),
    );

    let mut concept_index = String::from("# Concepts\n\n");
    for concept in concept_map.keys() {
        concept_index.push_str(&format!(
            "- [{}](../wiki/concepts/{}.md)\n",
            concept, concept
        ));
    }
    let concept_index_path = index_dir.join("concepts.md");
    write_markdown(&concept_index_path, &concept_index)?;
    generated_files.push(
        concept_index_path
            .strip_prefix(root)
            .unwrap_or(concept_index_path.as_path())
            .to_string_lossy()
            .to_string(),
    );

    Ok((sources.len(), concept_map.len(), generated_files))
}

fn extract_markdown_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find(')') {
            let target = after[..end].trim();
            if !target.is_empty() {
                links.push(target.to_string());
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    links
}

pub(crate) fn lint_workspace(
    root: &Path,
) -> Result<Vec<crate::IlhaeAppKbLintIssueDto>, std::io::Error> {
    let sources = collect_sources(root)?;
    let mut issues = Vec::new();

    for source in &sources {
        let slug = source_slug(&source.relative_path);
        let summary_path = root.join("wiki").join("sources").join(format!("{slug}.md"));
        if !summary_path.exists() {
            issues.push(crate::IlhaeAppKbLintIssueDto {
                kind: "missing_summary".to_string(),
                path: source.relative_path.clone(),
                message: "raw source has no compiled summary".to_string(),
            });
            continue;
        }
        let raw_meta = fs::metadata(root.join(&source.relative_path))?;
        let summary_meta = fs::metadata(&summary_path)?;
        if raw_meta.modified().unwrap_or(UNIX_EPOCH) > summary_meta.modified().unwrap_or(UNIX_EPOCH)
        {
            issues.push(crate::IlhaeAppKbLintIssueDto {
                kind: "stale_raw".to_string(),
                path: source.relative_path.clone(),
                message: "raw source is newer than compiled summary".to_string(),
            });
        }
    }

    for bucket in [root.join("wiki"), root.join("output"), root.join("index")] {
        if !bucket.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&bucket)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.into_path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let body = fs::read_to_string(&path)?;
            for target in extract_markdown_links(&body) {
                if target.starts_with("http://")
                    || target.starts_with("https://")
                    || target.starts_with("mailto:")
                    || target.starts_with('#')
                {
                    continue;
                }
                let normalized = target.split('#').next().unwrap_or("");
                if normalized.is_empty() {
                    continue;
                }
                let candidate = path
                    .parent()
                    .unwrap_or(root)
                    .join(normalized)
                    .components()
                    .as_path()
                    .to_path_buf();
                if !candidate.exists() {
                    issues.push(crate::IlhaeAppKbLintIssueDto {
                        kind: "broken_link".to_string(),
                        path: path
                            .strip_prefix(root)
                            .unwrap_or(path.as_path())
                            .to_string_lossy()
                            .to_string(),
                        message: format!("missing local target: {normalized}"),
                    });
                }
            }
        }
    }

    Ok(issues)
}

fn tokenize_query(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| part.len() >= 2)
        .collect()
}

fn markdown_preview(path: &Path) -> Option<String> {
    let body = fs::read_to_string(path).ok()?;
    let lines = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('#'))
        .filter(|line| !line.starts_with("- ["))
        .take(6)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" "))
    }
}

pub(crate) fn query_workspace(
    root: &Path,
    query: &str,
) -> Result<(String, Vec<String>), std::io::Error> {
    let tokens = tokenize_query(query);
    let mut scored = Vec::new();
    for bucket in [root.join("wiki"), root.join("index")] {
        if !bucket.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&bucket)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.into_path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .to_string();
            let lower = fs::read_to_string(&path)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let haystack = format!("{} {}", rel.to_ascii_lowercase(), lower);
            let score = tokens
                .iter()
                .map(|token| haystack.matches(token).count())
                .sum::<usize>();
            if score > 0 || tokens.is_empty() {
                scored.push((score, rel, markdown_preview(&path).unwrap_or_default()));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let top = scored.into_iter().take(8).collect::<Vec<_>>();
    let matched_paths = top
        .iter()
        .map(|(_, rel, _)| rel.clone())
        .collect::<Vec<_>>();

    let mut answer = String::new();
    answer.push_str(&format!("# Query Report\n\n- Query: `{query}`\n\n"));
    if top.is_empty() {
        answer.push_str("## Result\n\n- No matching wiki material found.\n");
        return Ok((answer, matched_paths));
    }

    answer.push_str("## Matched Documents\n\n");
    for (_, rel, preview) in &top {
        answer.push_str(&format!("- `{rel}`\n"));
        if !preview.is_empty() {
            answer.push_str(&format!("  - Summary: {}\n", preview));
        }
    }

    answer.push_str("\n## Synthesis\n\n");
    for (_, rel, preview) in &top {
        if preview.is_empty() {
            continue;
        }
        answer.push_str(&format!("- `{rel}` indicates: {}\n", preview));
    }
    Ok((answer, matched_paths))
}

pub(crate) fn resolve_relative_target(
    root: &Path,
    bucket: &str,
    relative_path: &str,
) -> Result<PathBuf, std::io::Error> {
    let base = match bucket {
        "wiki" => root.join("wiki"),
        "output" => root.join("output"),
        "index" => root.join("index"),
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsupported KB target: {other}"),
            ));
        }
    };
    let rel = Path::new(relative_path);
    if rel.is_absolute()
        || rel
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "relativePath must stay within the knowledge workspace",
        ));
    }
    Ok(base.join(rel))
}

#[macro_export]
macro_rules! register_admin_kb_handlers {
    ($builder:expr, $state:expr) => {{
        use sacp::{Client, Conductor, ConnectionTo, Responder};
        use std::path::PathBuf;
        let s = $state.clone();
        $builder
            .on_receive_request_from(Client, {
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |_req: crate::IlhaeAppKbWorkspaceListRequest, responder: Responder<crate::IlhaeAppKbWorkspaceListResponse>, _cx: ConnectionTo<Conductor>| {
                    tracing::info!("ilhae/app/kb/workspace/list RPC");
                    match $crate::admin_builtins::kb::load_registry(&ilhae_dir) {
                        Ok(registry) => responder.respond(crate::IlhaeAppKbWorkspaceListResponse {
                            workspaces: registry
                                .workspaces
                                .iter()
                                .map(|entry| $crate::admin_builtins::kb::workspace_to_dto(entry, registry.active_workspace.as_deref()))
                                .collect(),
                            active_workspace: registry.active_workspace,
                        }),
                        Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |req: crate::IlhaeAppKbWorkspaceUpsertRequest, responder: Responder<crate::IlhaeAppKbWorkspaceUpsertResponse>, _cx: ConnectionTo<Conductor>| {
                    tracing::info!("ilhae/app/kb/workspace/upsert RPC name={}", req.name);
                    match $crate::admin_builtins::kb::load_registry(&ilhae_dir) {
                        Ok(mut registry) => {
                            let mut workspace_id = req.workspace_id.clone().unwrap_or_else(|| $crate::admin_builtins::kb::slugify(&req.name));
                            if workspace_id.is_empty() {
                                workspace_id = format!("workspace-{}", registry.workspaces.len() + 1);
                            }
                            let root = PathBuf::from(&req.root_path);
                            if let Err(err) = $crate::admin_builtins::kb::ensure_workspace_dirs(&root) {
                                return responder.respond_with_error(sacp::util::internal_error(err));
                            }
                            let entry = $crate::admin_builtins::kb::KbWorkspaceEntry {
                                id: workspace_id.clone(),
                                name: req.name,
                                root_path: root.to_string_lossy().to_string(),
                            };
                            if let Some(existing) = registry.workspaces.iter_mut().find(|item| item.id == workspace_id) {
                                *existing = entry.clone();
                            } else {
                                registry.workspaces.push(entry.clone());
                            }
                            if req.active || registry.active_workspace.is_none() {
                                registry.active_workspace = Some(workspace_id.clone());
                            }
                            match $crate::admin_builtins::kb::save_registry(&ilhae_dir, &registry) {
                                Ok(()) => responder.respond(crate::IlhaeAppKbWorkspaceUpsertResponse {
                                    ok: true,
                                    active_workspace: registry.active_workspace.clone(),
                                    workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&entry, registry.active_workspace.as_deref())),
                                }),
                                Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                            }
                        }
                        Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |req: crate::IlhaeAppKbIngestRequest, responder: Responder<crate::IlhaeAppKbIngestResponse>, _cx: ConnectionTo<Conductor>| {
                    tracing::info!("ilhae/app/kb/ingest RPC");
                    match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, req.workspace_id.as_deref()) {
                        Ok((registry, workspace)) => {
                            let root = PathBuf::from(&workspace.root_path);
                            if let Err(err) = $crate::admin_builtins::kb::ensure_workspace_dirs(&root) {
                                return responder.respond_with_error(sacp::util::internal_error(err));
                            }
                            match $crate::admin_builtins::kb::collect_sources(&root) {
                                Ok(sources) => {
                                    let inventory_path = root.join("index").join("raw_inventory.json");
                                    let inventory_body = match serde_json::to_vec_pretty(&sources) {
                                        Ok(body) => body,
                                        Err(err) => return responder.respond_with_error(sacp::util::internal_error(err)),
                                    };
                                    if let Err(err) = std::fs::write(&inventory_path, inventory_body) {
                                        return responder.respond_with_error(sacp::util::internal_error(err));
                                    }
                                    responder.respond(crate::IlhaeAppKbIngestResponse {
                                        workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                        sources,
                                        inventory_path: Some(
                                            inventory_path
                                                .strip_prefix(&root)
                                                .unwrap_or(inventory_path.as_path())
                                                .to_string_lossy()
                                                .to_string(),
                                        ),
                                    })
                                }
                                Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                            }
                        }
                        Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |req: crate::IlhaeAppKbCompileRequest, responder: Responder<crate::IlhaeAppKbCompileResponse>, _cx: ConnectionTo<Conductor>| {
                    tracing::info!("ilhae/app/kb/compile RPC");
                    match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, req.workspace_id.as_deref()) {
                        Ok((registry, workspace)) => {
                            let root = PathBuf::from(&workspace.root_path);
                            if let Err(err) = $crate::admin_builtins::kb::ensure_workspace_dirs(&root) {
                                return responder.respond_with_error(sacp::util::internal_error(err));
                            }
                            match $crate::admin_builtins::kb::compile_workspace(&root) {
                                Ok((compiled_sources, concept_count, generated_files)) => responder.respond(crate::IlhaeAppKbCompileResponse {
                                    workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                    compiled_sources,
                                    concept_count,
                                    generated_files,
                                }),
                                Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                            }
                        }
                        Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |req: crate::IlhaeAppKbLintRequest, responder: Responder<crate::IlhaeAppKbLintResponse>, _cx: ConnectionTo<Conductor>| {
                    tracing::info!("ilhae/app/kb/lint RPC");
                    match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, req.workspace_id.as_deref()) {
                        Ok((registry, workspace)) => {
                            let root = PathBuf::from(&workspace.root_path);
                            match $crate::admin_builtins::kb::lint_workspace(&root) {
                                Ok(issues) => responder.respond(crate::IlhaeAppKbLintResponse {
                                    workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                    issues,
                                }),
                                Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                            }
                        }
                        Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |req: crate::IlhaeAppKbQueryRequest, responder: Responder<crate::IlhaeAppKbQueryResponse>, _cx: ConnectionTo<Conductor>| {
                    tracing::info!("ilhae/app/kb/query RPC");
                    match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, req.workspace_id.as_deref()) {
                        Ok((registry, workspace)) => {
                            let root = PathBuf::from(&workspace.root_path);
                            match $crate::admin_builtins::kb::query_workspace(&root, &req.query) {
                                Ok((answer, matched_paths)) => {
                                    let report_path = match req.output_path.as_deref() {
                                        Some(relative) => {
                                            match $crate::admin_builtins::kb::resolve_relative_target(&root, "output", relative) {
                                                Ok(path) => {
                                                    if let Err(err) = $crate::admin_builtins::kb::write_markdown(&path, &answer) {
                                                        return responder.respond_with_error(sacp::util::internal_error(err));
                                                    }
                                                    Some(
                                                        path.strip_prefix(&root)
                                                            .unwrap_or(path.as_path())
                                                            .to_string_lossy()
                                                            .to_string(),
                                                    )
                                                }
                                                Err(err) => return responder.respond_with_error(sacp::util::internal_error(err)),
                                            }
                                        }
                                        None => None,
                                    };
                                    responder.respond(crate::IlhaeAppKbQueryResponse {
                                        workspace: Some($crate::admin_builtins::kb::workspace_to_dto(&workspace, registry.active_workspace.as_deref())),
                                        answer,
                                        matched_paths,
                                        report_path,
                                    })
                                }
                                Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                            }
                        }
                        Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                    }
                }
            }, sacp::on_receive_request!())
            .on_receive_request_from(Client, {
                let ilhae_dir = s.infra.ilhae_dir.clone();
                async move |req: crate::IlhaeAppKbFileBackRequest, responder: Responder<crate::IlhaeAppKbFileBackResponse>, _cx: ConnectionTo<Conductor>| {
                    tracing::info!("ilhae/app/kb/file_back RPC");
                    match $crate::admin_builtins::kb::resolve_workspace(&ilhae_dir, req.workspace_id.as_deref()) {
                        Ok((_registry, workspace)) => {
                            let root = PathBuf::from(&workspace.root_path);
                            match $crate::admin_builtins::kb::resolve_relative_target(&root, &req.target, &req.relative_path) {
                                Ok(path) => match $crate::admin_builtins::kb::write_markdown(&path, &req.content) {
                                    Ok(()) => responder.respond(crate::IlhaeAppKbFileBackResponse {
                                        ok: true,
                                        path: Some(
                                            path.strip_prefix(&root)
                                                .unwrap_or(path.as_path())
                                                .to_string_lossy()
                                                .to_string(),
                                        ),
                                    }),
                                    Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                                },
                                Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                            }
                        }
                        Err(err) => responder.respond_with_error(sacp::util::internal_error(err)),
                    }
                }
            }, sacp::on_receive_request!())
    }};
}
