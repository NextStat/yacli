use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::credential_store::StoredOauthCredential;
use crate::error::{Result, YacliError};

pub const DEFAULT_OAUTH_BASE_URL: &str = "https://oauth.yandex.ru";
pub const DEFAULT_REDIRECT_URI: &str = "https://oauth.yandex.ru/verification_code";
pub const DEFAULT_YACLI_CLIENT_ID: &str = "babbe3ab2e254d5abee427890e2a5a8f";
pub const DISK_SCOPES: &[&str] = &[
    "cloud_api:disk.app_folder",
    "cloud_api:disk.info",
    "cloud_api:disk.read",
    "cloud_api:disk.write",
];
pub const MAIL_SCOPES: &[&str] = &["mail:imap_full", "mail:smtp"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OauthService {
    Mail,
    Disk,
}

impl OauthService {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mail => "mail",
            Self::Disk => "disk",
        }
    }

    pub fn store_ref(self) -> &'static str {
        match self {
            Self::Mail => "store:mail",
            Self::Disk => "store:disk",
        }
    }

    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Mail => MAIL_SCOPES,
            Self::Disk => DISK_SCOPES,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthorizationRequest {
    pub authorization_url: String,
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge_method: &'static str,
}

#[derive(Clone, Debug)]
pub struct AuthorizationSession {
    pub request: AuthorizationRequest,
    pub(crate) client_id: String,
    pub(crate) code_verifier: String,
}

#[derive(Clone, Debug)]
pub struct LoginResult {
    pub authorization: AuthorizationRequest,
    pub credential: StoredOauthCredential,
}

pub fn oauth_base_url() -> String {
    std::env::var("YACLI_OAUTH_BASE_URL").unwrap_or_else(|_| DEFAULT_OAUTH_BASE_URL.to_string())
}

pub const fn default_yacli_client_id() -> &'static str {
    DEFAULT_YACLI_CLIENT_ID
}

pub fn start_pkce_authorization(
    services: &[OauthService],
    client_id: &str,
    login_hint: Option<&str>,
) -> Result<AuthorizationSession> {
    if services.is_empty() {
        return Err(YacliError::Config(
            "OAuth authorization requires at least one target service".to_string(),
        ));
    }
    let code_verifier = random_token(32);
    let code_challenge = pkce_challenge(&code_verifier);
    let state = random_token(16);
    let scopes = deduped_scopes(services);
    let request = AuthorizationRequest {
        authorization_url: build_authorize_url(
            &oauth_base_url(),
            &scopes,
            client_id,
            &state,
            &code_challenge,
            login_hint,
        )?,
        redirect_uri: DEFAULT_REDIRECT_URI.to_string(),
        state,
        code_challenge_method: "S256",
    };

    Ok(AuthorizationSession {
        request,
        client_id: client_id.to_string(),
        code_verifier,
    })
}

pub fn exchange_authorization_code(
    session: AuthorizationSession,
    code: &str,
) -> Result<LoginResult> {
    let client = build_http_client()?;
    let response = client
        .post(oauth_endpoint(&oauth_base_url(), "/token")?)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", session.client_id.as_str()),
            ("code_verifier", session.code_verifier.as_str()),
        ])
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(oauth_error(status, &body, "authorization code exchange"));
    }

    let token = serde_json::from_str::<RawTokenResponse>(&body)
        .map_err(|err| YacliError::Serialization(format!("invalid OAuth token response: {err}")))?;

    Ok(LoginResult {
        authorization: session.request,
        credential: token.into_stored(&session.client_id),
    })
}

pub fn unix_timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn build_authorize_url(
    base_url: &str,
    scopes: &[&str],
    client_id: &str,
    state: &str,
    code_challenge: &str,
    login_hint: Option<&str>,
) -> Result<String> {
    let mut url = oauth_endpoint(base_url, "/authorize")?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("redirect_uri", DEFAULT_REDIRECT_URI);
        query.append_pair("scope", &scopes.join(" "));
        query.append_pair("state", state);
        query.append_pair("code_challenge", code_challenge);
        query.append_pair("code_challenge_method", "S256");
        if let Some(login_hint) = login_hint {
            query.append_pair("login_hint", login_hint);
        }
    }

    Ok(url.to_string())
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("yacli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(Into::into)
}

fn oauth_endpoint(base_url: &str, path: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid OAuth base URL: {err}")))?
        .join(path)
        .map_err(|err| YacliError::Config(format!("invalid OAuth endpoint: {err}")))
}

fn deduped_scopes(services: &[OauthService]) -> Vec<&'static str> {
    let mut scopes = Vec::new();
    for service in services {
        for scope in service.scopes() {
            if !scopes.contains(scope) {
                scopes.push(*scope);
            }
        }
    }
    scopes
}

fn pkce_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn random_token(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn oauth_error(status: StatusCode, body: &str, action: &str) -> YacliError {
    let parsed = serde_json::from_str::<OauthErrorBody>(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|value| value.error.as_deref())
        .unwrap_or("unknown_oauth_error");
    let description = parsed
        .as_ref()
        .and_then(|value| value.error_description.as_deref())
        .unwrap_or("No provider message returned.");

    YacliError::Auth(format!(
        "Yandex OAuth {} failed with status {} ({}): {}",
        action,
        status.as_u16(),
        code,
        description
    ))
}

#[derive(Debug, Deserialize)]
struct RawTokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    #[serde(default)]
    scope: String,
}

impl RawTokenResponse {
    fn into_stored(self, client_id: &str) -> StoredOauthCredential {
        StoredOauthCredential {
            kind: "oauth_pkce".to_string(),
            access_token: self.access_token,
            token_type: self.token_type,
            expires_at_epoch_secs: unix_timestamp_now().saturating_add(self.expires_in),
            scope: self
                .scope
                .split_whitespace()
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect(),
            client_id: client_id.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct OauthErrorBody {
    error: Option<String>,
    error_description: Option<String>,
}
