use std::ffi::OsString;
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::anyhow;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;

const PROXY_BINARY_NAME: &str = "codex-responses-api-proxy";
const PROXY_START_TIMEOUT: Duration = Duration::from_secs(30);
const PROXY_POLL_INTERVAL: Duration = Duration::from_millis(20);
const SGLANG_PROVIDER_ID: &str = "sglang";
const LLAMA_SERVER_PROVIDER_ID: &str = "llama-server";
const SGLANG_URL_ENV_VAR: &str = "CODEX_SGLANG_URL";

pub struct SglangQwenProxy {
    _temp_dir: TempDir,
    child: Child,
    base_url: String,
}

impl SglangQwenProxy {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for SglangQwenProxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn maybe_start_sglang_qwen_proxy(
    provider_id: &str,
    upstream_base_url: Option<&str>,
    codex_self_exe: Option<&Path>,
) -> Result<Option<SglangQwenProxy>> {
    let debug_dir = prepare_debug_dir()?;
    let env_override_present = std::env::var_os(SGLANG_URL_ENV_VAR).is_some();
    if !matches!(provider_id, SGLANG_PROVIDER_ID | LLAMA_SERVER_PROVIDER_ID) || env_override_present
    {
        write_decision(
            &debug_dir,
            "skipped",
            provider_id,
            upstream_base_url,
            codex_self_exe,
            env_override_present,
        )?;
        return Ok(None);
    }

    let Some(upstream_base_url) = upstream_base_url else {
        write_decision(
            &debug_dir,
            "missing_upstream_base_url",
            provider_id,
            upstream_base_url,
            codex_self_exe,
            env_override_present,
        )?;
        return Ok(None);
    };
    let Some(codex_self_exe) = codex_self_exe else {
        write_decision(
            &debug_dir,
            "missing_codex_self_exe",
            provider_id,
            Some(upstream_base_url),
            codex_self_exe,
            env_override_present,
        )?;
        return Ok(None);
    };

    write_decision(
        &debug_dir,
        "starting",
        provider_id,
        Some(upstream_base_url),
        Some(codex_self_exe),
        env_override_present,
    )?;

    Ok(Some(start_sglang_qwen_proxy(
        codex_self_exe,
        upstream_base_url,
        debug_dir,
    )?))
}

fn start_sglang_qwen_proxy(
    codex_self_exe: &Path,
    upstream_base_url: &str,
    debug_dir: PathBuf,
) -> Result<SglangQwenProxy> {
    let temp_dir = tempfile::Builder::new()
        .prefix("codex-sglang-qwen-proxy-")
        .tempdir()?;
    let server_info = temp_dir.path().join("server-info.json");
    let upstream_url = format!("{}/responses", upstream_base_url.trim_end_matches('/'));
    let stderr_log = debug_dir.join("proxy-stderr.log");
    let (program, needs_subcommand) = proxy_program_and_subcommand(codex_self_exe);

    let mut command = Command::new(program);
    if needs_subcommand {
        command.arg("responses-api-proxy");
    }
    let stderr = File::create(&stderr_log)?;
    let mut child = command
        .arg("--server-info")
        .arg(&server_info)
        .arg("--dump-dir")
        .arg(&debug_dir)
        .arg("--provider-mode")
        .arg("sglang-qwen")
        .arg("--upstream-url")
        .arg(&upstream_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()?;

    let deadline = Instant::now() + PROXY_START_TIMEOUT;
    loop {
        match std::fs::read_to_string(&server_info) {
            Ok(info) => {
                if !info.trim().is_empty() {
                    match serde_json::from_str::<Value>(&info) {
                        Ok(info) => {
                            let port = info
                                .get("port")
                                .and_then(Value::as_u64)
                                .ok_or_else(|| anyhow!("proxy server info missing port"))?;
                            return Ok(SglangQwenProxy {
                                _temp_dir: temp_dir,
                                child,
                                base_url: format!("http://127.0.0.1:{port}/v1"),
                            });
                        }
                        Err(err) if err.is_eof() => {}
                        Err(err) => return Err(err.into()),
                    }
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!(
                "responses-api-proxy exited before writing server info: {status}"
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("timed out waiting for responses-api-proxy"));
        }
        thread::sleep(PROXY_POLL_INTERVAL);
    }
}

fn prepare_debug_dir() -> Result<PathBuf> {
    let debug_dir = std::env::temp_dir().join("codex-sglang-qwen-proxy-last");
    match std::fs::remove_dir_all(&debug_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    std::fs::create_dir_all(&debug_dir)?;
    Ok(debug_dir)
}

fn write_decision(
    debug_dir: &Path,
    status: &str,
    provider_id: &str,
    upstream_base_url: Option<&str>,
    codex_self_exe: Option<&Path>,
    env_override_present: bool,
) -> Result<()> {
    let payload = json!({
        "status": status,
        "provider_id": provider_id,
        "upstream_base_url": upstream_base_url,
        "codex_self_exe": codex_self_exe.map(|path| path.display().to_string()),
        "env_override_present": env_override_present,
    });
    std::fs::write(
        debug_dir.join("decision.json"),
        serde_json::to_vec_pretty(&payload)?,
    )?;
    Ok(())
}

fn proxy_program_and_subcommand(codex_self_exe: &Path) -> (PathBuf, bool) {
    if let Some(sibling) = sibling_proxy_binary(codex_self_exe)
        && sibling.is_file()
    {
        return (sibling, false);
    }
    (codex_self_exe.to_path_buf(), true)
}

fn sibling_proxy_binary(codex_self_exe: &Path) -> Option<PathBuf> {
    let parent = codex_self_exe.parent()?;
    Some(parent.join(proxy_binary_filename()))
}

fn proxy_binary_filename() -> OsString {
    let mut name = OsString::from(PROXY_BINARY_NAME);
    if cfg!(windows) {
        name.push(".exe");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::proxy_program_and_subcommand;
    use pretty_assertions::assert_eq;

    #[test]
    fn uses_sibling_proxy_binary_when_present() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let codex_self = temp_dir
            .path()
            .join(if cfg!(windows) { "ilhae.exe" } else { "ilhae" });
        std::fs::write(&codex_self, b"").expect("write codex self");
        let sibling = temp_dir.path().join(if cfg!(windows) {
            "codex-responses-api-proxy.exe"
        } else {
            "codex-responses-api-proxy"
        });
        std::fs::write(&sibling, b"").expect("write sibling proxy");

        let (program, needs_subcommand) = proxy_program_and_subcommand(&codex_self);

        assert_eq!(program, sibling);
        assert_eq!(needs_subcommand, false);
    }

    #[test]
    fn falls_back_to_multitool_subcommand_when_sibling_is_missing() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let codex_self = temp_dir
            .path()
            .join(if cfg!(windows) { "ilhae.exe" } else { "ilhae" });
        std::fs::write(&codex_self, b"").expect("write codex self");

        let (program, needs_subcommand) = proxy_program_and_subcommand(&codex_self);

        assert_eq!(program, codex_self);
        assert_eq!(needs_subcommand, true);
    }
}
