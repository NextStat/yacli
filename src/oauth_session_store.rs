use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::oauth::{AuthorizationRequest, AuthorizationSession, OauthService, unix_timestamp_now};
use crate::paths::oauth_sessions_path;
use crate::persist::write_config_file;

const PENDING_OAUTH_SESSION_TTL_SECS: u64 = 30 * 60;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PendingOauthSessionsFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    sessions: Vec<PendingOauthSession>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingOauthSession {
    pub account: String,
    pub services: Vec<String>,
    pub client_id: String,
    pub created_at_epoch_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_hint: Option<String>,
    pub authorization: PendingAuthorizationRequest,
    pub code_verifier: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingAuthorizationRequest {
    pub authorization_url: String,
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge_method: String,
}

impl PendingOauthSession {
    pub fn from_authorization_session(
        account: String,
        services: &[OauthService],
        login_hint: Option<String>,
        session: AuthorizationSession,
    ) -> Self {
        Self {
            account,
            services: services
                .iter()
                .map(|service| service.as_str().to_string())
                .collect(),
            client_id: session.client_id.clone(),
            created_at_epoch_secs: unix_timestamp_now(),
            login_hint,
            authorization: PendingAuthorizationRequest::from_request(session.request),
            code_verifier: session.code_verifier,
        }
    }

    pub fn to_authorization_session(&self) -> AuthorizationSession {
        AuthorizationSession {
            request: self.authorization.to_request(),
            client_id: self.client_id.clone(),
            code_verifier: self.code_verifier.clone(),
        }
    }

    pub fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.created_at_epoch_secs) > PENDING_OAUTH_SESSION_TTL_SECS
    }

    fn matches(&self, account: &str, services: &[OauthService], client_id: &str) -> bool {
        self.matches_service_names(account, client_id)
            && self.services
                == services
                    .iter()
                    .map(|service| service.as_str().to_string())
                    .collect::<Vec<_>>()
    }

    fn matches_service_names(&self, account: &str, client_id: &str) -> bool {
        self.account == account && self.client_id == client_id
    }
}

pub struct PendingOauthSessionStore {
    file: PendingOauthSessionsFile,
}

impl PendingOauthSessionStore {
    pub fn load() -> Result<Self> {
        let path = oauth_sessions_path()?;
        if !path.exists() {
            return Ok(Self {
                file: PendingOauthSessionsFile::default(),
            });
        }

        let content = fs::read_to_string(path)?;
        let mut file = toml::from_str::<PendingOauthSessionsFile>(&content)?;
        file.sessions
            .retain(|session| !session.is_expired(unix_timestamp_now()));
        Ok(Self { file })
    }

    pub fn save(&self) -> Result<()> {
        let path = oauth_sessions_path()?;
        if self.file.sessions.is_empty() {
            if path.exists() {
                fs::remove_file(path)?;
            }
            return Ok(());
        }

        let content = toml::to_string_pretty(&self.file)?;
        write_config_file(&path, &content)
    }

    pub fn get_matching(
        &self,
        account: &str,
        services: &[OauthService],
        client_id: &str,
    ) -> Option<&PendingOauthSession> {
        self.file
            .sessions
            .iter()
            .find(|session| session.matches(account, services, client_id))
    }

    pub fn replace_matching(&mut self, session: PendingOauthSession) {
        self.file.sessions.retain(|item| {
            !(item.matches_service_names(&session.account, &session.client_id)
                && item.services == session.services)
        });
        self.file.sessions.push(session);
    }

    pub fn remove_matching(
        &mut self,
        account: &str,
        services: &[OauthService],
        client_id: &str,
    ) -> bool {
        let previous_len = self.file.sessions.len();
        self.file
            .sessions
            .retain(|session| !session.matches(account, services, client_id));
        previous_len != self.file.sessions.len()
    }
}

const fn default_version() -> u32 {
    1
}

impl PendingAuthorizationRequest {
    fn from_request(request: AuthorizationRequest) -> Self {
        Self {
            authorization_url: request.authorization_url,
            redirect_uri: request.redirect_uri,
            state: request.state,
            code_challenge_method: request.code_challenge_method.to_string(),
        }
    }

    fn to_request(&self) -> AuthorizationRequest {
        AuthorizationRequest {
            authorization_url: self.authorization_url.clone(),
            redirect_uri: self.redirect_uri.clone(),
            state: self.state.clone(),
            code_challenge_method: "S256",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;

    static OAUTH_SESSIONS_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn sample_session(account: &str) -> PendingOauthSession {
        PendingOauthSession {
            account: account.to_string(),
            services: vec!["mail".to_string(), "disk".to_string()],
            client_id: "client-123".to_string(),
            created_at_epoch_secs: unix_timestamp_now(),
            login_hint: Some("me@yandex.ru".to_string()),
            authorization: PendingAuthorizationRequest {
                authorization_url: "https://example.test/authorize".to_string(),
                redirect_uri: "https://oauth.yandex.ru/verification_code".to_string(),
                state: "state-123".to_string(),
                code_challenge_method: "S256".to_string(),
            },
            code_verifier: "verifier-123".to_string(),
        }
    }

    #[test]
    fn store_roundtrips_pending_session() {
        let _guard = OAUTH_SESSIONS_TEST_LOCK
            .lock()
            .expect("oauth sessions test lock");
        let temp = tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("YACLI_CONFIG_DIR", temp.path());
        }

        let mut store = PendingOauthSessionStore::load().expect("load");
        store.replace_matching(sample_session("mock"));
        store.save().expect("save");

        let store = PendingOauthSessionStore::load().expect("reload");
        let saved = store
            .get_matching(
                "mock",
                &[OauthService::Mail, OauthService::Disk],
                "client-123",
            )
            .expect("pending session");
        assert_eq!(
            saved.authorization.authorization_url,
            "https://example.test/authorize"
        );
        assert_eq!(saved.code_verifier, "verifier-123");
    }

    #[test]
    fn store_prunes_expired_sessions_on_load() {
        let _guard = OAUTH_SESSIONS_TEST_LOCK
            .lock()
            .expect("oauth sessions test lock");
        let temp = tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("YACLI_CONFIG_DIR", temp.path());
        }

        let mut stale = sample_session("mock");
        stale.created_at_epoch_secs = unix_timestamp_now() - PENDING_OAUTH_SESSION_TTL_SECS - 1;

        let mut store = PendingOauthSessionStore::load().expect("load");
        store.replace_matching(stale);
        store.save().expect("save");

        let store = PendingOauthSessionStore::load().expect("reload");
        assert!(
            store
                .get_matching(
                    "mock",
                    &[OauthService::Mail, OauthService::Disk],
                    "client-123"
                )
                .is_none()
        );
    }
}
