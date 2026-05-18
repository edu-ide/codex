use std::path::Path;
use std::path::PathBuf;

use axum::Json;
use axum::response::IntoResponse;
use axum::response::Response;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComfyViewPath {
    pub path: PathBuf,
    pub content_type: &'static str,
}

pub fn resolve_view_path(
    comfy_root: &Path,
    file_type: &str,
    subfolder: &str,
    filename: &str,
) -> anyhow::Result<ResolvedComfyViewPath> {
    if !matches!(file_type, "input" | "output" | "temp") {
        anyhow::bail!("invalid ComfyUI file type `{file_type}`");
    }
    if filename.is_empty() || filename.contains('\0') || subfolder.contains('\0') {
        anyhow::bail!("invalid ComfyUI view path");
    }

    let base = comfy_root.join(file_type);
    let base = base
        .canonicalize()
        .unwrap_or_else(|_| comfy_root.join(file_type));
    let resolved = base.join(subfolder).join(filename);
    let normalized = normalize_path_without_fs(&resolved);
    let normalized_base = normalize_path_without_fs(&base);
    if normalized != normalized_base && !normalized.starts_with(&normalized_base) {
        anyhow::bail!("invalid ComfyUI view path");
    }

    Ok(ResolvedComfyViewPath {
        path: normalized,
        content_type: content_type_for_filename(filename),
    })
}

fn normalize_path_without_fs(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn content_type_for_filename(filename: &str) -> &'static str {
    match Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("apng") => "image/apng",
        Some("gif") => "image/gif",
        Some("jpeg" | "jpg") => "image/jpeg",
        Some("json") => "application/json",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        Some("ogg") => "audio/ogg",
        Some("png") => "image/png",
        Some("wav") => "audio/wav",
        Some("webm") => "video/webm",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

impl IntoResponse for ResolvedComfyViewPath {
    fn into_response(self) -> Response {
        Json(serde_json::json!({
            "path": self.path,
            "contentType": self.content_type,
        }))
        .into_response()
    }
}
