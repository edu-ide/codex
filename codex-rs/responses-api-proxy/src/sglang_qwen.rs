use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use reqwest::Url;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

pub(crate) fn derive_chat_completions_url(upstream_url: &Url) -> Result<Url> {
    replace_terminal_path(upstream_url, "chat/completions")
}

pub(crate) fn derive_models_url(upstream_url: &Url) -> Result<Url> {
    replace_terminal_path(upstream_url, "models")
}

fn replace_terminal_path(upstream_url: &Url, terminal: &str) -> Result<Url> {
    let mut url = upstream_url.clone();
    let prefix = url
        .path()
        .strip_suffix("/responses")
        .ok_or_else(|| anyhow!("upstream URL path must end with /responses"))?;
    url.set_path(&format!("{prefix}/{terminal}"));
    Ok(url)
}

pub(crate) fn responses_to_chat_completions_request(body: &Value) -> Result<Value> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("responses request missing string model"))?;

    let mut messages = Vec::new();
    let mut system_parts = Vec::new();
    if let Some(instructions) = body.get("instructions").and_then(Value::as_str)
        && !instructions.trim().is_empty()
    {
        system_parts.push(instructions.to_string());
    }

    if let Some(input) = body.get("input").and_then(Value::as_array) {
        for item in input {
            if let Some(content) = extract_system_message_content(item)? {
                system_parts.push(content);
                continue;
            }
            if let Some(message) = response_input_item_to_chat_message(item)? {
                messages.push(message);
            }
        }
    }

    if !system_parts.is_empty() {
        messages.insert(
            0,
            json!({
                "role": "system",
                "content": system_parts.join("\n\n")
            }),
        );
    }

    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(response_tool_to_chat_tool)
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "tool_choice": body.get("tool_choice").cloned().unwrap_or(json!("auto")),
        "parallel_tool_calls": body.get("parallel_tool_calls").cloned().unwrap_or(json!(true)),
        "chat_template_kwargs": {
            "enable_thinking": false
        },
        "stream": false
    }))
}

fn extract_system_message_content(item: &Value) -> Result<Option<String>> {
    if item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message")
        != "message"
    {
        return Ok(None);
    }

    let role = item
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("message item missing role"))?;
    if !matches!(role, "developer" | "system") {
        return Ok(None);
    }

    Ok(Some(content_items_to_text(item.get("content"))))
}

fn response_tool_to_chat_tool(tool: &Value) -> Result<Value> {
    if let Some(function) = tool.get("function") {
        return Ok(json!({
            "type": "function",
            "function": function.clone()
        }));
    }

    if tool.get("type").and_then(Value::as_str) == Some("function") {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("function tool missing name"))?;
        return Ok(json!({
            "type": "function",
            "function": {
                "name": name,
                "description": tool.get("description").cloned().unwrap_or(Value::Null),
                "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}}))
            }
        }));
    }

    Err(anyhow!("unsupported tool type for sglang-qwen mode"))
}

fn response_input_item_to_chat_message(item: &Value) -> Result<Option<Value>> {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    match item_type {
        "message" => {
            let role = item
                .get("role")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("message item missing role"))?;
            let content = content_items_to_text(item.get("content"));
            Ok(Some(json!({
                "role": normalize_message_role(role)?,
                "content": content
            })))
        }
        "function_call" => {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("function_call item missing name"))?;
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("function_call item missing arguments"))?;
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("function_call item missing call_id"))?;
            Ok(Some(json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments
                    }
                }]
            })))
        }
        "function_call_output" => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("function_call_output item missing call_id"))?;
            let content = output_payload_to_text(item.get("output"));
            Ok(Some(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content
            })))
        }
        "reasoning" => Ok(None),
        other => Err(anyhow!(
            "unsupported input item type for sglang-qwen mode: {other}"
        )),
    }
}

fn normalize_message_role(role: &str) -> Result<&str> {
    match role {
        "developer" | "system" => Ok("system"),
        "user" => Ok("user"),
        "assistant" => Ok("assistant"),
        "tool" => Ok("tool"),
        other => Err(anyhow!(
            "unsupported message role for sglang-qwen mode: {other}"
        )),
    }
}

fn output_payload_to_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn content_items_to_text(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

pub(crate) fn transform_chat_completion_to_responses_json(completion: &Value) -> Result<Value> {
    let response_id = completion
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_sglang_qwen")
        .to_string();
    let created_at = completion
        .get("created")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let model = completion
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let usage = completion.get("usage").cloned();
    let message = completion
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| anyhow!("chat completion missing choices[0].message"))?;

    let output = message_to_response_items(message)?;

    Ok(json!({
        "id": response_id,
        "object": "response",
        "created_at": created_at,
        "model": model,
        "output": output,
        "status": "completed",
        "usage": map_usage(usage.as_ref())
    }))
}

pub(crate) fn build_sglang_qwen_response_body(
    completion: &Value,
    wants_stream: bool,
) -> Result<(&'static str, Vec<u8>)> {
    let response = transform_chat_completion_to_responses_json(completion)?;
    if !wants_stream {
        return Ok(("application/json", serde_json::to_vec(&response)?));
    }

    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_sglang_qwen");
    let mut sse = String::new();
    push_sse_event(
        &mut sse,
        &json!({
            "type": "response.created",
            "response": { "id": response_id }
        }),
    )?;

    if let Some(items) = response.get("output").and_then(Value::as_array) {
        for item in items {
            push_sse_event(
                &mut sse,
                &json!({
                    "type": "response.output_item.done",
                    "item": item
                }),
            )?;
        }
    }

    push_sse_event(
        &mut sse,
        &json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "usage": response.get("usage").cloned().unwrap_or(Value::Null)
            }
        }),
    )?;
    Ok(("text/event-stream", sse.into_bytes()))
}

pub(crate) fn canonical_model_id_from_models_response(models_response: &Value) -> Option<String> {
    let models = models_response.get("data")?.as_array()?;
    if models.len() == 1 {
        return models[0]
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
    None
}

fn push_sse_event(sse: &mut String, payload: &Value) -> Result<()> {
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("SSE payload missing type"))?;
    sse.push_str("event: ");
    sse.push_str(event_type);
    sse.push('\n');
    sse.push_str("data: ");
    sse.push_str(&serde_json::to_string(payload)?);
    sse.push_str("\n\n");
    Ok(())
}

fn map_usage(usage: Option<&Value>) -> Value {
    let prompt_tokens = usage
        .and_then(|value| value.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let completion_tokens = usage
        .and_then(|value| value.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total_tokens = usage
        .and_then(|value| value.get("total_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(prompt_tokens + completion_tokens);

    json!({
        "input_tokens": prompt_tokens,
        "input_tokens_details": {
            "cached_tokens": 0
        },
        "output_tokens": completion_tokens,
        "output_tokens_details": {
            "reasoning_tokens": 0
        },
        "total_tokens": total_tokens
    })
}

fn message_to_response_items(message: &Value) -> Result<Vec<Value>> {
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array)
        && !tool_calls.is_empty()
    {
        let mut items = Vec::with_capacity(tool_calls.len());
        for (index, tool_call) in tool_calls.iter().enumerate() {
            items.push(tool_call_to_response_item(tool_call, index + 1)?);
        }
        return Ok(items);
    }

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let tool_calls = parse_tool_calls_from_content(content)?;
    if !tool_calls.is_empty() {
        return Ok(tool_calls);
    }

    let cleaned = strip_think_blocks(content).trim().to_string();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }

    Ok(vec![json!({
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": cleaned
        }]
    })])
}

fn tool_call_to_response_item(tool_call: &Value, fallback_index: usize) -> Result<Value> {
    let function = tool_call
        .get("function")
        .ok_or_else(|| anyhow!("tool call missing function"))?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tool call missing function.name"))?;
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tool call missing function.arguments"))?;
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("call_{fallback_index}"));

    Ok(json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments
    }))
}

fn parse_tool_calls_from_content(content: &str) -> Result<Vec<Value>> {
    let blocks = extract_tag_blocks(content, "tool_call");
    let mut items = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let (name, arguments) = parse_tool_call_block(block)
            .with_context(|| format!("parsing tool call block {}", index + 1))?;
        items.push(json!({
            "type": "function_call",
            "call_id": format!("call_{}", index + 1),
            "name": name,
            "arguments": arguments
        }));
    }
    Ok(items)
}

fn parse_tool_call_block(block: &str) -> Result<(String, String)> {
    let trimmed = block.trim();
    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed).context("parsing tool call JSON")?;
        let name = value
            .get("name")
            .or_else(|| value.get("tool_name"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("tool call JSON missing name"))?
            .to_string();
        let arguments = value
            .get("arguments")
            .or_else(|| value.get("parameters"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments = match arguments {
            Value::String(value) => value,
            other => serde_json::to_string(&other)?,
        };
        return Ok((name, arguments));
    }

    parse_xmlish_tool_call(trimmed)
}

fn parse_xmlish_tool_call(block: &str) -> Result<(String, String)> {
    let function_open = "<function=";
    let function_start = block
        .find(function_open)
        .ok_or_else(|| anyhow!("missing <function=...> tag"))?;
    let function_name_start = function_start + function_open.len();
    let function_name_end = block[function_name_start..]
        .find('>')
        .map(|offset| function_name_start + offset)
        .ok_or_else(|| anyhow!("unterminated <function=...> tag"))?;
    let function_name = block[function_name_start..function_name_end]
        .trim()
        .to_string();
    let function_body = &block[function_name_end + 1..];

    let mut args = Map::new();
    let parameter_open = "<parameter=";
    let mut search = function_body;
    while let Some(start) = search.find(parameter_open) {
        let name_start = start + parameter_open.len();
        let name_end = search[name_start..]
            .find('>')
            .map(|offset| name_start + offset)
            .ok_or_else(|| anyhow!("unterminated <parameter=...> tag"))?;
        let param_name = search[name_start..name_end].trim().to_string();
        let value_start = name_end + 1;
        let close_tag = "</parameter>";
        let value_end = search[value_start..]
            .find(close_tag)
            .map(|offset| value_start + offset)
            .ok_or_else(|| anyhow!("missing </parameter> tag"))?;
        let raw_value = search[value_start..value_end].trim();
        let value = serde_json::from_str(raw_value)
            .unwrap_or_else(|_| Value::String(raw_value.to_string()));
        args.insert(param_name, value);
        search = &search[value_end + close_tag.len()..];
    }

    if args.is_empty() {
        return Err(anyhow!("no parameters found in tool call"));
    }

    Ok((function_name, serde_json::to_string(&Value::Object(args))?))
}

fn extract_tag_blocks(content: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut rest = content;
    let mut blocks = Vec::new();

    while let Some(open_index) = rest.find(&open) {
        let after_open = &rest[open_index + open.len()..];
        let Some(close_index) = after_open.find(&close) else {
            break;
        };
        blocks.push(after_open[..close_index].to_string());
        rest = &after_open[close_index + close.len()..];
    }

    blocks
}

fn strip_think_blocks(content: &str) -> String {
    let mut output = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("<think>") {
        output.push_str(&rest[..start]);
        let after_open = &rest[start + "<think>".len()..];
        let Some(end) = after_open.find("</think>") else {
            rest = "";
            break;
        };
        rest = &after_open[end + "</think>".len()..];
    }
    output.push_str(rest);

    if let Some(orphan_end) = output.find("</think>") {
        return output[orphan_end + "</think>".len()..].to_string();
    }

    output
}
