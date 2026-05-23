//! Compact TUI rendering for MCP resource discovery/read tool calls.

use codex_protocol::mcp::CallToolResult;
use ratatui::prelude::Line;
use ratatui::style::Stylize;
use serde::Deserialize;
use serde_json::Value;

use super::McpInvocation;

const MAX_TREE_ENTRIES: usize = 12;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceListPayload {
    #[serde(default)]
    resources: Vec<ResourceEntry>,
    #[serde(default)]
    resource_templates: Vec<ResourceTemplateEntry>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceEntry {
    server: String,
    uri: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceTemplateEntry {
    server: String,
    uri_template: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResourceReadPayload {
    server: String,
    uri: String,
    #[serde(default)]
    contents: Vec<ResourceContentEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceContentEntry {
    uri: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    blob: Option<String>,
}

pub(super) fn mcp_resource_header(
    invocation: &McpInvocation,
    completed: bool,
) -> Option<&'static str> {
    match invocation.tool.as_str() {
        "list_mcp_resources" | "list_mcp_resource_templates" => {
            Some(if completed { "Listed" } else { "Listing" })
        }
        "read_mcp_resource" => Some(if completed { "Read" } else { "Reading" }),
        _ => None,
    }
}

pub(super) fn format_mcp_resource_invocation(invocation: &McpInvocation) -> Option<Line<'static>> {
    match invocation.tool.as_str() {
        "list_mcp_resources" => Some("Ilhae resources".cyan().into()),
        "list_mcp_resource_templates" => Some("Ilhae resource templates".cyan().into()),
        "read_mcp_resource" => {
            let uri = invocation
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("uri"))
                .and_then(Value::as_str);
            let label = format!("{} resource", invocation.server);
            Some(match uri {
                Some(uri) => vec![label.cyan(), " ".into(), uri.to_string().dim()].into(),
                None => label.cyan().into(),
            })
        }
        _ => None,
    }
}

pub(super) fn render_mcp_resource_result(
    invocation: &McpInvocation,
    result: &CallToolResult,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    let text = first_text_block(result)?;
    let payload: Value = serde_json::from_str(text).ok()?;
    let lines = match invocation.tool.as_str() {
        "list_mcp_resources" => render_resource_list(&payload, width, ResourceListKind::Resource),
        "list_mcp_resource_templates" => {
            render_resource_list(&payload, width, ResourceListKind::Template)
        }
        "read_mcp_resource" => render_resource_read(&payload, width),
        _ => return None,
    };

    Some(lines)
}

pub(super) fn raw_mcp_resource_result(
    invocation: &McpInvocation,
    result: &CallToolResult,
) -> Option<Vec<Line<'static>>> {
    render_mcp_resource_result(invocation, result, usize::MAX)
        .map(|lines| lines.into_iter().map(unstyle_line).collect())
}

fn first_text_block(result: &CallToolResult) -> Option<&str> {
    result
        .content
        .iter()
        .find_map(|block| block.get("text").and_then(Value::as_str))
}

#[derive(Clone, Copy)]
enum ResourceListKind {
    Resource,
    Template,
}

fn render_resource_list(
    payload: &Value,
    width: usize,
    kind: ResourceListKind,
) -> Vec<Line<'static>> {
    let parsed = match serde_json::from_value::<ResourceListPayload>(payload.clone()) {
        Ok(parsed) => parsed,
        Err(_) => return Vec::new(),
    };

    let entries = match kind {
        ResourceListKind::Resource => parsed
            .resources
            .into_iter()
            .map(|resource| ResourceTreeEntry {
                server: resource.server,
                uri: resource.uri,
                label: resource.title.or(resource.name),
                mime_type: resource.mime_type,
            })
            .collect::<Vec<_>>(),
        ResourceListKind::Template => parsed
            .resource_templates
            .into_iter()
            .map(|template| ResourceTreeEntry {
                server: template.server,
                uri: template.uri_template,
                label: template.title.or(template.name),
                mime_type: template.mime_type,
            })
            .collect::<Vec<_>>(),
    };
    if entries.is_empty() {
        return vec!["No resources returned".dim().into()];
    }

    let title = match kind {
        ResourceListKind::Resource => format!("Ilhae resources ({})", entries.len()),
        ResourceListKind::Template => format!("Ilhae resource templates ({})", entries.len()),
    };
    let mut lines = vec![Line::from(title.dim())];
    let mut grouped: Vec<(String, Vec<ResourceTreeEntry>)> = Vec::new();
    for entry in entries {
        if let Some((_, server_entries)) = grouped
            .iter_mut()
            .find(|(server, _)| server == &entry.server)
        {
            server_entries.push(entry);
        } else {
            grouped.push((entry.server.clone(), vec![entry]));
        }
    }
    grouped.sort_by(|a, b| a.0.cmp(&b.0));

    let mut rendered_count = 0;
    for (server_index, (server, mut server_entries)) in grouped.into_iter().enumerate() {
        server_entries.sort_by(|a, b| a.uri.cmp(&b.uri));
        let server_branch = if server_index == 0 { "┌" } else { "├" };
        lines.push(Line::from(format!("{server_branch} {server}").dim()));
        for entry in server_entries {
            if rendered_count == MAX_TREE_ENTRIES {
                lines.push(Line::from("└ … more resources omitted".dim()));
                return lines;
            }
            rendered_count += 1;
            lines.extend(wrap_tree_line(&format!("├ {}", entry.uri), width));
            if let Some(label) = entry.label {
                lines.extend(wrap_tree_line(&format!("│  {label}"), width));
            }
            if let Some(mime_type) = entry.mime_type {
                lines.extend(wrap_tree_line(&format!("│  {mime_type}"), width));
            }
        }
    }

    if let Some(next_cursor) = parsed.next_cursor {
        lines.extend(wrap_tree_line(
            &format!("└ next cursor: {next_cursor}"),
            width,
        ));
    }

    lines
}

#[derive(Debug)]
struct ResourceTreeEntry {
    server: String,
    uri: String,
    label: Option<String>,
    mime_type: Option<String>,
}

fn render_resource_read(payload: &Value, width: usize) -> Vec<Line<'static>> {
    let parsed = match serde_json::from_value::<ResourceReadPayload>(payload.clone()) {
        Ok(parsed) => parsed,
        Err(_) => return Vec::new(),
    };
    let mut lines = vec![
        Line::from(format!("{} resource", parsed.server).dim()),
        Line::from(format!("└ {}", parsed.uri).dim()),
    ];

    if parsed.contents.is_empty() {
        lines.push("  └ empty resource".dim().into());
        return lines;
    }

    for content in parsed.contents {
        let mime_type = content
            .mime_type
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let size = content
            .text
            .as_deref()
            .map(str::len)
            .or_else(|| content.blob.as_deref().map(str::len))
            .unwrap_or(0);
        let summary = content
            .text
            .as_deref()
            .and_then(summarize_json_text)
            .unwrap_or_else(|| format!("{size} chars"));
        lines.extend(wrap_tree_line(
            &format!("  ├ {} · {summary}", content.uri),
            width,
        ));
        lines.extend(wrap_tree_line(&format!("  │  {mime_type}"), width));
    }

    lines
}

fn summarize_json_text(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some(match value {
        Value::Array(items) => format!("json array · {} items", items.len()),
        Value::Object(map) => match map.get("count").and_then(Value::as_u64) {
            Some(count) => format!("json object · {count} items"),
            None => format!("json object · {} keys", map.len()),
        },
        Value::String(value) => format!("json string · {} chars", value.len()),
        Value::Number(_) => "json number".to_string(),
        Value::Bool(_) => "json boolean".to_string(),
        Value::Null => "json null".to_string(),
    })
}

fn wrap_tree_line(text: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    textwrap::wrap(text, width)
        .into_iter()
        .map(|line| Line::from(line.into_owned().dim()))
        .collect()
}

fn unstyle_line(line: Line<'static>) -> Line<'static> {
    let text = line
        .spans
        .into_iter()
        .map(|span| span.content.into_owned())
        .collect::<String>();
    Line::from(text)
}
