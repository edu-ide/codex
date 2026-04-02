use std::path::Path;
use tracing::{info, warn};

pub const PID_FILE_NAME: &str = "proxy.pid";
pub const CHILDREN_PID_FILE_NAME: &str = "children.pids";
pub const DAEMON_PID_FILE_NAME: &str = "proxy-daemon.pid";
pub const DAEMON_CHILDREN_PID_FILE_NAME: &str = "children-daemon.pids";

fn pid_file_name(daemon_mode: bool) -> &'static str {
    if daemon_mode {
        DAEMON_PID_FILE_NAME
    } else {
        PID_FILE_NAME
    }
}

fn children_file_name(daemon_mode: bool) -> &'static str {
    if daemon_mode {
        DAEMON_CHILDREN_PID_FILE_NAME
    } else {
        CHILDREN_PID_FILE_NAME
    }
}

/// Write the current proxy's PID to ~/ilhae/proxy.pid
pub fn write_proxy_pid(ilhae_dir: &Path) {
    write_proxy_pid_for_mode(
        ilhae_dir,
        std::env::var("ILHAE_PROXY_DAEMON").ok().as_deref() == Some("1"),
    );
}

pub fn write_proxy_pid_for_mode(ilhae_dir: &Path, daemon_mode: bool) {
    let pid = std::process::id();
    let pid_file = ilhae_dir.join(pid_file_name(daemon_mode));
    if let Err(e) = std::fs::write(&pid_file, pid.to_string()) {
        warn!("Failed to write proxy PID file {:?}: {}", pid_file, e);
    } else {
        info!("[PID] Wrote proxy PID {} to {:?}", pid, pid_file);
    }
    // Clear children file for this new session
    let children_file = ilhae_dir.join(children_file_name(daemon_mode));
    let _ = std::fs::write(&children_file, "");
}

/// Append a child PID to ~/ilhae/children.pids (one PID per line)
pub fn append_child_pid(ilhae_dir: &Path, pid: u32) {
    let daemon_mode = std::env::var("ILHAE_PROXY_DAEMON")
        .ok()
        .as_deref()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    let children_file = ilhae_dir.join(children_file_name(daemon_mode));
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&children_file)
    {
        let _ = writeln!(f, "{}", pid);
    }
}

/// Kill the previous proxy session using PID files.
/// Returns (proxy_killed, children_killed) counts.
pub fn kill_previous_session(ilhae_dir: &Path) -> (u32, u32) {
    kill_previous_session_for_mode(
        ilhae_dir,
        std::env::var("ILHAE_PROXY_DAEMON").ok().as_deref() == Some("1"),
    )
}

pub fn kill_previous_session_for_mode(ilhae_dir: &Path, daemon_mode: bool) -> (u32, u32) {
    let mut proxy_killed = 0u32;
    let children_killed = 0u32;

    let pid_file = ilhae_dir.join(pid_file_name(daemon_mode));
    if let Ok(content) = std::fs::read_to_string(&pid_file) {
        if let Ok(old_pid) = content.trim().parse::<u32>() {
            if old_pid != std::process::id() {
                let alive = std::process::Command::new("kill")
                    .args(["-0", &old_pid.to_string()])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                if alive {
                    info!("[PID] Killing previous proxy (PID {})", old_pid);
                    let _ = std::process::Command::new("kill")
                        .args(["-TERM", &old_pid.to_string()])
                        .output();
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let _ = std::process::Command::new("kill")
                        .args(["-9", &old_pid.to_string()])
                        .output();
                    proxy_killed = 1;
                }
            }
        }
    }

    let children_file = ilhae_dir.join(children_file_name(daemon_mode));
    let _ = std::fs::remove_file(&pid_file);
    let _ = std::fs::remove_file(&children_file);

    if proxy_killed > 0 {
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    (proxy_killed, children_killed)
}

/// Remove PID files on clean shutdown.
pub fn cleanup_pid_files(ilhae_dir: &Path) {
    cleanup_pid_files_for_mode(
        ilhae_dir,
        std::env::var("ILHAE_PROXY_DAEMON").ok().as_deref() == Some("1"),
    );
}

pub fn cleanup_pid_files_for_mode(ilhae_dir: &Path, daemon_mode: bool) {
    let _ = std::fs::remove_file(ilhae_dir.join(pid_file_name(daemon_mode)));
    let _ = std::fs::remove_file(ilhae_dir.join(children_file_name(daemon_mode)));
}

/// Aggressively find and kill any other ilhae-proxy processes running on the system
/// using the sysinfo crate to scan the process table. This acts as a foolproof
/// fallback for zombie processes that didn't leave a PID file or bind a port.
pub fn enforce_singleton_proxy(skip_daemon_processes: bool) -> u32 {
    let mut killed = 0;
    let current_pid = sysinfo::get_current_pid().unwrap_or(sysinfo::Pid::from_u32(0));
    let current_pid_u32 = std::process::id();

    let mut sys = sysinfo::System::new_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let current_tasks = sys
        .process(current_pid)
        .and_then(|process| process.tasks().cloned())
        .unwrap_or_default();

    let mut killed_pids = Vec::new();

    for (pid, process) in sys.processes() {
        if *pid == current_pid
            || current_tasks.contains(pid)
            || process.thread_kind().is_some()
            || linux_thread_group_id(*pid) == Some(current_pid_u32)
        {
            continue;
        }

        let p_name = process.name().to_string_lossy().to_lowercase();
        let is_daemon = process
            .cmd()
            .iter()
            .any(|arg| arg.to_string_lossy().contains("--daemon"));
        if skip_daemon_processes && is_daemon {
            continue;
        }
        // Match exact or with .exe for windows
        if p_name == "ilhae-proxy"
            || p_name == "ilhae-proxy.exe"
            || process
                .exe()
                .map(|p| p.to_string_lossy().contains("ilhae-proxy"))
                .unwrap_or(false)
        {
            tracing::warn!(
                "[ZombieSweep] Found zombie ilhae-proxy (PID: {}). Killing it.",
                pid
            );
            process.kill();
            killed_pids.push(*pid);
            killed += 1;
        }
    }

    if !killed_pids.is_empty() {
        tracing::info!(
            "[ZombieSweep] Waiting for OS to release ports for {} killed processes...",
            killed_pids.len()
        );
        let start_wait = std::time::Instant::now();
        loop {
            // Keep refreshing to see if PIDs are gone
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            let any_alive = killed_pids.iter().any(|p| sys.process(*p).is_some());

            if !any_alive || start_wait.elapsed().as_millis() > 2000 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // Just an extra split-second buffer for the kernel network stack to fully flush TIME_WAIT
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    killed
}

fn linux_thread_group_id(pid: sysinfo::Pid) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/{}/status", pid.as_u32());
        let status = std::fs::read_to_string(path).ok()?;
        status.lines().find_map(|line| {
            line.strip_prefix("Tgid:")
                .and_then(|value| value.trim().parse::<u32>().ok())
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}
