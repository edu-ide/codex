//! Product model policy. No filesystem reads or Codex catalog/schema dependencies.

use super::IlhaeProfileConfig;
use super::IlhaeProfileNativeRuntimeConfig;
use std::path::Path;

fn parse_context_window_from_native_args(args: &[String]) -> Option<u64> {
    let mut idx = 0usize;
    while idx < args.len() {
        let arg = args[idx].trim();
        if matches!(
            arg,
            "-c" | "--ctx-size" | "--context-size" | "--context-length"
        ) {
            if let Some(value) = args.get(idx + 1).and_then(|next| next.parse::<u64>().ok()) {
                return Some(value);
            }
        } else if let Some(value) = arg
            .strip_prefix("--ctx-size=")
            .or_else(|| arg.strip_prefix("--context-size="))
            .or_else(|| arg.strip_prefix("--context-length="))
            .and_then(|value| value.parse::<u64>().ok())
        {
            return Some(value);
        }
        idx += 1;
    }
    None
}

fn parse_context_window_from_native_query_params(
    query_params: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<u64> {
    let query_params = query_params?;
    let normalized_lookup =
        |key: &str| -> Option<u64> { query_params.get(key).and_then(|value| value.parse().ok()) };

    if let Some(value) = normalized_lookup("context-size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("context_size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("ctx-size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("ctx_size") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("context-length") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("context_length") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("num-ctx") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("num_ctx") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("n-ctx") {
        return Some(value);
    }
    if let Some(value) = normalized_lookup("n_ctx") {
        return Some(value);
    }

    for (raw_key, value) in query_params {
        match raw_key
            .trim()
            .to_ascii_lowercase()
            .replace('_', "-")
            .as_str()
        {
            "context-size" | "ctx-size" | "context-length" | "num-ctx" | "n-ctx" => {
                if let Ok(value) = value.parse() {
                    return Some(value);
                }
            }
            _ => {}
        }
    }

    None
}

pub(super) fn native_runtime_model_context_window(
    runtime: &IlhaeProfileNativeRuntimeConfig,
) -> u64 {
    if let Some(context_window) = runtime
        .context_window
        .filter(|context_window| *context_window > 0)
    {
        return context_window;
    }

    if let Some(context_window) = parse_context_window_from_native_args(&runtime.args) {
        return context_window;
    }

    if let Some(context_window) =
        parse_context_window_from_native_query_params(runtime.query_params.as_ref())
    {
        return context_window;
    }

    32_768
}

pub(super) fn profile_engine_id_for_display(profile: &IlhaeProfileConfig) -> Option<String> {
    profile
        .agent
        .engine_id
        .as_deref()
        .map(|engine_id| engine_id.trim())
        .filter(|engine_id| !engine_id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            profile
                .agent
                .command
                .as_deref()
                .map(crate::helpers::infer_agent_id_from_command)
                .map(|engine_id| engine_id.trim().to_string())
                .filter(|engine_id| !engine_id.is_empty())
        })
}

pub(super) fn profile_engine_id(profile: &IlhaeProfileConfig) -> String {
    profile_engine_id_for_display(profile).unwrap_or_else(|| "ilhae".to_string())
}

pub(super) fn profile_runtime_model_name(profile: &IlhaeProfileConfig) -> Option<String> {
    let raw_model_path = profile.native_runtime.model_path.trim();
    if raw_model_path.is_empty() || raw_model_path.eq_ignore_ascii_case("default") {
        return None;
    }

    native_runtime_model_name_from_path(raw_model_path)
}

fn fallback_profile_name(profile_id: &str) -> String {
    let trimmed = profile_id.trim();
    if trimmed.is_empty() {
        "ilhae".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn resolve_profile_model_name(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
    cloud_model: impl FnOnce() -> Option<String>,
) -> String {
    if let Some(model_name) =
        native_runtime_model_name_from_path(&profile.native_runtime.model_path)
    {
        return model_name;
    }

    if let Some(engine_id) = profile
        .agent
        .engine_id
        .as_deref()
        .map(str::trim)
        .filter(|engine_id| {
            !engine_id.is_empty()
                && !engine_id.eq_ignore_ascii_case("default")
                && !engine_id.eq_ignore_ascii_case("codex")
                && !engine_id.eq_ignore_ascii_case("openai")
        })
    {
        return engine_id.to_string();
    }

    if let Some(model_name) = cloud_model() {
        return model_name;
    }

    if profile
        .agent
        .engine_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|engine_id| engine_id.eq_ignore_ascii_case("openai"))
    {
        return "gpt-5.5".to_string();
    }

    if let Some(command) = profile
        .agent
        .command
        .as_deref()
        .map(str::trim)
        .filter(|command| {
            !command.is_empty()
                && !command.eq_ignore_ascii_case("default")
                && !command.eq_ignore_ascii_case("codex")
                && !command.eq_ignore_ascii_case("openai")
        })
    {
        return command.to_string();
    }

    fallback_profile_name(profile_id)
}

pub fn native_runtime_model_name_from_path(raw_model_path: &str) -> Option<String> {
    let path = Path::new(raw_model_path.trim());
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let use_stem = matches!(
        extension.as_deref(),
        Some("gguf" | "safetensors" | "bin" | "pt" | "pth" | "onnx" | "ckpt")
    );
    let name = if use_stem {
        path.file_stem().or_else(|| path.file_name())
    } else {
        path.file_name().or_else(|| path.file_stem())
    }?;
    let model_name = name.to_string_lossy().trim().to_string();
    if model_name.is_empty() || model_name.eq_ignore_ascii_case("default") {
        None
    } else {
        Some(model_name)
    }
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
