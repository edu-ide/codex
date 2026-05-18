use std::io::Cursor;
use std::path::PathBuf;

use anyhow::Context;
use base64::Engine;
use chrono::Utc;
use rand::RngCore;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tiny_http::Header;
use tiny_http::Response;
use tiny_http::Server;
use url::Url;

use crate::config::resolve_ilhae_config_dir;

pub const DEFAULT_ISSUER: &str = "https://auth.ugot.uk";
pub const DEFAULT_CLIENT_ID: &str = "ilhae-cli";

const AUTH_FILE_NAME: &str = "identity-auth.json";
const DEFAULT_CALLBACK_PORT: u16 = 14580;
const FALLBACK_CALLBACK_PORT: u16 = 14581;
const CALLBACK_PATH: &str = "/auth/callback";
const DEFAULT_SCOPE: &str = "openid profile email offline_access";

#[derive(Debug, Clone)]
pub struct IdentityLoginOptions {
    pub issuer: String,
    pub client_id: String,
    pub open_browser: bool,
}

impl Default for IdentityLoginOptions {
    fn default() -> Self {
        Self {
            issuer: DEFAULT_ISSUER.to_string(),
            client_id: DEFAULT_CLIENT_ID.to_string(),
            open_browser: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityAuthFile {
    pub version: u32,
    pub issuer: String,
    pub client_id: String,
    pub token_type: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub scope: Option<String>,
    pub expires_at: Option<i64>,
    pub claims: Option<IdentityClaims>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IdentityClaims {
    pub subject: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    pub preferred_username: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IdentityAuthStatus {
    pub authenticated: bool,
    pub auth_file: PathBuf,
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    pub preferred_username: Option<String>,
    pub expires_at: Option<i64>,
    pub expired: bool,
}

#[derive(Debug, Clone)]
struct PkceCodes {
    code_verifier: String,
    code_challenge: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

pub async fn login(options: IdentityLoginOptions) -> anyhow::Result<IdentityAuthStatus> {
    let issuer = normalize_issuer(&options.issuer)?;
    let client_id = non_empty_or_default(&options.client_id, DEFAULT_CLIENT_ID);
    let pkce = generate_pkce();
    let state = generate_state();
    let server = bind_callback_server()?;
    let actual_port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .context("unable to determine identity login callback port")?;
    let redirect_uri = format!("http://127.0.0.1:{actual_port}{CALLBACK_PATH}");
    let auth_url = build_authorize_url(&issuer, &client_id, &redirect_uri, &pkce, &state)?;

    if options.open_browser {
        if let Err(err) = webbrowser::open(auth_url.as_str()) {
            eprintln!("Failed to open browser: {err}");
            eprintln!("Open this URL to sign in:\n{auth_url}");
        }
    } else {
        eprintln!("Open this URL to sign in:\n{auth_url}");
    }

    let callback = wait_for_callback(server, actual_port, state).await?;
    let token = exchange_code(&issuer, &client_id, &redirect_uri, &pkce, &callback.code).await?;
    let auth = build_auth_file(issuer, client_id, token);
    save_auth(&auth)?;
    status()
}

pub fn status() -> anyhow::Result<IdentityAuthStatus> {
    let auth_file = auth_file_path();
    let Some(auth) = load_auth()? else {
        return Ok(IdentityAuthStatus {
            authenticated: false,
            auth_file,
            issuer: None,
            client_id: None,
            subject: None,
            email: None,
            name: None,
            preferred_username: None,
            expires_at: None,
            expired: false,
        });
    };
    let expires_at = auth
        .claims
        .as_ref()
        .and_then(|claims| claims.expires_at)
        .or(auth.expires_at);
    Ok(IdentityAuthStatus {
        authenticated: true,
        auth_file,
        issuer: Some(auth.issuer),
        client_id: Some(auth.client_id),
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
        expires_at,
        expired: expires_at.is_some_and(|value| value <= Utc::now().timestamp()),
    })
}

pub fn logout() -> anyhow::Result<bool> {
    let path = auth_file_path();
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(path)?;
    Ok(true)
}

pub fn load_auth() -> anyhow::Result<Option<IdentityAuthFile>> {
    let path = auth_file_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

fn save_auth(auth: &IdentityAuthFile) -> anyhow::Result<()> {
    let path = auth_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(auth)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn auth_file_path() -> PathBuf {
    resolve_ilhae_config_dir().join(AUTH_FILE_NAME)
}

fn normalize_issuer(raw: &str) -> anyhow::Result<String> {
    let issuer = non_empty_or_default(raw, DEFAULT_ISSUER);
    let url =
        Url::parse(&issuer).with_context(|| format!("invalid identity issuer URL: {issuer}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("identity issuer must use http or https");
    }
    Ok(issuer.trim_end_matches('/').to_string())
}

fn non_empty_or_default(value: &str, default_value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default_value.to_string()
    } else {
        trimmed.to_string()
    }
}

fn generate_pkce() -> PkceCodes {
    let mut bytes = [0u8; 64];
    rand::rng().fill_bytes(&mut bytes);
    let code_verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn bind_callback_server() -> anyhow::Result<Server> {
    for port in [DEFAULT_CALLBACK_PORT, FALLBACK_CALLBACK_PORT] {
        match Server::http(("127.0.0.1", port)) {
            Ok(server) => return Ok(server),
            Err(err) => {
                if port == FALLBACK_CALLBACK_PORT {
                    return Err(anyhow::Error::msg(err.to_string())).with_context(|| {
                        format!("failed to bind identity login callback on port {port}")
                    });
                }
            }
        }
    }
    anyhow::bail!("failed to bind identity login callback server")
}

fn build_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> anyhow::Result<Url> {
    let mut url = Url::parse(&format!("{issuer}/oauth2/authorize"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", DEFAULT_SCOPE)
        .append_pair("state", state)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url)
}

#[derive(Debug)]
struct CallbackCode {
    code: String,
}

async fn wait_for_callback(
    server: Server,
    actual_port: u16,
    expected_state: String,
) -> anyhow::Result<CallbackCode> {
    tokio::task::spawn_blocking(move || {
        wait_for_callback_blocking(server, actual_port, &expected_state)
    })
    .await
    .context("identity login callback task failed")?
}

fn wait_for_callback_blocking(
    server: Server,
    actual_port: u16,
    expected_state: &str,
) -> anyhow::Result<CallbackCode> {
    loop {
        let request = server.recv()?;
        let raw_url = request.url().to_string();
        let parsed = Url::parse(&format!("http://127.0.0.1:{actual_port}{raw_url}"))?;
        if parsed.path() != CALLBACK_PATH {
            let response = text_response(404, "Not found");
            let _ = request.respond(response);
            continue;
        }

        let mut code = None;
        let mut state = None;
        let mut error = None;
        let mut error_description = None;
        for (key, value) in parsed.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                "error_description" => error_description = Some(value.into_owned()),
                _ => {}
            }
        }

        if let Some(error) = error {
            let detail = error_description.unwrap_or_else(|| error.clone());
            let _ = request.respond(text_response(
                400,
                "Ilhae login failed. You can close this tab.",
            ));
            anyhow::bail!("identity login failed: {detail}");
        }

        if state.as_deref() != Some(expected_state) {
            let _ = request.respond(text_response(
                400,
                "Invalid Ilhae login state. You can close this tab.",
            ));
            anyhow::bail!("identity login returned an invalid state");
        }

        let Some(code) = code else {
            let _ = request.respond(text_response(
                400,
                "Missing Ilhae login code. You can close this tab.",
            ));
            anyhow::bail!("identity login callback did not include an authorization code");
        };

        let _ = request.respond(text_response(
            200,
            "Ilhae login complete. You can close this tab.",
        ));
        return Ok(CallbackCode { code });
    }
}

fn text_response(status: u16, body: &str) -> Response<Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body.to_string()).with_status_code(status);
    if let Ok(header) = Header::from_bytes("Content-Type", "text/plain; charset=utf-8") {
        response = response.with_header(header);
    }
    response
}

async fn exchange_code(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> anyhow::Result<TokenResponse> {
    let token_url = format!("{issuer}/oauth2/token");
    let response = reqwest::Client::new()
        .post(token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", &pkce.code_verifier),
        ])
        .send()
        .await
        .context("failed to exchange identity authorization code")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read identity token response")?;
    if !status.is_success() {
        anyhow::bail!("identity token exchange failed with HTTP {status}: {body}");
    }
    serde_json::from_str(&body).context("failed to parse identity token response")
}

fn build_auth_file(issuer: String, client_id: String, token: TokenResponse) -> IdentityAuthFile {
    let now = Utc::now().timestamp();
    let expires_at = token.expires_in.map(|seconds| now + seconds);
    let claims = token
        .id_token
        .as_deref()
        .and_then(parse_identity_claims_from_jwt)
        .or_else(|| parse_identity_claims_from_jwt(&token.access_token));
    IdentityAuthFile {
        version: 1,
        issuer,
        client_id,
        token_type: token.token_type.unwrap_or_else(|| "Bearer".to_string()),
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        id_token: token.id_token,
        scope: token.scope,
        expires_at,
        claims,
        created_at: now,
        updated_at: now,
    }
}

fn parse_identity_claims_from_jwt(jwt: &str) -> Option<IdentityClaims> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&decoded).ok()?;
    Some(IdentityClaims {
        subject: json_string(&value, "sub"),
        email: json_string(&value, "email"),
        name: json_string(&value, "name"),
        preferred_username: json_string(&value, "preferred_username"),
        expires_at: value.get("exp").and_then(serde_json::Value::as_i64),
    })
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_authorize_url_targets_identity_authorization_endpoint() {
        let pkce = PkceCodes {
            code_verifier: "verifier".to_string(),
            code_challenge: "challenge".to_string(),
        };

        let url = build_authorize_url(
            DEFAULT_ISSUER,
            DEFAULT_CLIENT_ID,
            "http://127.0.0.1:14580/auth/callback",
            &pkce,
            "state",
        )
        .expect("authorize url");

        assert_eq!(
            Some("https://auth.ugot.uk/oauth2/authorize"),
            url.as_str().split('?').next()
        );
        let params = url.query_pairs().collect::<Vec<_>>();
        assert!(params.contains(&("client_id".into(), "ilhae-cli".into())));
        assert!(params.contains(&("code_challenge_method".into(), "S256".into())));
        assert!(params.contains(&("scope".into(), DEFAULT_SCOPE.into())));
    }

    #[test]
    fn parse_identity_claims_from_jwt_payload() {
        let claims = serde_json::json!({
            "sub": "user-1",
            "email": "user@example.com",
            "name": "Test User",
            "preferred_username": "test",
            "exp": 123,
        });
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).expect("claims json"));
        let jwt = format!("header.{payload}.signature");

        let parsed = parse_identity_claims_from_jwt(&jwt).expect("claims");

        assert_eq!(
            IdentityClaims {
                subject: Some("user-1".to_string()),
                email: Some("user@example.com".to_string()),
                name: Some("Test User".to_string()),
                preferred_username: Some("test".to_string()),
                expires_at: Some(123),
            },
            parsed
        );
    }
}
