use super::super::*;
use super::mcp::native_mcp_defaults;
use super::mcp::native_mcp_launchers;
use super::mcp::user_mcp_servers_for_managed_config;
use super::schema::read_valid_ilhae_codex_runtime_config_with_system2;
use super::schema::validate_ilhae_codex_runtime_config;
use super::storage::acquire_ilhae_codex_runtime_config_lock;
use super::storage::install_ilhae_codex_runtime_snapshot_locked;
use super::storage::write_ilhae_codex_runtime_file_atomically;
use super::*;
use sha2::Digest;
use std::sync::Arc;
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: tests mutate env in a scoped, single-process context and restore it on drop.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn preserve(key: &'static str) -> Self {
        Self {
            key,
            previous: std::env::var(key).ok(),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: tests restore process env to its previous value before exiting scope.
        unsafe {
            if let Some(previous) = self.previous.as_deref() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

#[derive(Clone)]
struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture log mutex")
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_warnings<T>(action: impl FnOnce() -> T) -> (T, String) {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer_bytes = Arc::clone(&bytes);
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || CapturedLogWriter(Arc::clone(&writer_bytes)))
        .finish();
    let result = tracing::subscriber::with_default(subscriber, action);
    let output = String::from_utf8(bytes.lock().expect("capture log mutex").clone())
        .expect("captured warnings are UTF-8");
    (result, output)
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    sha2::Sha256::digest(bytes).to_vec()
}

fn preserve_runtime_environment() -> Vec<EnvVarGuard> {
    [
        "CODEX_HOME",
        "ILHAE_RUNTIME",
        "ILHAE_SYSTEM2_ENABLED",
        "ILHAE_SYSTEM2_SOURCE_PROFILE",
        "ILHAE_SYSTEM2_PROFILE",
        "ILHAE_SYSTEM2_BASE_URL",
        "ILHAE_SYSTEM2_MODEL",
    ]
    .into_iter()
    .map(EnvVarGuard::preserve)
    .collect()
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_uses_explicit_runtime_home() {
    let tmp = tempdir().expect("tempdir");
    let config_dir = tmp.path().join("human");
    let data_dir = tmp.path().join("data");
    let runtime_home = tmp.path().join("explicit-runtime-home");
    std::fs::create_dir_all(&config_dir).expect("create human config directory");
    std::fs::write(config_dir.join("config.toml"), "[profile]\n").expect("write human config");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
    let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
    let _runtime_environment = preserve_runtime_environment();

    let prepared = prepare_ilhae_codex_home().expect("prepare explicit runtime home");

    assert_eq!(prepared, runtime_home);
    let active = std::fs::read(runtime_home.join("config.toml")).expect("read active snapshot");
    let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
        .expect("read LKG snapshot");
    assert_eq!(active, lkg);
    validate_ilhae_codex_runtime_config(
        std::str::from_utf8(&active).expect("active snapshot UTF-8"),
    )
    .expect("active snapshot validates");
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_separates_an_aliased_human_config_directory() {
    let tmp = tempdir().expect("tempdir");
    let config_dir = tmp.path().join("shared");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&config_dir).expect("create shared config directory");
    let source = config_dir.join("config.toml");
    let source_bytes = b"[profile]\nactive = \"fable\"\n";
    std::fs::write(&source, source_bytes).expect("write human config");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
    let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &config_dir);
    let _runtime_environment = preserve_runtime_environment();

    let prepared = prepare_ilhae_codex_home().expect("prepare isolated runtime home");

    assert_eq!(prepared, config_dir.join(".ilhae-runtime-home"));
    assert_eq!(
        std::fs::read(&source).expect("read unchanged human config"),
        source_bytes
    );
    let active = std::fs::read(prepared.join("config.toml")).expect("read active snapshot");
    validate_ilhae_codex_runtime_config(
        std::str::from_utf8(&active).expect("active snapshot UTF-8"),
    )
    .expect("isolated active snapshot validates");
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_separates_an_empty_aliased_human_config_directory() {
    let tmp = tempdir().expect("tempdir");
    let config_dir = tmp.path().join("shared");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&config_dir).expect("create empty shared config directory");
    let source = config_dir.join("config.toml");
    assert!(!source.exists());
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
    let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &config_dir);
    let _runtime_environment = preserve_runtime_environment();

    let prepared = prepare_ilhae_codex_home().expect("prepare isolated runtime home");

    assert_eq!(prepared, config_dir.join(".ilhae-runtime-home"));
    assert!(
        !source.exists(),
        "generated runtime must not create the human config file"
    );
    let active = std::fs::read(prepared.join("config.toml")).expect("read active snapshot");
    let lkg = std::fs::read(prepared.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
        .expect("read LKG snapshot");
    assert_eq!(active, lkg);
    validate_ilhae_codex_runtime_config(
        std::str::from_utf8(&active).expect("active snapshot UTF-8"),
    )
    .expect("isolated active snapshot validates");
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_separates_symlink_and_hardlink_aliases() {
    use std::os::unix::fs::symlink;

    let tmp = tempdir().expect("tempdir");
    let config_dir = tmp.path().join("human");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&config_dir).expect("create human config directory");
    let source = config_dir.join("config.toml");
    let source_bytes = b"[profile]\nactive = \"fable\"\n";
    std::fs::write(&source, source_bytes).expect("write human config");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
    let _runtime_environment = preserve_runtime_environment();

    let symlink_home = tmp.path().join("runtime-symlink");
    symlink(&config_dir, &symlink_home).expect("symlink runtime home to human config");
    {
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &symlink_home);
        let prepared = prepare_ilhae_codex_home().expect("separate symlink alias");
        assert_eq!(prepared, symlink_home.join(".ilhae-runtime-home"));
    }
    assert_eq!(
        std::fs::read(&source).expect("read human config after symlink case"),
        source_bytes
    );

    let hardlink_home = tmp.path().join("runtime-hardlink");
    std::fs::create_dir_all(&hardlink_home).expect("create hardlink runtime home");
    std::fs::hard_link(&source, hardlink_home.join("config.toml"))
        .expect("hardlink runtime config to human source");
    {
        let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &hardlink_home);
        let prepared = prepare_ilhae_codex_home().expect("separate hardlink alias");
        assert_eq!(prepared, hardlink_home.join(".ilhae-runtime-home"));
    }
    assert_eq!(
        std::fs::read(&source).expect("read human config after hardlink case"),
        source_bytes
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_filters_reserved_and_poisoned_mcp_entries() {
    let tmp = tempdir().expect("tempdir");
    let config_dir = tmp.path().join("human");
    let data_dir = tmp.path().join("data");
    let runtime_home = tmp.path().join("runtime");
    std::fs::create_dir_all(&config_dir).expect("create human config directory");
    let source = config_dir.join("config.toml");
    let source_bytes = br#"
[profile]

[mcp_servers.valid_stdio]
command = "node"
args = ["server.js"]

[mcp_servers.valid_http]
url = "https://example.com/mcp"

[mcp_servers.office]
command = "node"

[mcp_servers."excel-mcp"]
command = "node"

[mcp_servers.mcpb_custom]
command = "node"

[mcp_servers.mixed_poison]
command = "node"
url = "https://example.com/mcp"

[mcp_servers.missing_transport]
enabled = true

[mcp_servers.empty_stdio]
command = ""

[mcp_servers.whitespace_stdio]
command = "   "

[mcp_servers.empty_http]
url = ""

[mcp_servers.whitespace_http]
url = "   "

[mcp_servers.padded_http]
url = " https://example.com/mcp "

[mcp_servers.ftp_http]
url = "ftp://example.com/mcp"
"#;
    std::fs::write(&source, source_bytes).expect("write human config");
    let source_hash = sha256(source_bytes);
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
    let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
    let _runtime_environment = preserve_runtime_environment();

    let (result, warnings) = capture_warnings(prepare_ilhae_codex_home);
    result.expect("poisoned MCP entries must not stop runtime preparation");

    let source_after = std::fs::read(&source).expect("read unchanged human config");
    assert_eq!(source_after, source_bytes);
    assert_eq!(sha256(&source_after), source_hash);

    let active = std::fs::read(runtime_home.join("config.toml")).expect("read active snapshot");
    let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
        .expect("read LKG snapshot");
    assert_eq!(active, lkg);
    let active_text = std::str::from_utf8(&active).expect("active snapshot UTF-8");
    validate_ilhae_codex_runtime_config(active_text).expect("active snapshot validates");
    let parsed = toml::from_str::<codex_config::config_toml::ConfigToml>(active_text)
        .expect("parse exact Codex config type");
    for preserved in ["valid_stdio", "valid_http", "mcpb_custom", "office"] {
        assert!(
            parsed.mcp_servers.contains_key(preserved),
            "valid human MCP intent must be preserved: {preserved}"
        );
    }
    for excluded in [
        "excel-mcp",
        "mixed_poison",
        "missing_transport",
        "empty_stdio",
        "whitespace_stdio",
        "empty_http",
        "whitespace_http",
        "padded_http",
        "ftp_http",
    ] {
        assert!(
            !parsed.mcp_servers.contains_key(excluded),
            "{excluded} must not reach the runtime snapshot"
        );
    }
    assert!(warnings.contains("invalid MCP configuration entries"));
    for secret in [
        "mixed_poison",
        "url is not supported for stdio",
        source.to_string_lossy().as_ref(),
    ] {
        assert!(
            !warnings.contains(secret),
            "warning leaked poison diagnostic: {secret}"
        );
    }
}

#[test]
fn native_mcp_defaults_preserve_explicit_transports_and_profile_environments() {
    let user: toml::Value = r#"
[mcp_servers.browser]
url = "http://127.0.0.1:18709/mcp"
enabled = false
[mcp_servers.browser.env]
AGENT_BROWSER_PROFILE = "work"
[mcp_servers.email]
command = "/opt/mail/custom-email"
args = ["mcp", "--account", "work"]
[mcp_servers.email.env]
MAIL_MCP_DB_PATH = "/profiles/work/mail.db"
[mcp_servers.brain]
command = "/opt/brain"
args = ["mcp"]
[mcp_servers.office]
command = "node"
args = ["/opt/ugot-office/launcher.mjs", "mcp"]
[mcp_servers.office.env]
UGOT_OFFICE_DATA_DIR = "/profiles/work/uk.ugot.office"
UGOT_SESSION_PATH = "/profiles/work/ugot-session.json"
"#
    .parse()
    .expect("valid explicit configuration");
    let mut servers = user_mcp_servers_for_managed_config(&user);
    let before = servers.clone();
    native_mcp_defaults(&mut servers, &user);
    for name in ["browser", "email", "brain", "office"] {
        for field in ["command", "args", "url", "enabled", "env"] {
            assert_eq!(
                servers[name].get(field),
                before[name].get(field),
                "explicit {name}.{field} changed"
            );
        }
    }
    assert!(servers["browser"].get("env_vars").is_none());
    assert!(servers["email"].get("env_vars").is_none());
    for name in ["brain", "office"] {
        assert!(
            servers[name]["env_vars"]
                .as_array()
                .unwrap()
                .contains(&toml::Value::from("DISPLAY")),
            "{name} must inherit the desktop session"
        );
    }
}

#[test]
fn native_mcp_explicit_product_inherits_gui_without_losing_custom_vars() {
    let user: toml::Value = r#"
[mcp_servers.email]
command = "/home/user/.cargo/bin/email"
args = ["mcp"]
env_vars = ["CUSTOM_MAIL_VAR", "DISPLAY"]
[mcp_servers.email.env]
MAIL_MCP_DB_PATH = "/profiles/personal/mail.db"
[mcp_servers.work]
command = "/opt/ugot-work"
args = ["mcp"]
"#
    .parse()
    .unwrap();
    let mut servers = user_mcp_servers_for_managed_config(&user);
    native_mcp_defaults(&mut servers, &user);
    let email = &servers["email"];
    let names = email["env_vars"].as_array().unwrap();
    assert_eq!(
        names
            .iter()
            .filter(|name| name.as_str() == Some("DISPLAY"))
            .count(),
        1
    );
    assert!(names.contains(&toml::Value::from("XAUTHORITY")));
    assert!(names.contains(&toml::Value::from("CUSTOM_MAIL_VAR")));
    assert_eq!(
        email["env"]["MAIL_MCP_DB_PATH"].as_str(),
        Some("/profiles/personal/mail.db")
    );
    assert!(
        servers["work"]["env_vars"]
            .as_array()
            .unwrap()
            .contains(&toml::Value::from("WAYLAND_DISPLAY"))
    );
}

#[test]
fn native_mcp_script_launchers_inherit_gui_session() {
    let user: toml::Value = r#"
[mcp_servers.browser]
command = "/usr/local/bin/browser"
args = ["mcp"]
[mcp_servers.logo]
command = "/opt/logo/.venv/bin/python"
args = ["/opt/logo-generator/mcp_launcher.py"]
[mcp_servers.video]
command = "node"
args = ["/opt/videoeditor-mcp/launcher.mjs"]
"#
    .parse()
    .unwrap();
    let mut servers = user_mcp_servers_for_managed_config(&user);
    native_mcp_defaults(&mut servers, &user);
    for name in ["browser", "logo", "video"] {
        assert!(
            servers[name]["env_vars"]
                .as_array()
                .unwrap()
                .contains(&toml::Value::from("DISPLAY")),
            "{name} must inherit the desktop session"
        );
    }
}

#[test]
fn native_mcp_alias_projection_preserves_profile_without_duplicate_defaults() {
    let user: toml::Value = r#"
[mcp_servers.agent-browser]
command = "/opt/ugot-browser"
args = ["mcp"]
[mcp_servers.agent-browser.env]
AGENT_BROWSER_PROFILE = "work"
UGOT_BROWSER_STATE_DIR = "/profiles/work/service"
"#
    .parse()
    .expect("valid alias configuration");
    let mut servers = user_mcp_servers_for_managed_config(&user);
    let selected = servers["agent-browser"].clone();
    native_mcp_defaults(&mut servers, &user);
    assert_eq!(servers["browser"]["command"], selected["command"]);
    assert_eq!(servers["browser"]["args"], selected["args"]);
    assert_eq!(servers["browser"]["env"], selected["env"]);
    assert!(
        servers["browser"]["env_vars"]
            .as_array()
            .unwrap()
            .contains(&toml::Value::from("DISPLAY"))
    );
    assert!(!servers.contains_key("agent-browser"));
    assert_eq!(user["mcp_servers"]["agent-browser"], selected);
}

#[test]
fn native_mcp_defaults_do_not_pick_between_multiple_profiles() {
    let user: toml::Value = r#"
[mcp_servers.agent-browser]
url = "http://127.0.0.1:18701/mcp"
[mcp_servers.browser-bridge-mcp]
url = "http://127.0.0.1:18702/mcp"
[mcp_servers.browser-work]
url = "http://127.0.0.1:18703/mcp"
"#
    .parse()
    .expect("valid profile configuration");
    let mut servers = user_mcp_servers_for_managed_config(&user);
    native_mcp_defaults(&mut servers, &user);
    assert!(!servers.contains_key("browser"));
    for name in ["agent-browser", "browser-bridge-mcp", "browser-work"] {
        assert_eq!(servers[name], user["mcp_servers"][name]);
    }
}

#[test]
fn native_mcp_invalid_explicit_browser_never_launches_a_default_profile() {
    for name in ["browser", "agent-browser"] {
        let mut browser = toml::value::Table::new();
        browser.insert("url".into(), "file:///not-an-mcp-endpoint".into());
        let mut configured = toml::value::Table::new();
        configured.insert(name.into(), browser.into());
        let mut root = toml::value::Table::new();
        root.insert("mcp_servers".into(), configured.into());
        let user = toml::Value::Table(root);
        let mut servers = user_mcp_servers_for_managed_config(&user);
        assert!(!servers.contains_key(name));
        native_mcp_defaults(&mut servers, &user);
        assert!(!servers.contains_key("browser"));
    }
}

#[test]
fn native_mcp_office_default_uses_desktop_launcher_and_workspace_environment() {
    let user = toml::Value::Table(toml::value::Table::new());
    let mut servers = toml::value::Table::new();
    native_mcp_defaults(&mut servers, &user);
    let office = &servers["office"];
    let executable = native_mcp_launchers::resolve_command(
        "ugot-office-mcp",
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
    .expect("Office launcher command");
    assert_eq!(office["command"].as_str(), Some(executable.as_str()));
    assert_eq!(
        office["args"].as_array().unwrap(),
        &[toml::Value::from("mcp")]
    );
    let names = office["env_vars"].as_array().unwrap();
    assert!(names.contains(&toml::Value::from("UGOT_OFFICE_DATA_DIR")));
    assert!(names.contains(&toml::Value::from("UGOT_SESSION_PATH")));
    assert!(!office.as_table().unwrap().contains_key("url"));
}

#[test]
#[serial_test::serial]
fn invalid_human_config_preserves_active_and_lkg_without_diagnostic_leaks() {
    let tmp = tempdir().expect("tempdir");
    let config_dir = tmp.path().join("human");
    let data_dir = tmp.path().join("data");
    let runtime_home = tmp.path().join("runtime");
    std::fs::create_dir_all(&config_dir).expect("create human config directory");
    let source = config_dir.join("config.toml");
    std::fs::write(&source, "[profile]\n").expect("write baseline human config");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
    let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
    let _runtime_environment = preserve_runtime_environment();
    prepare_ilhae_codex_home().expect("prepare baseline runtime snapshot");
    let baseline_active =
        std::fs::read(runtime_home.join("config.toml")).expect("read baseline active");
    let baseline_lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
        .expect("read baseline LKG");

    for invalid_source in [
        b"[mcp_servers.poison_fixture\nurl = \"fixture-secret\"\n".to_vec(),
        vec![0xff, 0xfe, b'p', b'o', b'i', b's', b'o', b'n'],
    ] {
        std::fs::write(&source, &invalid_source).expect("write invalid human config");
        let source_hash = sha256(&invalid_source);
        // The preserved active generation has no System2 projection, so no
        // stale environment value may survive recovery.
        unsafe {
            std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
            std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", "stale-source");
            std::env::set_var("ILHAE_SYSTEM2_PROFILE", "stale-profile");
            std::env::set_var("ILHAE_SYSTEM2_BASE_URL", "https://stale.invalid/v1");
            std::env::set_var("ILHAE_SYSTEM2_MODEL", "stale-model");
        }

        let (result, warnings) = capture_warnings(prepare_ilhae_codex_home);
        result.expect("invalid human config must not stop runtime preparation");

        let source_after = std::fs::read(&source).expect("read unchanged invalid source");
        assert_eq!(source_after, invalid_source);
        assert_eq!(sha256(&source_after), source_hash);
        assert_eq!(
            std::fs::read(runtime_home.join("config.toml")).expect("read preserved active"),
            baseline_active
        );
        assert_eq!(
            std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
                .expect("read preserved LKG"),
            baseline_lkg
        );
        for key in [
            "ILHAE_SYSTEM2_ENABLED",
            "ILHAE_SYSTEM2_SOURCE_PROFILE",
            "ILHAE_SYSTEM2_PROFILE",
            "ILHAE_SYSTEM2_BASE_URL",
            "ILHAE_SYSTEM2_MODEL",
        ] {
            assert!(
                std::env::var_os(key).is_none(),
                "stale System2 environment survived recovery: {key}"
            );
        }
        assert!(warnings.contains("Human configuration was unreadable"));
        for secret in [
            "poison_fixture",
            "fixture-secret",
            "url is not supported for stdio",
            source.to_string_lossy().as_ref(),
        ] {
            assert!(
                !warnings.contains(secret),
                "warning leaked human config diagnostic: {secret}"
            );
        }
    }
}

#[test]
#[serial_test::serial]
fn invalid_first_start_override_bootstraps_valid_runtime_snapshot() {
    let tmp = tempdir().expect("tempdir");
    let config_dir = tmp.path().join("human");
    let data_dir = tmp.path().join("data");
    let runtime_home = tmp.path().join("runtime");
    std::fs::create_dir_all(&config_dir).expect("create human config directory");
    let source = config_dir.join("config.toml");
    let invalid_source = b"[features]\nmulti_agent = \"fixture-secret\"\n";
    std::fs::write(&source, invalid_source).expect("write invalid feature override");
    let source_hash = sha256(invalid_source);
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", &data_dir);
    let _runtime_home_guard = EnvVarGuard::set("ILHAE_CODEX_HOME", &runtime_home);
    let _runtime_environment = preserve_runtime_environment();

    let (result, warnings) = capture_warnings(prepare_ilhae_codex_home);
    result.expect("invalid first-start override must fall back to safe defaults");

    let source_after = std::fs::read(&source).expect("read unchanged human config");
    assert_eq!(source_after, invalid_source);
    assert_eq!(sha256(&source_after), source_hash);
    let active = std::fs::read(runtime_home.join("config.toml")).expect("read active snapshot");
    let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
        .expect("read LKG snapshot");
    assert_eq!(active, lkg);
    let active_text = std::str::from_utf8(&active).expect("active snapshot UTF-8");
    validate_ilhae_codex_runtime_config(active_text).expect("safe snapshot validates");
    let active_toml = active_text
        .parse::<toml::Value>()
        .expect("parse safe snapshot TOML");
    assert_eq!(
        active_toml
            .get("features")
            .and_then(toml::Value::as_table)
            .and_then(|features| features.get("multi_agent"))
            .and_then(toml::Value::as_bool),
        Some(true)
    );
    assert!(!active_text.contains("fixture-secret"));
    assert!(warnings.contains("Ignored invalid human configuration overrides"));
    assert!(!warnings.contains("fixture-secret"));
    assert!(!warnings.contains(source.to_string_lossy().as_ref()));
}

#[test]
fn rejected_candidate_cannot_replace_valid_active_or_lkg_snapshot() {
    let tmp = tempdir().expect("tempdir");
    let runtime_home = tmp.path().join("runtime");
    let valid = render_ilhae_codex_runtime_candidate(
        &HumanIlhaeConfigSnapshot::default(),
        &runtime_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE),
    )
    .expect("render valid snapshot");
    install_ilhae_codex_runtime_snapshot_locked(&runtime_home, &valid)
        .expect("install valid snapshot");
    let baseline_active =
        std::fs::read(runtime_home.join("config.toml")).expect("read baseline active");
    let baseline_lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
        .expect("read baseline LKG");

    let poison = "[mcp_servers.poison]\ncommand = \"   \"\n";
    assert!(install_ilhae_codex_runtime_snapshot_locked(&runtime_home, poison).is_err());
    assert_eq!(
        std::fs::read(runtime_home.join("config.toml")).expect("read unchanged active"),
        baseline_active
    );
    assert_eq!(
        std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read unchanged LKG"),
        baseline_lkg
    );
}

#[test]
fn atomic_runtime_file_writer_replaces_existing_contents() {
    let tmp = tempdir().expect("tempdir");
    let destination = tmp.path().join("model_catalog.json");

    write_ilhae_codex_runtime_file_atomically(&destination, b"first")
        .expect("write initial runtime file");
    write_ilhae_codex_runtime_file_atomically(&destination, b"second")
        .expect("replace runtime file");

    assert_eq!(
        std::fs::read(destination).expect("read replaced runtime file"),
        b"second"
    );
}

#[test]
fn valid_dynamic_active_does_not_replace_base_lkg_and_invalid_active_restores_base() {
    let tmp = tempdir().expect("tempdir");
    let runtime_home = tmp.path().join("runtime");
    let candidate_a = render_ilhae_codex_runtime_candidate(
        &HumanIlhaeConfigSnapshot::default(),
        &runtime_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE),
    )
    .expect("render candidate A");
    install_ilhae_codex_runtime_snapshot_locked(&runtime_home, &candidate_a)
        .expect("install candidate A");

    let mut candidate_b_toml = candidate_a
        .parse::<toml::Value>()
        .expect("parse candidate A");
    let mut office = toml::value::Table::new();
    office.insert(
        "url".to_string(),
        toml::Value::String("http://127.0.0.1:43123/mcp".to_string()),
    );
    candidate_b_toml
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
        .expect("candidate MCP table")
        .insert("office".to_string(), toml::Value::Table(office));
    let candidate_b = toml::to_string_pretty(&candidate_b_toml).expect("render candidate B");
    validate_ilhae_codex_runtime_config(&candidate_b).expect("candidate B validates");

    std::fs::write(runtime_home.join("config.toml"), candidate_b.as_bytes())
        .expect("simulate independently updated valid active snapshot");
    assert!(preserve_valid_active_runtime_locked(&runtime_home));
    assert_eq!(
        std::fs::read(runtime_home.join("config.toml")).expect("read preserved active"),
        candidate_b.as_bytes()
    );
    assert_eq!(
        std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read unchanged base LKG"),
        candidate_a.as_bytes()
    );

    std::fs::write(
        runtime_home.join("config.toml"),
        b"[mcp_servers.poison]\ncommand = \"   \"\n",
    )
    .expect("simulate corrupted active snapshot");
    recover_or_bootstrap_ilhae_codex_runtime_locked(
        &runtime_home,
        "Recovering a corrupted active runtime snapshot",
    )
    .expect("restore valid LKG");
    assert_eq!(
        std::fs::read(runtime_home.join("config.toml")).expect("read restored active"),
        candidate_a.as_bytes()
    );
    assert_eq!(
        std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
            .expect("read unchanged LKG"),
        candidate_a.as_bytes()
    );
}

#[test]
fn concurrent_runtime_snapshot_writers_do_not_tear_active_or_lkg() {
    let tmp = tempdir().expect("tempdir");
    let runtime_home = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime_home).expect("create runtime home");
    let candidate_a = render_ilhae_codex_runtime_candidate(
        &HumanIlhaeConfigSnapshot::default(),
        &runtime_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE),
    )
    .expect("render candidate A");
    let mut candidate_b_toml = candidate_a
        .parse::<toml::Value>()
        .expect("parse candidate A");
    candidate_b_toml
        .as_table_mut()
        .expect("candidate root table")
        .insert(
            "model_context_window".to_string(),
            toml::Value::Integer(65_536),
        );
    let candidate_b = toml::to_string_pretty(&candidate_b_toml).expect("render candidate B");
    validate_ilhae_codex_runtime_config(&candidate_b).expect("candidate B validates");

    let runtime_home = Arc::new(runtime_home);
    let handles = (0..12)
        .map(|index| {
            let runtime_home = Arc::clone(&runtime_home);
            let candidate = if index % 2 == 0 {
                candidate_a.clone()
            } else {
                candidate_b.clone()
            };
            std::thread::spawn(move || {
                let _guard = acquire_ilhae_codex_runtime_config_lock(&runtime_home)
                    .expect("acquire runtime snapshot lock")
                    .expect("runtime snapshot lock before timeout");
                install_ilhae_codex_runtime_snapshot_locked(&runtime_home, &candidate)
                    .expect("install serialized runtime snapshot");
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("runtime writer thread");
    }

    let active = std::fs::read(runtime_home.join("config.toml")).expect("read final active");
    let lkg = std::fs::read(runtime_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE))
        .expect("read final LKG");
    assert_eq!(active, lkg);
    assert!(active == candidate_a.as_bytes() || active == candidate_b.as_bytes());
    validate_ilhae_codex_runtime_config(std::str::from_utf8(&active).expect("final active UTF-8"))
        .expect("final active validates");
    let temporary_files = std::fs::read_dir(runtime_home.as_ref())
        .expect("read runtime home")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(".ilhae-runtime.") && name.ends_with(".tmp")
        })
        .count();
    assert_eq!(temporary_files, 0);
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_projects_named_profiles_into_managed_config() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("nemotron-local".to_string());

    let mut nemotron = IlhaeProfileConfig::default();
    nemotron.agent.engine_id = Some("ilhae".to_string());
    nemotron.agent.auto_mode = true;
    nemotron.agent.auto_max_turns = 8;
    nemotron.native_runtime.enabled = true;
    nemotron.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
    nemotron.native_runtime.model_path = "/models/gemma-4-26b.gguf".to_string();
    nemotron.native_runtime.args = vec!["--ctx-size".to_string(), "65536".to_string()];
    config
        .profiles
        .insert("nemotron-local".to_string(), nemotron.clone());

    let mut review = IlhaeProfileConfig::default();
    review.agent.engine_id = Some("openai".to_string());
    review.agent.command = Some("codex".to_string());
    config.profiles.insert("review".to_string(), review.clone());

    save_ilhae_toml_config(&config).expect("save config");
    let config_path = tmp.path().join("config.toml");
    let mut config_toml = std::fs::read_to_string(&config_path).expect("read config");
    config_toml.push_str(
        r#"
[mcp_servers.fortune]
url = "https://fortune.ugot.uk/mcp"
enabled = true
scopes = ["openid", "profile", "email", "mcp.read", "mcp.write"]
oauth_resource = "https://fortune.ugot.uk/mcp"

[model_providers.minimax-turboquant]
name = "MiniMax local"
base_url = "http://127.0.0.1:8080/v1"
wire_api = "responses"
requires_openai_auth = false
"#,
    );
    std::fs::write(config_path, config_toml).expect("write config with mcp");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let codex_home = tmp.path().join("codex-home");
    let managed =
        std::fs::read_to_string(codex_home.join("config.toml")).expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let root = parsed.as_table().expect("root table");

    assert!(root.get("profile").is_none());
    assert_eq!(
        root.get("mcp_oauth_credentials_store")
            .and_then(toml::Value::as_str),
        Some("file")
    );
    assert_eq!(
        root.get("model").and_then(toml::Value::as_str),
        Some("gemma-4-26b")
    );
    assert_eq!(
        root.get("model_provider").and_then(toml::Value::as_str),
        Some("ilhae-native-nemotron-local")
    );
    let agent = root
        .get("agent")
        .and_then(toml::Value::as_table)
        .expect("agent table");
    assert_eq!(
        agent.get("active_profile").and_then(toml::Value::as_str),
        Some("nemotron-local")
    );
    assert_eq!(
        agent.get("autonomous_mode").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        agent
            .get("auto_max_turns")
            .and_then(toml::Value::as_integer),
        Some(8)
    );

    let profiles = root
        .get("profiles")
        .and_then(toml::Value::as_table)
        .expect("profiles table");
    assert!(profiles.contains_key("nemotron-local"));
    assert!(profiles.contains_key("review"));
    assert!(profiles.contains_key("ilhae-active"));

    let nemotron_profile = profiles
        .get("nemotron-local")
        .and_then(toml::Value::as_table)
        .expect("nemotron profile");
    assert_eq!(
        nemotron_profile.get("model").and_then(toml::Value::as_str),
        Some("gemma-4-26b")
    );
    assert_eq!(
        nemotron_profile
            .get("model_provider")
            .and_then(toml::Value::as_str),
        Some("ilhae-native-nemotron-local")
    );
    assert!(nemotron_profile.get("url").is_none());
    assert_eq!(
        nemotron_profile
            .get("model_context_window")
            .and_then(toml::Value::as_integer),
        Some(65_536)
    );

    let review_profile = profiles
        .get("review")
        .and_then(toml::Value::as_table)
        .expect("review profile");
    assert_eq!(
        review_profile
            .get("model_provider")
            .and_then(toml::Value::as_str),
        Some("openai")
    );
    assert!(review_profile.get("url").is_none());

    let mcp_servers = root
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .expect("mcp servers table");
    let fortune = mcp_servers
        .get("fortune")
        .and_then(toml::Value::as_table)
        .expect("fortune mcp server");
    assert_eq!(
        fortune.get("url").and_then(toml::Value::as_str),
        Some("https://fortune.ugot.uk/mcp")
    );

    let codex_config =
        std::fs::read_to_string(codex_home.join("config.toml")).expect("read codex config");
    let codex_config: toml::Value = toml::from_str(&codex_config).expect("parse codex config");
    let codex_mcp_servers = codex_config
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .expect("codex mcp servers table");
    let projected_fortune = codex_mcp_servers
        .get("fortune")
        .and_then(toml::Value::as_table)
        .expect("projected fortune mcp server");
    assert_eq!(projected_fortune, fortune);

    let model_providers = root
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .expect("model providers table");
    let minimax_provider = model_providers
        .get("minimax-turboquant")
        .and_then(toml::Value::as_table)
        .expect("minimax provider");
    assert_eq!(
        minimax_provider
            .get("requires_openai_auth")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    let nemotron_provider = model_providers
        .get("ilhae-native-nemotron-local")
        .and_then(toml::Value::as_table)
        .expect("nemotron provider");
    assert_eq!(
        nemotron_provider
            .get("base_url")
            .and_then(toml::Value::as_str),
        Some("http://127.0.0.1:8081/v1")
    );
    assert_eq!(
        nemotron_provider
            .get("requires_openai_auth")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_projects_foreground_loop_instructions_into_managed_config() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("foreground-local".to_string());

    let mut foreground = IlhaeProfileConfig::default();
    foreground.agent.command = Some("ilhae".to_string());
    foreground.agent.kairos = true;
    foreground.agent.self_improvement = true;
    foreground.agent.self_improvement_preset = "foreground".to_string();
    foreground.knowledge = Some(IlhaeProfileKnowledgeConfig {
        mode: "both".to_string(),
        ..IlhaeProfileKnowledgeConfig::default()
    });
    config
        .profiles
        .insert("foreground-local".to_string(), foreground);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let instructions = parsed
        .get("developer_instructions")
        .and_then(toml::Value::as_str)
        .expect("developer instructions");

    assert!(instructions.contains("ILHAE RUNTIME LOOP STATE"));
    assert!(instructions.contains("- Knowledge loop: enabled (both)"));
    assert!(instructions.contains("- Preset: foreground"));
    assert!(instructions.contains("SELF-IMPROVEMENT SKILL LOOP"));
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_defaults_self_improvement_foreground_loop() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    save_ilhae_toml_config(&IlhaeTomlConfig::default()).expect("save default config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let agent = parsed
        .get("agent")
        .and_then(toml::Value::as_table)
        .expect("agent table");
    let features = parsed
        .get("features")
        .and_then(toml::Value::as_table)
        .expect("features table");
    let instructions = parsed
        .get("developer_instructions")
        .and_then(toml::Value::as_str)
        .expect("developer instructions");

    assert_eq!(
        agent
            .get("self_improvement_enabled")
            .and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        features
            .get("apply_patch_freeform")
            .and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        features
            .get("apply_patch_streaming_events")
            .and_then(toml::Value::as_bool),
        Some(true)
    );
    assert!(instructions.contains("- Self-improvement: enabled"));
    assert!(instructions.contains("- Preset: foreground"));
    assert!(instructions.contains("SELF-IMPROVEMENT SKILL LOOP"));
}

#[test]
#[serial_test::serial]
fn get_native_runtime_config_accepts_non_ilhae_engine_profiles() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("qwen3.6-local".to_string());

    let mut local = IlhaeProfileConfig::default();
    local.agent.engine_id = Some("llama-server".to_string());
    local.agent.command = Some("ilhae".to_string());
    local.native_runtime.enabled = true;
    local.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
    local.native_runtime.health_url = "http://127.0.0.1:8081/health".to_string();
    local.native_runtime.model_path = "/models/Qwen3.6-35B.gguf".to_string();
    config.profiles.insert("qwen3.6-local".to_string(), local);

    save_ilhae_toml_config(&config).expect("save config");

    let (profile_id, runtime) =
        get_native_runtime_config(None).expect("native runtime config for local profile");
    assert_eq!(profile_id, "qwen3.6-local");
    assert_eq!(runtime.base_url, "http://127.0.0.1:8081/v1");
    assert_eq!(runtime.health_url, "http://127.0.0.1:8081/health");
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_scopes_static_catalog_to_native_profiles() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("qwen-local".to_string());

    let mut qwen = IlhaeProfileConfig::default();
    qwen.agent.engine_id = Some("llama-server".to_string());
    qwen.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
    qwen.native_runtime.model_path = "qwen3.6-27b".to_string();
    config.profiles.insert("qwen-local".to_string(), qwen);

    let mut review = IlhaeProfileConfig::default();
    review.agent.engine_id = Some("openai".to_string());
    review.agent.command = Some("codex".to_string());
    config.profiles.insert("review".to_string(), review);

    save_ilhae_toml_config(&config).expect("save config");
    let codex_home = prepare_ilhae_codex_home().expect("prepare codex home");
    let catalog_path = codex_home.join(ILHAE_CODEX_MODEL_CATALOG_FILE);
    let managed =
        std::fs::read_to_string(codex_home.join("config.toml")).expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let profiles = parsed
        .get("profiles")
        .and_then(toml::Value::as_table)
        .expect("profiles table");

    assert_eq!(
        parsed
            .get("model_catalog_json")
            .and_then(toml::Value::as_str),
        catalog_path.to_str()
    );
    assert_eq!(
        profiles
            .get("qwen-local")
            .and_then(|profile| profile.get("model_catalog_json"))
            .and_then(toml::Value::as_str),
        catalog_path.to_str()
    );
    assert!(
        profiles
            .get("review")
            .and_then(|profile| profile.get("model_catalog_json"))
            .is_none()
    );

    let catalog = serde_json::from_slice::<codex_protocol::openai_models::ModelsResponse>(
        &std::fs::read(&catalog_path).expect("read native model catalog"),
    )
    .expect("parse native model catalog");
    assert_eq!(
        catalog,
        codex_protocol::openai_models::ModelsResponse {
            models: vec![{
                let mut model =
                    codex_models_manager::model_info::model_info_from_slug("qwen3.6-27b");
                model.visibility = codex_protocol::openai_models::ModelVisibility::List;
                model
            }],
        }
    );

    config.profile.active = Some("review".to_string());
    save_ilhae_toml_config(&config).expect("save OpenAI-active config");
    prepare_ilhae_codex_home().expect("prepare OpenAI-active codex home");
    let managed =
        std::fs::read_to_string(codex_home.join("config.toml")).expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let profiles = parsed
        .get("profiles")
        .and_then(toml::Value::as_table)
        .expect("profiles table");

    assert!(parsed.get("model_catalog_json").is_none());
    assert!(
        profiles
            .get("review")
            .and_then(|profile| profile.get("model_catalog_json"))
            .is_none()
    );
    assert_eq!(
        profiles
            .get("qwen-local")
            .and_then(|profile| profile.get("model_catalog_json"))
            .and_then(toml::Value::as_str),
        catalog_path.to_str()
    );
}

#[test]
fn native_runtime_effective_urls_fall_back_to_url_alias() {
    let mut config = IlhaeProfileNativeRuntimeConfig::default();
    config.url = Some("http://127.0.0.1:8085/v1".to_string());
    config.health_url = String::new();
    config.base_url = String::new();

    assert_eq!(
        native_runtime_effective_base_url(&config),
        "http://127.0.0.1:8085/v1"
    );
    assert_eq!(
        native_runtime_effective_health_url(&config),
        "http://127.0.0.1:8085/health"
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_projects_non_ilhae_native_profiles_into_managed_config() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("qwen3.6-local".to_string());

    let mut local = IlhaeProfileConfig::default();
    local.agent.engine_id = Some("llama-server".to_string());
    local.agent.command = Some("ilhae".to_string());
    local.native_runtime.enabled = true;
    local.native_runtime.base_url = "http://127.0.0.1:8081/v1".to_string();
    local.native_runtime.provider = Some("sglang".to_string());
    local.native_runtime.model_path = "/models/Qwen3.6-35B-A3B.gguf".to_string();
    config.profiles.insert("qwen3.6-local".to_string(), local);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let profiles = parsed
        .get("profiles")
        .and_then(toml::Value::as_table)
        .expect("profiles table");
    let local_profile = profiles
        .get("qwen3.6-local")
        .and_then(toml::Value::as_table)
        .expect("local profile");

    assert_eq!(
        local_profile.get("model").and_then(toml::Value::as_str),
        Some("Qwen3.6-35B-A3B")
    );
    assert_eq!(
        local_profile
            .get("model_provider")
            .and_then(toml::Value::as_str),
        Some("ilhae-native-qwen3.6-local")
    );
    assert!(local_profile.get("url").is_none());
    let model_providers = parsed
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .expect("model providers table");
    let local_provider = model_providers
        .get("ilhae-native-qwen3.6-local")
        .and_then(toml::Value::as_table)
        .expect("local provider");
    assert_eq!(
        local_provider.get("name").and_then(toml::Value::as_str),
        Some("sglang")
    );
    assert_eq!(
        local_provider.get("base_url").and_then(toml::Value::as_str),
        Some("http://127.0.0.1:8081/v1")
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_projects_external_runtime_into_root_model_config() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();
    let mut query_params = BTreeMap::new();
    query_params.insert("draft".to_string(), "mtp".to_string());
    query_params.insert("ngram-mode".to_string(), "1".to_string());
    let mut http_headers = BTreeMap::new();
    http_headers.insert("X-Test-Header".to_string(), "test-value".to_string());
    let mut env_http_headers = BTreeMap::new();
    env_http_headers.insert("X-Test-Env".to_string(), "TEST_ENV_HEADER".to_string());

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("remote-llama".to_string());

    let mut remote = IlhaeProfileConfig::default();
    remote.agent.engine_id = Some("ilhae".to_string());
    remote.agent.command = Some("ilhae".to_string());
    remote.native_runtime.enabled = false;
    remote.native_runtime.provider = Some("llama-server".to_string());
    remote.native_runtime.base_url = "http://tripleyoung.synology.me:8082/v1".to_string();
    remote.native_runtime.health_url = "http://tripleyoung.synology.me:8082/health".to_string();
    remote.native_runtime.query_params = Some(query_params);
    remote.native_runtime.http_headers = Some(http_headers);
    remote.native_runtime.env_http_headers = Some(env_http_headers);
    config.profiles.insert("remote-llama".to_string(), remote);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

    assert_eq!(
        parsed.get("model").and_then(toml::Value::as_str),
        Some("ilhae")
    );
    assert_eq!(
        parsed.get("model_provider").and_then(toml::Value::as_str),
        Some("ilhae-native-remote-llama")
    );

    let model_providers = parsed
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .expect("model providers");
    let remote_provider = model_providers
        .get("ilhae-native-remote-llama")
        .and_then(toml::Value::as_table)
        .expect("remote provider");
    assert_eq!(
        remote_provider
            .get("query_params")
            .and_then(toml::Value::as_table)
            .and_then(|query| query.get("draft"))
            .and_then(toml::Value::as_str),
        Some("mtp")
    );
    assert_eq!(
        remote_provider
            .get("query_params")
            .and_then(toml::Value::as_table)
            .and_then(|query| query.get("ngram-mode"))
            .and_then(toml::Value::as_str),
        Some("1")
    );
    assert_eq!(
        remote_provider
            .get("http_headers")
            .and_then(toml::Value::as_table)
            .and_then(|headers| headers.get("X-Test-Header"))
            .and_then(toml::Value::as_str),
        Some("test-value")
    );
    assert_eq!(
        remote_provider
            .get("env_http_headers")
            .and_then(toml::Value::as_table)
            .and_then(|headers| headers.get("X-Test-Env"))
            .and_then(toml::Value::as_str),
        Some("TEST_ENV_HEADER")
    );
}

#[test]
fn prepare_ilhae_codex_home_keeps_runtime_args_out_of_request_query_params() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("remote-args".to_string());

    let mut remote = IlhaeProfileConfig::default();
    remote.agent.engine_id = Some("ilhae".to_string());
    remote.agent.command = Some("ilhae".to_string());
    remote.native_runtime.enabled = false;
    remote.native_runtime.provider = Some("llama-server".to_string());
    remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
    remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
    remote.native_runtime.args = vec![
        "draft".to_string(),
        "mtp".to_string(),
        "ngram-mode".to_string(),
        "1".to_string(),
    ];
    config.profiles.insert("remote-args".to_string(), remote);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

    let model_providers = parsed
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .expect("model providers");
    let remote_provider = model_providers
        .get("ilhae-native-remote-args")
        .and_then(toml::Value::as_table)
        .expect("remote provider");
    assert!(remote_provider.get("query_params").is_none());
}

#[test]
fn prepare_ilhae_codex_home_remote_runtime_context_size_from_query_params() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("remote-ctx".to_string());

    let mut query_params = BTreeMap::new();
    query_params.insert("context_size".to_string(), "131072".to_string());

    let mut remote = IlhaeProfileConfig::default();
    remote.agent.engine_id = Some("ilhae".to_string());
    remote.agent.command = Some("ilahe".to_string());
    remote.native_runtime.enabled = false;
    remote.native_runtime.provider = Some("llama-server".to_string());
    remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
    remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
    remote.native_runtime.query_params = Some(query_params);
    config.profiles.insert("remote-ctx".to_string(), remote);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

    let profile = parsed
        .get("profiles")
        .and_then(toml::Value::as_table)
        .and_then(|profiles| profiles.get("remote-ctx"))
        .and_then(toml::Value::as_table)
        .expect("remote profile");
    assert_eq!(
        profile
            .get("model_context_window")
            .and_then(toml::Value::as_integer),
        Some(131_072)
    );

    let model_providers = parsed
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .expect("model providers");
    let remote_provider = model_providers
        .get("ilhae-native-remote-ctx")
        .and_then(toml::Value::as_table)
        .expect("remote provider");
    assert_eq!(
        remote_provider
            .get("query_params")
            .and_then(toml::Value::as_table)
            .and_then(|query| query.get("context_size"))
            .and_then(toml::Value::as_str),
        Some("131072")
    );
}

#[test]
fn prepare_ilhae_codex_home_keeps_context_window_out_of_request_query_params() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("remote-context-window".to_string());

    let mut remote = IlhaeProfileConfig::default();
    remote.agent.engine_id = Some("ilhae".to_string());
    remote.agent.command = Some("ilhae".to_string());
    remote.native_runtime.provider = Some("llama-server".to_string());
    remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
    remote.native_runtime.context_window = Some(16_384);
    config
        .profiles
        .insert("remote-context-window".to_string(), remote);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

    let profile = parsed
        .get("profiles")
        .and_then(toml::Value::as_table)
        .and_then(|profiles| profiles.get("remote-context-window"))
        .and_then(toml::Value::as_table)
        .expect("remote profile");
    assert_eq!(
        profile
            .get("model_context_window")
            .and_then(toml::Value::as_integer),
        Some(16_384)
    );

    let provider = parsed
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .and_then(|providers| providers.get("ilhae-native-remote-context-window"))
        .and_then(toml::Value::as_table)
        .expect("remote provider");
    assert!(provider.get("query_params").is_none());
}

#[test]
fn prepare_ilhae_codex_home_remote_turboquant_query_params() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("remote-turboquant".to_string());

    let mut query_params = BTreeMap::new();
    query_params.insert("draft".to_string(), "mtp".to_string());
    query_params.insert("ngram-mode".to_string(), "1".to_string());
    query_params.insert("cache-type-k".to_string(), "turbo4_0".to_string());
    query_params.insert("cache-type-v".to_string(), "turbo4_0".to_string());

    let mut remote = IlhaeProfileConfig::default();
    remote.agent.engine_id = Some("minimax-turboquant".to_string());
    remote.agent.command = Some("minimax-turboquant".to_string());
    remote.native_runtime.enabled = false;
    remote.native_runtime.provider = Some("minimax-turboquant".to_string());
    remote.native_runtime.base_url = "http://127.0.0.1:8082/v1".to_string();
    remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
    remote.native_runtime.query_params = Some(query_params);
    config
        .profiles
        .insert("remote-turboquant".to_string(), remote);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");

    let model_providers = parsed
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .expect("model providers");
    let provider = model_providers
        .get("ilhae-native-remote-turboquant")
        .and_then(toml::Value::as_table)
        .expect("turboquant provider");
    let provider_query_params = provider
        .get("query_params")
        .and_then(toml::Value::as_table)
        .expect("turboquant query params");

    assert_eq!(
        provider_query_params
            .get("draft")
            .and_then(toml::Value::as_str),
        Some("mtp")
    );
    assert_eq!(
        provider_query_params
            .get("ngram-mode")
            .and_then(toml::Value::as_str),
        Some("1")
    );
    assert_eq!(
        provider_query_params
            .get("cache-type-k")
            .and_then(toml::Value::as_str),
        Some("turbo4_0")
    );
    assert_eq!(
        provider_query_params
            .get("cache-type-v")
            .and_then(toml::Value::as_str),
        Some("turbo4_0")
    );

    let profiles = parsed
        .get("profiles")
        .and_then(toml::Value::as_table)
        .expect("profiles table");
    let profile = profiles
        .get("remote-turboquant")
        .and_then(toml::Value::as_table)
        .expect("remote profile");
    assert_eq!(
        profile.get("model_provider").and_then(toml::Value::as_str),
        Some("ilhae-native-remote-turboquant")
    );
}

#[test]
fn prepare_ilhae_codex_home_derives_remote_runtime_urls_from_proxy_origin() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("remote-proxy".to_string());

    let mut remote = IlhaeProfileConfig::default();
    remote.agent.engine_id = Some("ilhae".to_string());
    remote.agent.command = Some("ilhae".to_string());
    remote.native_runtime.enabled = false;
    remote.native_runtime.provider = Some("llama-server".to_string());
    remote.native_runtime.base_url = String::new();
    remote.native_runtime.proxy_url = Some("https://yth-runtime.example.com/".to_string());
    remote.native_runtime.proxy_token = Some("test-token".to_string());
    remote.native_runtime.env_http_headers = Some(BTreeMap::from([
        (
            "X-Ilhae-Runtime-Token".to_string(),
            "WRONG_PROXY_TOKEN".to_string(),
        ),
        ("X-Test-Env".to_string(), "TEST_ENV_HEADER".to_string()),
    ]));
    remote.native_runtime.health_url = "http://127.0.0.1:8082/health".to_string();
    remote.native_runtime.model_path = "/models/Qwen3.6-27B-Fable-Fusion.gguf".to_string();
    config.profiles.insert("remote-proxy".to_string(), remote);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let model_providers = parsed
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .expect("model providers");
    let remote_provider = model_providers
        .get("ilhae-native-remote-proxy")
        .and_then(toml::Value::as_table)
        .expect("remote provider");
    assert_eq!(
        remote_provider
            .get("base_url")
            .and_then(toml::Value::as_str),
        Some("https://yth-runtime.example.com/v1")
    );
    assert_eq!(
        remote_provider
            .get("http_headers")
            .and_then(toml::Value::as_table)
            .and_then(|headers| headers.get("X-Ilhae-Runtime-Token"))
            .and_then(toml::Value::as_str),
        Some("test-token")
    );
    assert_eq!(
        remote_provider
            .get("env_http_headers")
            .and_then(toml::Value::as_table)
            .and_then(|headers| headers.get("X-Test-Env"))
            .and_then(toml::Value::as_str),
        Some("TEST_ENV_HEADER")
    );
    assert!(
        remote_provider
            .get("env_http_headers")
            .and_then(toml::Value::as_table)
            .is_some_and(|headers| !headers.contains_key("X-Ilhae-Runtime-Token"))
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_preserves_sglang_directory_model_names() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some("qwen3.6-sglang".to_string());

    let mut sglang = IlhaeProfileConfig::default();
    sglang.agent.engine_id = Some("sglang".to_string());
    sglang.agent.command = Some("ilhae".to_string());
    sglang.native_runtime.enabled = true;
    sglang.native_runtime.base_url = "http://192.168.219.113:30000/v1".to_string();
    sglang.native_runtime.provider = Some("sglang".to_string());
    sglang.native_runtime.model_path = "/home/sk/ws/llm/models/Qwen3.6-27B".to_string();
    sglang.native_runtime.args = vec!["--context-length".to_string(), "262144".to_string()];
    config.profiles.insert("qwen3.6-sglang".to_string(), sglang);

    save_ilhae_toml_config(&config).expect("save config");
    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let profile = parsed
        .get("profiles")
        .and_then(toml::Value::as_table)
        .and_then(|profiles| profiles.get("qwen3.6-sglang"))
        .and_then(toml::Value::as_table)
        .expect("sglang profile");

    assert_eq!(
        profile.get("model").and_then(toml::Value::as_str),
        Some("Qwen3.6-27B")
    );
    assert_eq!(
        profile
            .get("model_context_window")
            .and_then(toml::Value::as_integer),
        Some(262144)
    );
}

#[test]
fn profile_runtime_display_parts_identifies_native_provider_and_model() {
    let mut local = IlhaeProfileConfig::default();
    local.agent.engine_id = Some("ilhae".to_string());
    local.native_runtime.enabled = true;
    local.native_runtime.model_path =
        "/models/Qwen3.6-27B-GGUF/Qwen3.6-27B-UD-Q4_K_XL.gguf".to_string();

    assert_eq!(
        profile_runtime_display_parts(&local),
        vec!["llama-server", "Qwen3.6-27B-UD-Q4_K_XL"]
    );

    local.native_runtime.provider = Some("luce-dflash".to_string());

    assert_eq!(
        profile_runtime_display_parts(&local),
        vec!["luce-dflash", "Qwen3.6-27B-UD-Q4_K_XL"]
    );
}

#[test]
fn profile_runtime_display_parts_keeps_remote_engine_and_model_metadata() {
    let mut minimax = IlhaeProfileConfig::default();
    minimax.agent.engine_id = Some("minimax-turboquant".to_string());
    minimax.native_runtime.base_url = "http://192.168.219.113:8080/v1".to_string();
    minimax.native_runtime.model_path =
        "/models/MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf".to_string();

    assert_eq!(
        profile_runtime_display_parts(&minimax),
        vec![
            "minimax-turboquant",
            "MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003",
            "remote",
        ]
    );

    let mut sglang = IlhaeProfileConfig::default();
    sglang.agent.engine_id = Some("sglang".to_string());
    sglang.native_runtime.base_url = "http://192.168.219.113:30000/v1".to_string();
    sglang.native_runtime.model_path = "default".to_string();

    assert_eq!(
        profile_runtime_display_parts(&sglang),
        vec!["sglang", "server-default", "remote"]
    );
}

#[test]
#[serial_test::serial]
fn current_thinking_mode_reads_persisted_setting() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());

    assert_eq!(current_thinking_mode(), "on");

    let settings_store = crate::settings_store::SettingsStore::new(&tmp.path().join("data"));
    settings_store
        .set_value("agent.thinking_mode", serde_json::json!("off"))
        .expect("persist thinking mode");

    assert_eq!(current_thinking_mode(), "off");
    assert!(!current_thinking_enabled());
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_preserves_oauth_credentials() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    std::fs::write(tmp.path().join("config.toml"), "[profile]\n").expect("write config");
    std::fs::write(tmp.path().join(".credentials.json"), r#"{"mcp":"token"}"#)
        .expect("write credentials");
    std::fs::write(tmp.path().join("auth.json"), r#"{"auth":"token"}"#).expect("write auth");

    prepare_ilhae_codex_home().expect("prepare codex home");

    assert_eq!(
        std::fs::read_to_string(tmp.path().join(".credentials.json")).expect("read credentials"),
        r#"{"mcp":"token"}"#
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("codex-home/.credentials.json"))
            .expect("read codex credentials"),
        r#"{"mcp":"token"}"#
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("auth.json")).expect("read auth"),
        r#"{"auth":"token"}"#
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("codex-home/auth.json")).expect("read codex auth"),
        r#"{"auth":"token"}"#
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_disables_duplicate_legacy_fortune_server() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    save_ilhae_toml_config(&IlhaeTomlConfig::default()).expect("save config");
    let config_path = tmp.path().join("config.toml");
    let mut config_toml = std::fs::read_to_string(&config_path).expect("read config");
    config_toml.push_str(
        r#"
[mcp_servers.fortune]
url = "https://fortune.ugot.uk/mcp"
enabled = true
scopes = ["openid", "profile", "email", "mcp.read", "mcp.write"]
oauth_resource = "https://fortune.ugot.uk/mcp"

[mcp_servers.ugot_fortune]
url = "https://fortune.ugot.uk/mcp"
enabled = true
scopes = ["openid", "profile", "email", "offline_access", "mcp.read", "mcp.write"]
oauth_resource = "https://fortune.ugot.uk/mcp"
"#,
    );
    std::fs::write(config_path, config_toml).expect("write config with duplicate fortune");

    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let mcp_servers = parsed
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .expect("mcp servers table");
    let legacy = mcp_servers
        .get("fortune")
        .and_then(toml::Value::as_table)
        .expect("legacy fortune server");
    let canonical = mcp_servers
        .get("ugot_fortune")
        .and_then(toml::Value::as_table)
        .expect("canonical fortune server");

    assert_eq!(
        legacy.get("enabled").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        canonical.get("enabled").and_then(toml::Value::as_bool),
        Some(true)
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_seeds_auth_from_codex_home_fallback() {
    let tmp = tempdir().expect("tempdir");
    let ilhae_dir = tmp.path().join(".ilhae");
    let home_dir = tmp.path().join("home");
    let codex_dir = home_dir.join(".codex");
    std::fs::create_dir_all(&ilhae_dir).expect("create ilhae dir");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &ilhae_dir);
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _home_guard = EnvVarGuard::set("HOME", &home_dir);
    let _runtime_environment = preserve_runtime_environment();

    std::fs::write(ilhae_dir.join("config.toml"), "[profile]\n").expect("write config");
    std::fs::write(codex_dir.join("auth.json"), r#"{"codex":"auth"}"#).expect("write codex auth");

    prepare_ilhae_codex_home().expect("prepare codex home");

    assert_eq!(
        std::fs::read_to_string(ilhae_dir.join("codex-home/auth.json")).expect("read seeded auth"),
        r#"{"codex":"auth"}"#
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_preserves_existing_codex_home_oauth_credentials() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    std::fs::write(tmp.path().join("config.toml"), "[profile]\n").expect("write config");
    let codex_home = tmp.path().join("codex-home");
    std::fs::create_dir_all(&codex_home).expect("create codex home");
    std::fs::write(tmp.path().join(".credentials.json"), r#"{"root":"mcp"}"#)
        .expect("write root credentials");
    std::fs::write(codex_home.join(".credentials.json"), r#"{"codex":"mcp"}"#)
        .expect("write codex credentials");

    prepare_ilhae_codex_home().expect("prepare codex home");

    assert_eq!(
        std::fs::read_to_string(codex_home.join(".credentials.json"))
            .expect("read codex credentials"),
        r#"{"codex":"mcp"}"#
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_removes_stale_runtime_overrides() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let config_toml = r#"
[profile]
active = "qwen3.6-27b-mtp-ud-q4-k-xl"

[profiles."qwen3.6-27b-mtp-ud-q4-k-xl".native_runtime]
enabled = true
model_path = "/models/Qwen3.6-27B-MTP-UD-Q4_K_XL.gguf"
"#;
    std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

    let codex_home = tmp.path().join("codex-home");
    std::fs::create_dir_all(&codex_home).expect("create codex home");
    std::fs::write(
        codex_home.join("config.toml"),
        r#"model = "Qwen3.6-27B-UD-Q4_K_XL"
model_provider = "ilhae-native-qwen3.6-27b-ud-q4-k-xl"
profile = "qwen3.6-27b-ud-q4-k-xl"

[projects."/mnt/nvme0n1p2/workspace/monorepo"]
trust_level = "trusted"
"#,
    )
    .expect("write stale codex config");

    prepare_ilhae_codex_home().expect("prepare codex home");

    let sanitized =
        std::fs::read_to_string(codex_home.join("config.toml")).expect("read codex config");
    let parsed: toml::Value = toml::from_str(&sanitized).expect("parse managed codex config");
    assert_eq!(
        parsed.get("model").and_then(toml::Value::as_str),
        Some("Qwen3.6-27B-MTP-UD-Q4_K_XL")
    );
    assert_eq!(
        parsed.get("model_provider").and_then(toml::Value::as_str),
        Some("llama-server")
    );
    assert!(parsed.get("profile").is_none());
    assert!(!sanitized.contains("Qwen3.6-27B-UD-Q4_K_XL"));
    assert!(!sanitized.contains("ilhae-native-qwen3.6-27b-ud-q4-k-xl"));
    assert!(parsed.get("projects").is_none());
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_carries_project_trust_into_managed_config() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let config_toml = r#"
[profile]
active = "qwen-35b-sglang"

[profiles.qwen-35b-sglang.agent]
engine = "sglang"
command = "ilhae"

[projects."/mnt/nvme0n1p2/workspace/monorepo"]
trust_level = "trusted"

[projects."/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent"]
trust_level = "untrusted"
"#;
    std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let projects = parsed
        .get("projects")
        .and_then(toml::Value::as_table)
        .expect("projects table");

    let trusted = projects
        .get("/mnt/nvme0n1p2/workspace/monorepo")
        .and_then(toml::Value::as_table)
        .expect("trusted project entry");
    assert_eq!(
        trusted.get("trust_level").and_then(toml::Value::as_str),
        Some("trusted")
    );

    let untrusted = projects
        .get("/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent")
        .and_then(toml::Value::as_table)
        .expect("untrusted project entry");
    assert_eq!(
        untrusted.get("trust_level").and_then(toml::Value::as_str),
        Some("untrusted")
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_carries_web_search_into_managed_config() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let config_toml = r#"
web_search = "live"

[tools.web_search]
engine = "duckduckgo"
use_duckduckgo_fallback = true

[profile]
active = "qwen-local"

[profiles.qwen-local.agent]
engine = "ilhae"
command = "ilhae"
"#;
    std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    assert_eq!(
        parsed.get("web_search").and_then(toml::Value::as_str),
        Some("live")
    );

    let web_search = parsed
        .get("tools")
        .and_then(toml::Value::as_table)
        .and_then(|tools| tools.get("web_search"))
        .and_then(toml::Value::as_table)
        .expect("tools.web_search table");
    assert_eq!(
        web_search.get("engine").and_then(toml::Value::as_str),
        Some("duckduckgo")
    );
    assert_eq!(
        web_search
            .get("use_duckduckgo_fallback")
            .and_then(toml::Value::as_bool),
        Some(true)
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_carries_features_into_managed_config() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let config_toml = r#"
[features]
goals = true
fast_mode = false

[profile]
active = "qwen-local"

[profiles.qwen-local.agent]
engine = "ilhae"
command = "ilhae"
"#;
    std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

    prepare_ilhae_codex_home().expect("prepare codex home");

    let managed = std::fs::read_to_string(tmp.path().join("codex-home/config.toml"))
        .expect("read generated config");
    let parsed: toml::Value = toml::from_str(&managed).expect("parse generated config");
    let features = parsed
        .get("features")
        .and_then(toml::Value::as_table)
        .expect("features table");

    assert_eq!(
        features.get("goals").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        features.get("fast_mode").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        features
            .get("apply_patch_freeform")
            .and_then(toml::Value::as_bool),
        Some(true)
    );
}

#[test]
#[serial_test::serial]
fn prepare_ilhae_codex_home_persists_and_recovers_system2_projection() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let config_toml = r#"
[profile]
active = "qwen3.6-local"

[profiles."qwen3.6-local".agent]
engine = "ilhae"
command = "ilhae"

[profiles."qwen3.6-local".system2]
enabled = true
profile = "minimax-m2.7-turboquant"

[profiles."minimax-m2.7-turboquant".agent]
engine = "ilhae"
command = "ilhae"

[profiles."minimax-m2.7-turboquant".native_runtime]
enabled = false
base_url = "http://192.168.219.113:8080/v1"
model_path = "/home/sk/models/MiniMax-M2.7-GGUF/UD-IQ3_XXS/MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf"
"#;
    std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

    let codex_home = prepare_ilhae_codex_home().expect("prepare codex home");
    let expected = ResolvedSystem2TargetConfig {
        source_profile_id: "qwen3.6-local".to_string(),
        target_profile_id: "minimax-m2.7-turboquant".to_string(),
        base_url: "http://192.168.219.113:8080/v1".to_string(),
        model_name: "MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf".to_string(),
    };

    assert!(
        std::env::var("ILHAE_SYSTEM2_ENABLED").ok().as_deref() == Some("1"),
        "system2 should be enabled"
    );
    assert_eq!(
        std::env::var("ILHAE_SYSTEM2_SOURCE_PROFILE")
            .ok()
            .as_deref(),
        Some("qwen3.6-local")
    );
    assert_eq!(
        std::env::var("ILHAE_SYSTEM2_PROFILE").ok().as_deref(),
        Some("minimax-m2.7-turboquant")
    );
    assert_eq!(
        std::env::var("ILHAE_SYSTEM2_BASE_URL").ok().as_deref(),
        Some("http://192.168.219.113:8080/v1")
    );
    assert_eq!(
        std::env::var("ILHAE_SYSTEM2_MODEL").ok().as_deref(),
        Some("MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf")
    );
    assert_eq!(get_active_system2_target_config(), Some(expected.clone()));

    let active_path = codex_home.join("config.toml");
    let lkg_path = codex_home.join(ILHAE_CODEX_RUNTIME_CONFIG_LKG_FILE);
    let baseline_active = std::fs::read(&active_path).expect("read System2 active snapshot");
    let baseline_lkg = std::fs::read(&lkg_path).expect("read System2 LKG snapshot");
    assert_eq!(baseline_active, baseline_lkg);
    let (_, persisted_projection) =
        read_valid_ilhae_codex_runtime_config_with_system2(&active_path)
            .expect("read validated System2 snapshot");
    assert_eq!(persisted_projection, Some(expected.clone()));

    let active_document = std::str::from_utf8(&baseline_active)
        .expect("System2 active snapshot UTF-8")
        .parse::<toml::Value>()
        .expect("parse System2 active snapshot");
    let projection = active_document
        .get("desktop")
        .and_then(toml::Value::as_table)
        .and_then(|desktop| desktop.get(ILHAE_RUNTIME_SYSTEM2_PROJECTION_KEY))
        .and_then(toml::Value::as_table)
        .expect("opaque desktop System2 projection");
    assert_eq!(
        projection
            .get("schema_version")
            .and_then(toml::Value::as_integer),
        Some(ILHAE_RUNTIME_SYSTEM2_PROJECTION_SCHEMA_VERSION)
    );
    assert_eq!(
        projection
            .get("source_profile_id")
            .and_then(toml::Value::as_str),
        Some("qwen3.6-local")
    );
    assert_eq!(
        projection
            .get("target_profile_id")
            .and_then(toml::Value::as_str),
        Some("minimax-m2.7-turboquant")
    );
    assert_eq!(
        projection.get("base_url").and_then(toml::Value::as_str),
        Some("http://192.168.219.113:8080/v1")
    );
    assert_eq!(
        projection.get("model_name").and_then(toml::Value::as_str),
        Some("MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf")
    );

    // A failed human generation must keep both validated runtime files and
    // repopulate every environment value from that final active generation.
    std::fs::write(
        tmp.path().join("config.toml"),
        b"[profile\ninvalid = true\n",
    )
    .expect("write invalid human config");
    unsafe {
        std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
        std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", "stale-source");
        std::env::set_var("ILHAE_SYSTEM2_PROFILE", "stale-target");
        std::env::set_var("ILHAE_SYSTEM2_BASE_URL", "https://stale.invalid/v1");
        std::env::set_var("ILHAE_SYSTEM2_MODEL", "stale-model");
    }
    prepare_ilhae_codex_home().expect("recover System2 projection from final active");

    assert_eq!(
        std::fs::read(&active_path).expect("read recovered active snapshot"),
        baseline_active
    );
    assert_eq!(
        std::fs::read(&lkg_path).expect("read unchanged System2 LKG snapshot"),
        baseline_lkg
    );
    assert_eq!(get_active_system2_target_config(), Some(expected));
}

#[test]
#[serial_test::serial]
fn active_profile_system2_resolves_target_profile() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();
    // This test exercises the human-config API, not the managed runtime projection.
    unsafe {
        std::env::remove_var("ILHAE_RUNTIME");
    }

    let config_toml = r#"
[profile]
active = "qwen3.6-local"

[profiles."qwen3.6-local".agent]
engine = "ilhae"
command = "ilhae"

[profiles."qwen3.6-local".system2]
enabled = true
profile = "minimax-m2.7-turboquant"

[profiles."minimax-m2.7-turboquant".agent]
engine = "minimax-turboquant"
command = "ilhae"

[profiles."minimax-m2.7-turboquant".native_runtime]
enabled = false
base_url = "http://192.168.219.113:8080/v1"
model_path = "/home/sk/models/MiniMax-M2.7-GGUF/UD-IQ3_XXS/MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf"
"#;
    std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write config");

    let resolved = get_active_system2_target_config().expect("system2 target");
    assert_eq!(resolved.source_profile_id, "qwen3.6-local");
    assert_eq!(resolved.target_profile_id, "minimax-m2.7-turboquant");
    assert_eq!(resolved.base_url, "http://192.168.219.113:8080/v1");
    assert_eq!(
        resolved.model_name,
        "MiniMax-M2.7-UD-IQ3_XXS-00001-of-00003.gguf"
    );
}

#[test]
#[serial_test::serial]
fn runtime_system2_getter_requires_complete_env_without_human_fallback() {
    let tmp = tempdir().expect("tempdir");
    let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", tmp.path());
    let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data").as_path());
    let _runtime_environment = preserve_runtime_environment();

    let config_toml = r#"
[profile]
active = "human-source"

[profiles.human-source.system2]
enabled = true
profile = "human-target"

[profiles.human-target.native_runtime]
base_url = "https://human.invalid/v1"
model_path = "/models/human-model.gguf"
"#;
    std::fs::write(tmp.path().join("config.toml"), config_toml).expect("write human config");

    unsafe {
        std::env::set_var("ILHAE_RUNTIME", "1");
        std::env::remove_var("ILHAE_SYSTEM2_ENABLED");
        std::env::remove_var("ILHAE_SYSTEM2_SOURCE_PROFILE");
        std::env::remove_var("ILHAE_SYSTEM2_PROFILE");
        std::env::remove_var("ILHAE_SYSTEM2_BASE_URL");
        std::env::remove_var("ILHAE_SYSTEM2_MODEL");
    }
    assert_eq!(get_active_system2_target_config(), None);

    unsafe {
        std::env::set_var("ILHAE_SYSTEM2_ENABLED", "1");
        std::env::set_var("ILHAE_SYSTEM2_SOURCE_PROFILE", "runtime-source");
        std::env::set_var("ILHAE_SYSTEM2_PROFILE", "runtime-target");
        std::env::set_var("ILHAE_SYSTEM2_BASE_URL", "https://runtime.test/v1");
    }
    assert_eq!(
        get_active_system2_target_config(),
        None,
        "an incomplete runtime projection must not fall back to human config"
    );

    unsafe {
        std::env::set_var("ILHAE_SYSTEM2_MODEL", "runtime-model.gguf");
    }
    assert_eq!(
        get_active_system2_target_config(),
        Some(ResolvedSystem2TargetConfig {
            source_profile_id: "runtime-source".to_string(),
            target_profile_id: "runtime-target".to_string(),
            base_url: "https://runtime.test/v1".to_string(),
            model_name: "runtime-model.gguf".to_string(),
        })
    );
}
