use super::*;
use crate::config::IlhaeTomlConfig;
use serial_test::serial;

/// Serves 200 on /health and 404 elsewhere, like the Laya router's readiness probe.
const FAKE_SIDECAR: &str = r#"
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == "/health" else 404)
        self.end_headers()
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
"#;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: the tests are serialized and restore the variable on drop.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: restores the value captured in `set` while tests are serialized.
        unsafe {
            match self.previous.as_ref() {
                Some(previous) => std::env::set_var(self.key, previous),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Points data and config dirs at a temp dir so tests never touch `~/.ilhae`.
struct Sandbox {
    dir: tempfile::TempDir,
    _guards: Vec<EnvVarGuard>,
}

fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().expect("tempdir");
    let guards = vec![
        EnvVarGuard::set("ILHAE_DATA_DIR", dir.path()),
        EnvVarGuard::set("ILHAE_CONFIG_DIR", dir.path()),
    ];
    Sandbox {
        dir,
        _guards: guards,
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

fn fake_sidecar(name: &str, port: u16) -> IlhaeProfileSidecarConfig {
    IlhaeProfileSidecarConfig {
        name: name.to_string(),
        command: vec![
            "python3".to_string(),
            "-c".to_string(),
            FAKE_SIDECAR.to_string(),
            port.to_string(),
        ],
        health_url: format!("http://127.0.0.1:{port}/health"),
        startup_timeout_secs: 20,
        ..IlhaeProfileSidecarConfig::default()
    }
}

fn profile_with(
    sidecars: Vec<IlhaeProfileSidecarConfig>,
    stop_when_unused: bool,
) -> IlhaeProfileConfig {
    IlhaeProfileConfig {
        sidecars,
        stop_when_unused,
        ..IlhaeProfileConfig::default()
    }
}

fn write_config(sandbox: &Sandbox, profile_id: &str, profile: &IlhaeProfileConfig) {
    let mut config = IlhaeTomlConfig::default();
    config.profile.active = Some(profile_id.to_string());
    config
        .profiles
        .insert(profile_id.to_string(), profile.clone());
    let body = toml::to_string(&config).expect("serialize config");
    std::fs::write(sandbox.dir.path().join("config.toml"), body).expect("write config");
}

fn owned_record(profile_id: &str, name: &str) -> SidecarRecord {
    read_record(&record_path(profile_id, name)).expect("sidecar record")
}

#[test]
fn process_state_reads_own_start_time() {
    let (state, ticks) = process_state(std::process::id()).expect("own stat");
    assert_ne!(state, 'Z');
    assert!(ticks > 0);
    assert!(process_alive(std::process::id(), ticks));
    assert!(!process_alive(std::process::id(), ticks + 1));
}

#[tokio::test]
#[serial]
async fn sidecar_is_started_reused_and_stopped() {
    let _sandbox = sandbox();
    let port = free_port();
    let profile = profile_with(vec![fake_sidecar("router", port)], true);

    ensure_profile_sidecars("laya", &profile)
        .await
        .expect("first ensure");
    let first = owned_record("laya", "router");
    assert!(
        record_attested(&first),
        "started sidecar carries the owner token"
    );

    ensure_profile_sidecars("laya", &profile)
        .await
        .expect("second ensure");
    assert_eq!(
        owned_record("laya", "router").pid,
        first.pid,
        "healthy sidecar is reused"
    );

    stop_profile_sidecars("laya").await.expect("stop");
    assert!(!process_alive(first.pid, first.process_start_ticks));
    assert!(!record_path("laya", "router").exists());
}

#[tokio::test]
#[serial]
async fn sidecar_that_exits_early_reports_it() {
    let _sandbox = sandbox();
    let mut sidecar = fake_sidecar("broken", free_port());
    sidecar.command = vec![
        "python3".to_string(),
        "-c".to_string(),
        "import sys; sys.exit(3)".to_string(),
    ];

    let error = ensure_profile_sidecars("laya", &profile_with(vec![sidecar], false))
        .await
        .expect_err("a sidecar that exits cannot become healthy");
    let message = format!("{error:#}");
    assert!(
        message.contains("exited") && message.contains("broken"),
        "unexpected error: {message}"
    );
    assert!(!record_path("laya", "broken").exists());
}

#[tokio::test]
#[serial]
async fn manually_started_sidecar_is_reused_but_never_stopped() {
    let _sandbox = sandbox();
    let port = free_port();
    let mut manual = std::process::Command::new("python3")
        .args(["-c", FAKE_SIDECAR, &port.to_string()])
        .spawn()
        .expect("spawn manual server");
    let sidecar = fake_sidecar("router", port);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !health_ok(&sidecar.health_url).await {
        assert!(
            std::time::Instant::now() < deadline,
            "manual server did not come up"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    ensure_profile_sidecars("laya", &profile_with(vec![sidecar], true))
        .await
        .expect("ensure reuses the healthy instance");
    assert!(
        !record_path("laya", "router").exists(),
        "nothing was spawned"
    );

    stop_profile_sidecars("laya").await.expect("stop");
    assert!(
        manual.try_wait().expect("try_wait").is_none(),
        "unowned server keeps running"
    );
    manual.kill().expect("kill manual server");
    let _ = manual.wait();
}

#[tokio::test]
#[serial]
async fn last_client_release_stops_unused_profile_services() {
    let sandbox = sandbox();
    let profile = profile_with(vec![fake_sidecar("router", free_port())], true);
    write_config(&sandbox, "laya", &profile);

    register_runtime_client("laya").expect("register");
    ensure_profile_sidecars("laya", &profile)
        .await
        .expect("ensure");
    let record = owned_record("laya", "router");

    // A client that died without releasing must not keep the services alive.
    std::fs::write(clients_dir().join("999999"), "1\nlaya\n").expect("stale lease");

    release_runtime_client().await;
    assert!(
        !process_alive(record.pid, record.process_start_ticks),
        "sidecar stopped"
    );
    assert!(live_runtime_clients().is_empty());
    assert!(!clients_dir().join("999999").exists(), "stale lease pruned");
}

#[tokio::test]
#[serial]
async fn release_keeps_services_while_another_client_lives() {
    let sandbox = sandbox();
    let profile = profile_with(vec![fake_sidecar("router", free_port())], true);
    write_config(&sandbox, "laya", &profile);

    register_runtime_client("laya").expect("register");
    ensure_profile_sidecars("laya", &profile)
        .await
        .expect("ensure");
    let record = owned_record("laya", "router");

    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn other client");
    let (_, ticks) = process_state(other.id()).expect("other stat");
    std::fs::write(
        clients_dir().join(other.id().to_string()),
        format!("{ticks}\nlaya\n"),
    )
    .expect("other lease");

    release_runtime_client().await;
    assert!(
        process_alive(record.pid, record.process_start_ticks),
        "sidecar kept for the other client"
    );

    other.kill().expect("kill other client");
    let _ = other.wait();
    stop_profile_sidecars("laya").await.expect("cleanup");
}

#[tokio::test]
#[serial]
async fn release_leaves_profiles_without_stop_when_unused_running() {
    let sandbox = sandbox();
    let profile = profile_with(vec![fake_sidecar("router", free_port())], false);
    write_config(&sandbox, "keep", &profile);

    register_runtime_client("keep").expect("register");
    ensure_profile_sidecars("keep", &profile)
        .await
        .expect("ensure");
    let record = owned_record("keep", "router");

    release_runtime_client().await;
    assert!(
        process_alive(record.pid, record.process_start_ticks),
        "opt-in only"
    );
    stop_profile_sidecars("keep").await.expect("cleanup");
}

#[test]
fn profile_service_keys_parse_and_stay_out_of_plain_profiles() {
    let config: IlhaeTomlConfig = toml::from_str(
        r#"
[profiles.laya]
backend_profile = "bonsai"
stop_when_unused = true

[[profiles.laya.sidecars]]
name = "laya-router"
command = ["/usr/bin/python3", "router_server.py"]
cwd = "/srv/laya"
health_url = "http://127.0.0.1:8900/health"
[profiles.laya.sidecars.env]
HF_HOME = "/srv/hf"

[profiles.plain.agent]
engine = "openai"
"#,
    )
    .expect("parse profile services");
    let laya = &config.profiles["laya"];
    assert_eq!(laya.backend_profile.as_deref(), Some("bonsai"));
    assert!(laya.stop_when_unused);
    assert_eq!(laya.sidecars.len(), 1);
    assert_eq!(
        laya.sidecars[0].startup_timeout_secs, 180,
        "default timeout"
    );
    assert_eq!(laya.sidecars[0].env["HF_HOME"], "/srv/hf");

    let written = toml::to_string(&config).expect("serialize");
    let reparsed: IlhaeTomlConfig = toml::from_str(&written).expect("reparse");
    assert_eq!(
        reparsed.profiles["laya"].sidecars, laya.sidecars,
        "round trip keeps sidecars"
    );
    let plain = toml::to_string(&config.profiles["plain"]).expect("serialize plain");
    for key in ["backend_profile", "sidecars", "stop_when_unused"] {
        assert!(
            !plain.contains(key),
            "default `{key}` must not be written: {plain}"
        );
    }
}
