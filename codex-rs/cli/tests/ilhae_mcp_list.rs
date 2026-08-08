use std::fs;
use std::path::Path;
use std::process::Output;

use anyhow::Context;
use anyhow::Result;
use serde_json::Value as JsonValue;
use tempfile::TempDir;

fn ilhae_command(config_dir: &Path, home_dir: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("ilhae")?);
    cmd.env("HOME", home_dir)
        .env("ILHAE_CONFIG_DIR", config_dir)
        .env("ILHAE_DATA_DIR", config_dir)
        .env("ILHAE_CODEX_HOME", config_dir.join("codex-home"))
        .env("ILHAE_DREAM_MODE", "1")
        .env_remove("CODEX_HOME")
        .env_remove("ILHAE_APP_SERVER")
        .env_remove("ILHAE_RUNTIME");
    Ok(cmd)
}

fn source_hash(bytes: &[u8]) -> u64 {
    // A stable FNV-1a digest is enough to prove that the human-managed source
    // bytes did not change while the isolated runtime snapshot was rebuilt.
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn assert_no_forbidden_diagnostics(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let forbidden = [
        "failed to load configuration",
        "url is not supported for stdio",
        "unknown MCP server",
        "인계 업무를 확인하지 못했습니다.",
        "결재를 확인하지 못했습니다.",
        "원문을 읽지 못했습니다 — upload-workbook 단계에서 LCA 실행이 멈췄습니다.",
        "연결을 확인한 뒤 같은 좌표부터 다시 읽을 수 있습니다.",
        // Poison fixture payloads must not be echoed by a warning either.
        "mcp_servers.poison",
        "third-party-mcp",
        "https://poison.invalid/mcp",
        "office-mcp",
        "https://office.invalid/mcp",
        "file:///tmp/office.sock",
        "retired-excel-mcp",
    ];

    for text in forbidden {
        assert!(
            !stdout.contains(text) && !stderr.contains(text),
            "forbidden diagnostic {text:?} was emitted\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}

fn run_mcp_list(config_source: &[u8]) -> Result<(TempDir, JsonValue, toml::Value)> {
    let temp = TempDir::new()?;
    let config_dir = temp.path().join("ilhae-config");
    let home_dir = temp.path().join("home");
    fs::create_dir_all(&config_dir)?;
    fs::create_dir_all(&home_dir)?;

    let source_path = config_dir.join("config.toml");
    fs::write(&source_path, config_source)?;
    let before_hash = source_hash(config_source);

    let mut cmd = ilhae_command(&config_dir, &home_dir)?;
    let output = cmd.args(["mcp", "list", "--json"]).output()?;
    assert!(
        output.status.success(),
        "ilhae mcp list --json failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_no_forbidden_diagnostics(&output);

    let source_after = fs::read(&source_path)?;
    assert_eq!(source_hash(&source_after), before_hash);
    assert_eq!(source_after, config_source);

    let listed: JsonValue = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "stdout was not JSON: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })?;
    let runtime_source = fs::read_to_string(config_dir.join("codex-home/config.toml"))?;
    let runtime = toml::from_str::<toml::Value>(&runtime_source)?;

    Ok((temp, listed, runtime))
}

fn assert_runtime_has_one_transport_selector(runtime: &toml::Value) {
    let servers = runtime
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .expect("generated runtime config should contain mcp_servers");

    for (name, server) in servers {
        let server = server
            .as_table()
            .unwrap_or_else(|| panic!("runtime MCP server {name:?} should be a table"));
        assert_ne!(
            server.contains_key("command"),
            server.contains_key("url"),
            "runtime MCP server {name:?} must have exactly one transport selector"
        );
    }
}

fn assert_list_has_one_transport_selector(listed: &JsonValue) {
    let servers = listed
        .as_array()
        .expect("ilhae mcp list --json should return an array");

    for server in servers {
        let name = server
            .get("name")
            .and_then(JsonValue::as_str)
            .expect("listed MCP server should have a name");
        let transport = server
            .get("transport")
            .and_then(JsonValue::as_object)
            .unwrap_or_else(|| panic!("listed MCP server {name:?} should have a transport"));
        assert_ne!(
            transport.contains_key("command"),
            transport.contains_key("url"),
            "listed MCP server {name:?} must have exactly one transport selector"
        );
    }
}

fn listed_names(listed: &JsonValue) -> Vec<&str> {
    listed
        .as_array()
        .expect("ilhae mcp list --json should return an array")
        .iter()
        .map(|server| {
            server
                .get("name")
                .and_then(JsonValue::as_str)
                .expect("listed MCP server should have a name")
        })
        .collect()
}

#[test]
fn mcp_list_survives_unparseable_human_config_without_mutating_source() -> Result<()> {
    let invalid_utf8 = b"[mcp_servers.poison]\ncommand = \"node\xff\"\n";
    let cases: [(&str, &[u8]); 2] = [
        (
            "invalid TOML",
            b"[mcp_servers.poison]\ncommand = \"node\"\nurl = ",
        ),
        ("invalid UTF-8", invalid_utf8),
    ];

    for (label, source) in cases {
        let (_temp, listed, runtime) = run_mcp_list(source).with_context(|| label.to_string())?;
        assert_eq!(listed, serde_json::json!([]), "{label}");
        assert_runtime_has_one_transport_selector(&runtime);
        assert_list_has_one_transport_selector(&listed);
        assert!(
            !runtime.to_string().contains("excel-mcp"),
            "{label}: generated runtime must not contain retired excel-mcp"
        );
    }

    Ok(())
}

#[test]
fn mcp_list_filters_poisoned_transports_and_keeps_canonical_url_only() -> Result<()> {
    let cases = [
        (
            "third-party command plus url",
            r#"[mcp_servers.third-party]
command = "third-party-mcp"
url = "https://poison.invalid/mcp"
"#,
        ),
        (
            "office command plus url",
            r#"[mcp_servers.office]
command = "office-mcp"
args = ["--stdio"]
url = "https://office.invalid/mcp"
"#,
        ),
        (
            "office empty url",
            r#"[mcp_servers.office]
url = ""
"#,
        ),
        (
            "office non-http url",
            r#"[mcp_servers.office]
url = "file:///tmp/office.sock"
"#,
        ),
        (
            "office canonical url only before runtime materialization",
            r#"[mcp_servers.office]
url = "https://office.example/mcp"
"#,
        ),
    ];

    for (label, poison) in cases {
        let source = format!(
            r#"{poison}
[mcp_servers.excel-mcp]
command = "retired-excel-mcp"

[mcp_servers.canonical-remote]
url = "https://canonical.example/mcp"
enabled = false
"#
        );
        let (_temp, listed, runtime) =
            run_mcp_list(source.as_bytes()).with_context(|| label.to_string())?;

        assert_eq!(listed_names(&listed), vec!["canonical-remote"], "{label}");
        assert_runtime_has_one_transport_selector(&runtime);
        assert_list_has_one_transport_selector(&listed);

        let runtime_servers = runtime
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .expect("generated runtime config should contain mcp_servers");
        assert_eq!(
            runtime_servers
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["canonical-remote"],
            "{label}"
        );
        assert!(!runtime_servers.contains_key("excel-mcp"), "{label}");
    }

    Ok(())
}
