use super::*;
use pretty_assertions::assert_eq;

#[test]
fn requested_provider_is_explicit_and_unknown_engines_are_rejected() {
    let mut settings = BrowserSettings::default();
    for (browser_type, expected) in [
        ("auto", None),
        ("cef", None),
        ("chrome", Some("chrome")),
        ("Firefox", Some("firefox")),
        ("camoufox", Some("camoufox")),
        ("webkit", Some("webkit")),
    ] {
        settings.browser_type = browser_type.to_string();
        assert_eq!(launch_engine(&settings), Ok(expected));
    }
    settings.browser_type = "unsupported".to_string();
    assert!(launch_engine(&settings).is_err());
}

#[test]
fn custom_connection_and_temporary_profile_settings_use_shared_lifecycle() {
    let remote = BrowserSettings {
        server_url: "ws://127.0.0.1:9888/devtools/browser/session".to_string(),
        ..BrowserSettings::default()
    };
    let request = settings_launch_request(&remote).unwrap().unwrap();
    assert_eq!(request.name, "browser_launch_config");
    assert_eq!(
        request.arguments,
        json!({ "action":"attach", "cdpUrl":remote.server_url })
            .as_object()
            .cloned()
    );

    let custom_port = BrowserSettings {
        cdp_port: 9888,
        ..BrowserSettings::default()
    };
    let request = settings_launch_request(&custom_port).unwrap().unwrap();
    assert_eq!(
        request.arguments,
        json!({ "action":"attach", "cdpUrl":"http://127.0.0.1:9888" })
            .as_object()
            .cloned()
    );

    let temporary = BrowserSettings {
        browser_type: "chrome".to_string(),
        persistent: false,
        headless: false,
        cdp_port: 19888,
        ..BrowserSettings::default()
    };
    let request = settings_launch_request(&temporary).unwrap().unwrap();
    assert_eq!(request.arguments, json!({ "action":"configure", "engine":"chrome", "persistent":false, "headless":false, "cdpPort":19888, "relaunch":true }).as_object().cloned());
    assert!(
        settings_launch_request(&BrowserSettings::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn stopped_browser_remains_connected_in_ui_projection() {
    let observed = status_projection(&json!({
        "selectedBackend":"cef", "browserConnected":true, "stopped":true
    }));
    assert_eq!(
        serde_json::to_value(observed).unwrap(),
        json!({
            "running":true,
            "browser_type":"cef",
            "session_connected":true,
            "message":"Shared browser actions stopped; browser remains open"
        })
    );
}

#[test]
fn invalid_explicit_binary_never_falls_back_to_path() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join(if cfg!(windows) {
        "ugot-browser.exe"
    } else {
        "ugot-browser"
    });
    std::fs::write(&candidate, b"\x7fELFtest fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = std::env::join_paths([directory.path()]).unwrap();
    assert_eq!(
        service_executable(None, Some(&path)),
        Ok(candidate.canonicalize().unwrap())
    );
    assert!(service_executable(Some(OsStr::new("missing-relative-binary")), Some(&path)).is_err());
}

#[test]
fn node_launchers_are_not_used_as_the_native_service_identity() {
    let directory = tempfile::tempdir().unwrap();
    let launcher = directory.path().join("ugot-browser");
    std::fs::write(&launcher, b"#!/usr/bin/env node\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(!native_executable(&launcher));
    assert!(service_executable(Some(launcher.as_os_str()), None).is_err());
}
