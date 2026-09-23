use super::*;
use crate::auth::DEFAULT_CLIENT_ID;
use crate::auth::IdentityClaims;

struct TestPaths {
    _directory: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl TestPaths {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary auth directory");
        let mut previous = Vec::new();
        for (key, relative) in [
            ("UGOT_SESSION_PATH", "ugot/session"),
            ("ILHAE_CONFIG_DIR", "ilhae"),
            ("UGOT_BRIDGE_STATE_DIR", "bridge"),
            ("UGOT_OFFICE_ACCOUNT_PATH", "office/account.json"),
        ] {
            previous.push((key, std::env::var_os(key)));
            // All environment-dependent auth tests use serial_test.
            unsafe { std::env::set_var(key, directory.path().join(relative)) };
        }
        Self {
            _directory: directory,
            previous,
        }
    }
}

impl Drop for TestPaths {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

fn verified_auth() -> IdentityAuthFile {
    IdentityAuthFile {
        version: 1,
        issuer: DEFAULT_ISSUER.into(),
        client_id: DEFAULT_CLIENT_ID.into(),
        token_type: "Bearer".into(),
        access_token: "test-access-token".into(),
        refresh_token: Some("test-refresh-token".into()),
        id_token: None,
        scope: None,
        expires_at: Some(Utc::now().timestamp() + 600),
        claims: Some(IdentityClaims {
            subject: Some("verified-user".into()),
            email: Some("user@ugot.uk".into()),
            name: Some("User".into()),
            preferred_username: Some("user".into()),
            expires_at: None,
        }),
        created_at: Utc::now().timestamp(),
        updated_at: Utc::now().timestamp(),
    }
}

#[test]
fn shared_session_preserves_issuing_client_and_converts_expiry_to_milliseconds() {
    let auth = verified_auth();
    let session = shared_session(&auth).expect("shared session");
    assert_eq!(
        session,
        UgotSession {
            v: 1,
            iss: DEFAULT_ISSUER.into(),
            sub: "verified-user".into(),
            email: "user@ugot.uk".into(),
            name: "User".into(),
            username: "user".into(),
            access_token: "test-access-token".into(),
            refresh_token: "test-refresh-token".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
            expires_at: auth.expires_at.expect("expiry") * 1000,
        }
    );
}

#[test]
fn expired_session_is_never_authenticated() {
    let mut session = shared_session(&verified_auth()).expect("session");
    session.expires_at = 1;
    let result = shared_status(session, PathBuf::from("session"));
    assert!(!result.authenticated);
    assert!(result.expired);
}

#[tokio::test]
#[serial_test::serial]
async fn verified_login_persists_shared_session_and_removes_legacy_credentials() {
    let _paths = TestPaths::new();
    let auth = verified_auth();
    save_private_auth(&auth).expect("legacy credentials");
    let result = finish_login(&auth, /*generation*/ 0).await.expect("login");
    assert_eq!(
        ugot_local_session::load().expect("load"),
        Some(shared_session(&auth).expect("session"))
    );
    assert_eq!(
        result.auth_file,
        ugot_local_session::session_path().expect("shared path")
    );
    assert!(!auth_file_path().exists());
}

#[tokio::test]
#[serial_test::serial]
async fn logout_during_browser_login_prevents_session_resurrection() {
    let _paths = TestPaths::new();
    let generation = SessionLock::acquire()
        .expect("lock")
        .generation()
        .expect("generation");
    logout().expect("logout");
    let result = finish_login(&verified_auth(), generation).await;
    assert!(result.is_err());
    assert_eq!(ugot_local_session::load().expect("load"), None);
}

#[tokio::test]
#[serial_test::serial]
async fn logout_blocks_migration_of_old_cli_credentials() {
    let _paths = TestPaths::new();
    logout().expect("logout");
    save_private_auth(&verified_auth()).expect("old CLI wrote stale credentials");
    migrate_legacy_auth().await.expect("skip old credentials");
    assert_eq!(ugot_local_session::load().expect("load"), None);
    let result = status().await.expect("status");
    assert!(!result.authenticated);
    assert_eq!(
        result.auth_file,
        ugot_local_session::session_path().expect("shared path")
    );
}

#[tokio::test]
#[serial_test::serial]
async fn custom_issuer_login_keeps_shared_identity_unchanged() {
    let _paths = TestPaths::new();
    let shared = shared_session(&verified_auth()).expect("shared session");
    ugot_local_session::save(&shared).expect("existing shared identity");
    let generation = SessionLock::acquire()
        .expect("lock")
        .generation()
        .expect("generation");
    let mut custom = verified_auth();
    custom.issuer = "https://identity.example.com".into();
    let result = finish_login(&custom, generation)
        .await
        .expect("custom login");
    assert_eq!(ugot_local_session::load().expect("load"), Some(shared));
    assert_eq!(result.auth_file, auth_file_path());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(auth_file_path())
                .expect("private credentials")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    logout().expect("logout all credentials");
    assert_eq!(ugot_local_session::load().expect("load"), None);
    assert!(!auth_file_path().exists());
}
