//! Profile sidecars and runtime client leases.
//!
//! A profile can declare helper services under `[[profiles.<id>.sidecars]]` that must
//! run while an Ilhae client uses the profile, e.g. the Laya router in front of the
//! profile's native runtime. Ilhae starts them detached, reuses any healthy instance,
//! and only ever signals processes it started itself (PID start time plus an owner
//! token in the environment, the same attestation the native runtime uses).
//!
//! Long-lived clients (interactive CLI, `exec`, app-server) hold a lease file in
//! `run/runtime-clients/<pid>`. When the last live client releases its lease, profiles
//! with `stop_when_unused = true` get their sidecars and native runtimes (their own and
//! their `backend_profile`'s) stopped, so nothing stays resident on the GPU once no
//! Ilhae client is running. A client that dies without releasing leaves a stale lease;
//! the next client prunes it.

use crate::config::IlhaeProfileConfig;
use crate::config::IlhaeProfileSidecarConfig;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tracing::info;
use tracing::warn;

const SIDECAR_RECORD_SCHEMA_VERSION: u32 = 1;
const SIDECAR_OWNER_TOKEN_ENV: &str = "ILHAE_SIDECAR_OWNER_TOKEN";
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(1);
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(15);
const KILL_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The lease this process registered, released once at exit.
static CLIENT_LEASE: Mutex<Option<ClientLease>> = Mutex::new(None);

#[derive(Debug, Clone)]
struct ClientLease {
    profile_id: String,
    path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct SidecarRecord {
    schema_version: u32,
    profile_id: String,
    name: String,
    pid: u32,
    process_start_ticks: u64,
    owner_token: String,
    command: Vec<String>,
}

fn run_dir() -> PathBuf {
    crate::config::resolve_ilhae_data_dir().join("run")
}

fn sidecar_dir() -> PathBuf {
    run_dir().join("sidecars")
}

fn clients_dir() -> PathBuf {
    run_dir().join("runtime-clients")
}

fn clean_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn file_stem(profile_id: &str, name: &str) -> String {
    format!("{}--{}", clean_component(profile_id), clean_component(name))
}

fn record_path(profile_id: &str, name: &str) -> PathBuf {
    sidecar_dir().join(format!("{}.json", file_stem(profile_id, name)))
}

/// Exclusive `flock` held for the guard's lifetime.
struct FileLock {
    _file: std::fs::File,
}

fn lock_file(path: &Path) -> anyhow::Result<FileLock> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: flock is called on a valid descriptor owned by `file`; the lock is
        // released when the descriptor is closed as the guard drops.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(anyhow::Error::from(std::io::Error::last_os_error()));
        }
    }
    Ok(FileLock { _file: file })
}

/// `(state, start_ticks)` from `/proc/<pid>/stat`.
fn process_state(pid: u32) -> Option<(char, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let command_end = stat.rfind(')')?;
    let mut fields = stat.get(command_end + 1..)?.split_whitespace();
    let state = fields.next()?.chars().next()?;
    // After the state (field 3), starttime is field 22.
    let start_ticks = fields.nth(18)?.parse().ok()?;
    Some((state, start_ticks))
}

/// Alive and still the same process (not a zombie, not a reused PID).
fn process_alive(pid: u32, start_ticks: u64) -> bool {
    matches!(process_state(pid), Some((state, ticks)) if ticks == start_ticks && state != 'Z' && state != 'X')
}

fn environ_contains(pid: u32, entry: &str) -> bool {
    std::fs::read(format!("/proc/{pid}/environ"))
        .map(|environ| {
            environ
                .split(|byte| *byte == 0)
                .any(|item| item == entry.as_bytes())
        })
        .unwrap_or(false)
}

fn record_attested(record: &SidecarRecord) -> bool {
    process_alive(record.pid, record.process_start_ticks)
        && environ_contains(
            record.pid,
            &format!("{SIDECAR_OWNER_TOKEN_ENV}={}", record.owner_token),
        )
}

fn read_record(path: &Path) -> Option<SidecarRecord> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<SidecarRecord>(&bytes)
        .ok()
        .filter(|record| record.schema_version == SIDECAR_RECORD_SCHEMA_VERSION)
}

fn write_record(record: &SidecarRecord) -> anyhow::Result<()> {
    let path = record_path(&record.profile_id, &record.name);
    std::fs::create_dir_all(sidecar_dir())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(record)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

async fn health_ok(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(HEALTH_PROBE_TIMEOUT)
        .build()
    else {
        return false;
    };
    matches!(client.get(url).send().await, Ok(response) if response.status().is_success())
}

fn validate_sidecar(sidecar: &IlhaeProfileSidecarConfig) -> anyhow::Result<()> {
    if sidecar.name.trim().is_empty() {
        anyhow::bail!("sidecar is missing `name`");
    }
    if sidecar
        .command
        .first()
        .map(|program| program.trim().is_empty())
        .unwrap_or(true)
    {
        anyhow::bail!("sidecar `{}` is missing `command`", sidecar.name);
    }
    if sidecar.health_url.trim().is_empty() {
        anyhow::bail!("sidecar `{}` is missing `health_url`", sidecar.name);
    }
    Ok(())
}

fn resolve_program(program: &str) -> anyhow::Result<PathBuf> {
    let candidate = Path::new(program);
    if candidate.is_absolute() {
        return Ok(candidate.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
        .ok_or_else(|| anyhow::anyhow!("sidecar program `{program}` not found on PATH"))
}

fn spawn_sidecar(
    profile_id: &str,
    sidecar: &IlhaeProfileSidecarConfig,
) -> anyhow::Result<SidecarRecord> {
    use std::process::Stdio;

    let program = resolve_program(sidecar.command[0].trim())?;
    let owner_token = uuid::Uuid::new_v4().to_string();
    let mut command = std::process::Command::new(&program);
    command
        .args(&sidecar.command[1..])
        .envs(&sidecar.env)
        .env(SIDECAR_OWNER_TOKEN_ENV, &owner_token)
        .stdin(Stdio::null());
    if !sidecar.cwd.trim().is_empty() {
        command.current_dir(sidecar.cwd.trim());
    }
    if sidecar.log_file.trim().is_empty() {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    } else {
        let log_path = Path::new(sidecar.log_file.trim());
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        command.stdout(log.try_clone()?).stderr(log);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group: terminal signals (Ctrl+C in the TUI) must not reach the
        // sidecar, and stopping it can signal the whole group.
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|error| {
        anyhow::anyhow!(
            "failed to start sidecar `{}` ({}): {error}",
            sidecar.name,
            program.display()
        )
    })?;
    let pid = child.id();
    let started = std::time::Instant::now();
    let process_start_ticks = loop {
        if let Some((_, ticks)) = process_state(pid) {
            break ticks;
        }
        if started.elapsed() > Duration::from_secs(1) {
            let _ = child.kill();
            anyhow::bail!("sidecar `{}` exited immediately", sidecar.name);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Reap the child while this process lives; after we exit, init adopts it.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    let record = SidecarRecord {
        schema_version: SIDECAR_RECORD_SCHEMA_VERSION,
        profile_id: profile_id.to_string(),
        name: sidecar.name.clone(),
        pid,
        process_start_ticks,
        owner_token,
        command: sidecar.command.clone(),
    };
    write_record(&record)?;
    info!(profile = %profile_id, sidecar = %sidecar.name, pid, "[Sidecar] started");
    Ok(record)
}

/// Makes sure the sidecar answers its health check, starting it if needed.
async fn ensure_sidecar(
    profile_id: &str,
    sidecar: &IlhaeProfileSidecarConfig,
) -> anyhow::Result<()> {
    validate_sidecar(sidecar)?;
    let path = record_path(profile_id, &sidecar.name);
    let _lock = lock_file(&path.with_extension("lock"))?;

    let owned = read_record(&path).filter(record_attested);
    if owned.is_none() && path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    if health_ok(&sidecar.health_url).await {
        // Healthy: either ours or a manually started instance we must not touch.
        return Ok(());
    }

    let record = match owned {
        // Ours but not healthy yet: another client may have just started it.
        Some(record) => record,
        None => spawn_sidecar(profile_id, sidecar)?,
    };
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(sidecar.startup_timeout_secs.max(1));
    loop {
        if health_ok(&sidecar.health_url).await {
            info!(profile = %profile_id, sidecar = %sidecar.name, pid = record.pid, "[Sidecar] ready");
            return Ok(());
        }
        if !process_alive(record.pid, record.process_start_ticks) {
            let _ = std::fs::remove_file(&path);
            anyhow::bail!(
                "sidecar `{}` exited before becoming healthy at {} (see {})",
                sidecar.name,
                sidecar.health_url,
                if sidecar.log_file.trim().is_empty() {
                    "no log_file configured"
                } else {
                    sidecar.log_file.trim()
                }
            );
        }
        if tokio::time::Instant::now() >= deadline {
            stop_record(&record).await?;
            let _ = std::fs::remove_file(&path);
            anyhow::bail!(
                "sidecar `{}` did not become healthy at {} within {}s",
                sidecar.name,
                sidecar.health_url,
                sidecar.startup_timeout_secs
            );
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: libc::c_int) {
    // SAFETY: plain kill(2) on the sidecar's own process group (it is the group leader).
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

async fn wait_exit(record: &SidecarRecord, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while process_alive(record.pid, record.process_start_ticks) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    true
}

/// SIGTERM, then SIGKILL, re-attesting ownership before each signal.
async fn stop_record(record: &SidecarRecord) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        if !record_attested(record) {
            return Ok(());
        }
        signal_group(record.pid, libc::SIGTERM);
        if !wait_exit(record, GRACEFUL_STOP_TIMEOUT).await {
            if record_attested(record) {
                signal_group(record.pid, libc::SIGKILL);
            }
            if !wait_exit(record, KILL_STOP_TIMEOUT).await {
                anyhow::bail!(
                    "sidecar `{}` (pid {}) did not exit after SIGKILL",
                    record.name,
                    record.pid
                );
            }
        }
        info!(profile = %record.profile_id, sidecar = %record.name, pid = record.pid, "[Sidecar] stopped");
        Ok(())
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("sidecar signaling is unsupported on this platform")
    }
}

/// Ensures every sidecar the profile declares, in order.
pub async fn ensure_profile_sidecars(
    profile_id: &str,
    profile: &IlhaeProfileConfig,
) -> anyhow::Result<()> {
    for sidecar in &profile.sidecars {
        ensure_sidecar(profile_id, sidecar).await?;
    }
    Ok(())
}

/// Stops every sidecar this data dir started for the profile (including ones since
/// removed from the config). Manually started instances are left alone.
pub async fn stop_profile_sidecars(profile_id: &str) -> anyhow::Result<()> {
    let prefix = format!("{}--", clean_component(profile_id));
    let Ok(entries) = std::fs::read_dir(sidecar_dir()) else {
        return Ok(());
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    paths.sort();
    for path in paths {
        let _lock = lock_file(&path.with_extension("lock"))?;
        // The prefix can also match a profile whose id merely starts with this one.
        let Some(record) = read_record(&path).filter(|record| record.profile_id == profile_id)
        else {
            continue;
        };
        stop_record(&record).await?;
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

fn registry_lock() -> anyhow::Result<FileLock> {
    lock_file(&run_dir().join("runtime-clients.lock"))
}

/// Live `(pid, profile_id)` leases; stale ones (dead or reused PIDs) are deleted.
fn live_runtime_clients() -> Vec<(u32, String)> {
    let Ok(entries) = std::fs::read_dir(clients_dir()) else {
        return Vec::new();
    };
    let mut live = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let parsed = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u32>().ok())
            .zip(std::fs::read_to_string(&path).ok());
        let Some((pid, body)) = parsed else {
            continue;
        };
        let mut lines = body.lines();
        let ticks = lines
            .next()
            .and_then(|line| line.trim().parse::<u64>().ok());
        let profile_id = lines.next().unwrap_or_default().trim().to_string();
        match ticks {
            Some(ticks) if process_alive(pid, ticks) => live.push((pid, profile_id)),
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    live
}

/// Registers this process as a user of `profile_id` until [`release_runtime_client`].
pub fn register_runtime_client(profile_id: &str) -> anyhow::Result<()> {
    let pid = std::process::id();
    let ticks = process_state(pid)
        .map(|(_, ticks)| ticks)
        .unwrap_or_default();
    let path = clients_dir().join(pid.to_string());
    let _lock = registry_lock()?;
    std::fs::create_dir_all(clients_dir())?;
    std::fs::write(&path, format!("{ticks}\n{profile_id}\n"))?;
    *CLIENT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ClientLease {
        profile_id: profile_id.to_string(),
        path,
    });
    Ok(())
}

fn take_lease() -> Option<ClientLease> {
    CLIENT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Drops this process's lease without stopping anything (for hard-exit paths).
pub fn forget_runtime_client() {
    if let Some(lease) = take_lease() {
        let _ = std::fs::remove_file(&lease.path);
    }
}

/// Releases this process's lease. If it was the last live client, stops the sidecars
/// and native runtime of every profile involved that sets `stop_when_unused`.
pub async fn release_runtime_client() {
    let Some(lease) = take_lease() else {
        return;
    };
    if let Err(error) = release_lease(&lease).await {
        warn!(profile = %lease.profile_id, "[Sidecar] releasing runtime client failed: {error:#}");
    }
}

async fn release_lease(lease: &ClientLease) -> anyhow::Result<()> {
    let _lock = registry_lock()?;
    let _ = std::fs::remove_file(&lease.path);
    if !live_runtime_clients().is_empty() {
        return Ok(());
    }
    let config = crate::config::load_ilhae_toml_config();
    let mut profile_ids = vec![lease.profile_id.clone()];
    if let Ok(entries) = std::fs::read_dir(sidecar_dir()) {
        for record in entries
            .flatten()
            .filter_map(|entry| read_record(&entry.path()))
        {
            if !profile_ids.contains(&record.profile_id) {
                profile_ids.push(record.profile_id);
            }
        }
    }
    for profile_id in profile_ids {
        let Some(profile) = config.profiles.get(&profile_id) else {
            continue;
        };
        if !profile.stop_when_unused {
            continue;
        }
        info!(profile = %profile_id, "[Sidecar] last Ilhae client exited; stopping profile services");
        stop_profile_sidecars(&profile_id).await?;
        crate::startup_main::stop_profile_runtimes(&config, &profile_id, profile).await?;
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
#[path = "profile_sidecars_tests.rs"]
mod tests;
