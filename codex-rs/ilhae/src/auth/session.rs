use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use chrono::Utc;
use ugot_local_session::SessionLock;
use ugot_local_session::UgotSession;

use super::DEFAULT_ISSUER;
use super::IdentityAuthFile;
use super::IdentityAuthStatus;
use super::userinfo_for_issuer;
use crate::config::resolve_ilhae_config_dir;

const AUTH_FILE_NAME: &str = "identity-auth.json";

pub async fn status() -> anyhow::Result<IdentityAuthStatus> {
    migrate_legacy_auth().await?;
    let shared_path = ugot_local_session::session_path().map_err(anyhow::Error::msg)?;
    if let Some(session) = ugot_local_session::auth::authenticate()
        .await
        .map_err(anyhow::Error::msg)?
    {
        return Ok(shared_status(session, shared_path));
    }
    let mut status = signed_out(shared_path);
    let guard = SessionLock::acquire_async()
        .await
        .map_err(anyhow::Error::msg)?;
    if let Some(session) = guard.load().map_err(anyhow::Error::msg)? {
        status.expires_at = Some(session.expires_at / 1000);
        status.expired = session.is_expired();
        return Ok(status);
    }
    // Serialize the custom issuer check with logout as well. Its credentials
    // remain private to the CLI and never replace an existing UGOT session.
    if let Some(mut auth) = load_legacy_auth()?
        && auth.issuer != DEFAULT_ISSUER
    {
        status.auth_file = auth_file_path();
        status.expires_at = auth.expires_at;
        status.expired = auth
            .expires_at
            .is_none_or(|expiry| expiry <= Utc::now().timestamp());
        if !status.expired
            && let Some(identity) = userinfo_for_issuer(&auth.issuer, &auth.access_token).await?
        {
            auth.claims = Some(super::IdentityClaims {
                subject: Some(identity.sub),
                email: identity.email,
                name: identity.name,
                preferred_username: identity.preferred_username,
                expires_at: auth.expires_at,
            });
            return Ok(private_status(&auth));
        }
    }
    Ok(status)
}

pub fn logout() -> anyhow::Result<bool> {
    let mut guard = SessionLock::acquire().map_err(anyhow::Error::msg)?;
    let shared_path = ugot_local_session::session_path().map_err(anyhow::Error::msg)?;
    let removed = shared_path.exists() || auth_file_path().exists();
    // Advance the shared generation even when no session exists, so an in-flight
    // browser login or an old private CLI file cannot sign the user back in.
    guard.logout_everywhere().map_err(anyhow::Error::msg)?;
    remove_legacy_auth()?;
    Ok(removed)
}

pub(super) async fn finish_login(
    auth: &IdentityAuthFile,
    generation: u64,
) -> anyhow::Result<IdentityAuthStatus> {
    let mut guard = SessionLock::acquire_async()
        .await
        .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        guard.generation().map_err(anyhow::Error::msg)? == generation,
        "identity changed while login was in progress; sign in again"
    );
    if auth.issuer == DEFAULT_ISSUER {
        let session = shared_session(auth)?;
        let path = guard.save(&session).map_err(anyhow::Error::msg)?;
        remove_legacy_auth()?;
        return Ok(shared_status(session, path));
    }
    save_private_auth(auth)?;
    Ok(private_status(auth))
}

async fn migrate_legacy_auth() -> anyhow::Result<()> {
    let mut guard = SessionLock::acquire_async()
        .await
        .map_err(anyhow::Error::msg)?;
    if guard.load().map_err(anyhow::Error::msg)?.is_some()
        || guard.generation().map_err(anyhow::Error::msg)? != 0
    {
        return Ok(());
    }
    let Some(auth) = load_legacy_auth()? else {
        return Ok(());
    };
    if auth.issuer != DEFAULT_ISSUER {
        return Ok(());
    }
    let Ok(candidate) = shared_session(&auth) else {
        return Ok(());
    };
    if let Some(session) = ugot_local_session::auth::validate_session(&candidate)
        .await
        .map_err(anyhow::Error::msg)?
    {
        guard.save(&session).map_err(anyhow::Error::msg)?;
        remove_legacy_auth()?;
    }
    Ok(())
}

fn shared_session(auth: &IdentityAuthFile) -> anyhow::Result<UgotSession> {
    anyhow::ensure!(
        auth.version == 1 && auth.issuer == DEFAULT_ISSUER,
        "invalid UGOT identity file"
    );
    anyhow::ensure!(
        !auth.client_id.trim().is_empty(),
        "identity OAuth client is missing"
    );
    anyhow::ensure!(
        !auth.access_token.is_empty(),
        "identity access token is missing"
    );
    let claims = auth
        .claims
        .as_ref()
        .context("identity claims are missing")?;
    let subject = claims
        .subject
        .as_ref()
        .context("identity subject is missing")?;
    anyhow::ensure!(!subject.trim().is_empty(), "identity subject is missing");
    let expires_at = auth
        .expires_at
        .context("identity expiry is missing")?
        .checked_mul(1000)
        .context("identity expiry overflow")?;
    anyhow::ensure!(
        expires_at > Utc::now().timestamp_millis(),
        "identity token has expired"
    );
    Ok(UgotSession {
        v: 1,
        iss: DEFAULT_ISSUER.into(),
        sub: subject.clone(),
        email: claims.email.clone().unwrap_or_default(),
        name: claims.name.clone().unwrap_or_default(),
        username: claims.preferred_username.clone().unwrap_or_default(),
        access_token: auth.access_token.clone(),
        refresh_token: auth.refresh_token.clone().unwrap_or_default(),
        client_id: auth.client_id.clone(),
        expires_at,
    })
}

fn shared_status(session: UgotSession, path: PathBuf) -> IdentityAuthStatus {
    let expired = session.is_expired();
    IdentityAuthStatus {
        authenticated: !expired,
        auth_file: path,
        issuer: Some(session.iss),
        client_id: Some(session.client_id),
        subject: Some(session.sub),
        email: (!session.email.is_empty()).then_some(session.email),
        name: (!session.name.is_empty()).then_some(session.name),
        preferred_username: (!session.username.is_empty()).then_some(session.username),
        expires_at: Some(session.expires_at / 1000),
        expired,
    }
}

fn private_status(auth: &IdentityAuthFile) -> IdentityAuthStatus {
    let expired = auth
        .expires_at
        .is_none_or(|expiry| expiry <= Utc::now().timestamp());
    IdentityAuthStatus {
        authenticated: !expired,
        auth_file: auth_file_path(),
        issuer: Some(auth.issuer.clone()),
        client_id: Some(auth.client_id.clone()),
        subject: auth
            .claims
            .as_ref()
            .and_then(|claims| claims.subject.clone()),
        email: auth.claims.as_ref().and_then(|claims| claims.email.clone()),
        name: auth.claims.as_ref().and_then(|claims| claims.name.clone()),
        preferred_username: auth
            .claims
            .as_ref()
            .and_then(|claims| claims.preferred_username.clone()),
        expires_at: auth.expires_at,
        expired,
    }
}

fn signed_out(path: PathBuf) -> IdentityAuthStatus {
    IdentityAuthStatus {
        authenticated: false,
        auth_file: path,
        issuer: None,
        client_id: None,
        subject: None,
        email: None,
        name: None,
        preferred_username: None,
        expires_at: None,
        expired: false,
    }
}

fn auth_file_path() -> PathBuf {
    resolve_ilhae_config_dir().join(AUTH_FILE_NAME)
}

fn load_legacy_auth() -> anyhow::Result<Option<IdentityAuthFile>> {
    match fs::read(auth_file_path()) {
        Ok(content) => Ok(Some(serde_json::from_slice(&content)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn remove_legacy_auth() -> anyhow::Result<()> {
    match fs::remove_file(auth_file_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn save_private_auth(auth: &IdentityAuthFile) -> anyhow::Result<()> {
    let path = auth_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", super::generate_state()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(auth)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
