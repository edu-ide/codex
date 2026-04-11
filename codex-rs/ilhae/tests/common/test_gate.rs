#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

pub const ILHAE_RUN_TEAM_LIVE_A2A: &str = "ILHAE_RUN_TEAM_LIVE_A2A";
pub const ILHAE_RUN_TEAM_HEADLESS_E2E: &str = "ILHAE_RUN_TEAM_HEADLESS_E2E";
pub const ILHAE_RUN_TEAM_LOCAL_A2A_SPAWN: &str = "ILHAE_RUN_TEAM_LOCAL_A2A_SPAWN";
pub const ILHAE_RUN_TEAM_PROXY_E2E: &str = "ILHAE_RUN_TEAM_PROXY_E2E";

pub fn env_flag_enabled(name: &str) -> bool {
    let Ok(value) = std::env::var(name) else {
        return false;
    };

    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn skip_notice_registry() -> &'static Mutex<HashSet<String>> {
    static REGISTRY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn should_emit_skip_notice(suite_name: &str) -> bool {
    skip_notice_registry()
        .lock()
        .expect("skip notice registry poisoned")
        .insert(suite_name.to_string())
}

fn require_opt_in(env_var: &str, suite_name: &str) -> bool {
    if env_flag_enabled(env_var) {
        return true;
    }

    if should_emit_skip_notice(suite_name) {
        eprintln!("skipping {suite_name}; set {env_var}=1 to run");
    }
    false
}

pub fn require_team_live_a2a() -> bool {
    require_opt_in(ILHAE_RUN_TEAM_LIVE_A2A, "team live A2A suite")
}

pub fn require_team_headless_e2e() -> bool {
    require_opt_in(ILHAE_RUN_TEAM_HEADLESS_E2E, "team headless E2E suite")
}

pub fn require_team_local_a2a_spawn() -> bool {
    require_opt_in(ILHAE_RUN_TEAM_LOCAL_A2A_SPAWN, "team local A2A spawn suite")
}

pub fn require_team_proxy_e2e() -> bool {
    require_opt_in(ILHAE_RUN_TEAM_PROXY_E2E, "team proxy E2E suite")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn env_flag_enabled_accepts_truthy_values() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::set_var("ILHAE_TEST_TEAM_GATE", "true") };
        assert!(env_flag_enabled("ILHAE_TEST_TEAM_GATE"));
        unsafe { std::env::remove_var("ILHAE_TEST_TEAM_GATE") };
    }

    #[test]
    fn env_flag_enabled_rejects_absent_or_falsey_values() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::remove_var("ILHAE_TEST_TEAM_GATE") };
        assert!(!env_flag_enabled("ILHAE_TEST_TEAM_GATE"));

        unsafe { std::env::set_var("ILHAE_TEST_TEAM_GATE", "0") };
        assert!(!env_flag_enabled("ILHAE_TEST_TEAM_GATE"));
        unsafe { std::env::remove_var("ILHAE_TEST_TEAM_GATE") };
    }

    #[test]
    fn require_team_live_a2a_respects_opt_in_env() {
        let _guard = env_lock().lock().unwrap();
        unsafe { std::env::remove_var(ILHAE_RUN_TEAM_LIVE_A2A) };
        assert!(!require_team_live_a2a());

        unsafe { std::env::set_var(ILHAE_RUN_TEAM_LIVE_A2A, "1") };
        assert!(require_team_live_a2a());
        unsafe { std::env::remove_var(ILHAE_RUN_TEAM_LIVE_A2A) };
    }

    #[test]
    fn skip_notice_emits_once_per_suite_name() {
        assert!(should_emit_skip_notice("test-suite-once"));
        assert!(!should_emit_skip_notice("test-suite-once"));
        assert!(should_emit_skip_notice("test-suite-other"));
    }
}
