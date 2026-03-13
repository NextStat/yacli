use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;

use crate::error::{Result, YacliError};
use crate::paths::credentials_path;
use crate::persist::write_config_file;

const SECRET_BACKEND_ENV: &str = "YACLI_SECRET_BACKEND";
const KEYRING_SERVICE_NAME: &str = "com.nextstat.yacli";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CredentialsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountCredentialSet>,
}

impl Default for CredentialsFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            accounts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AccountCredentialSet {
    #[serde(default)]
    pub services: BTreeMap<String, StoredCredential>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StoredCredential {
    Oauth(StoredOauthCredential),
    AppPassword(StoredAppPasswordCredential),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredOauthCredential {
    pub kind: String,
    pub access_token: String,
    pub token_type: String,
    pub expires_at_epoch_secs: u64,
    #[serde(default)]
    pub scope: Vec<String>,
    pub client_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredAppPasswordCredential {
    pub kind: String,
    pub secret: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SecretBackendKind {
    File,
    Keyring,
}

#[derive(Clone, Debug)]
pub struct CredentialStore {
    backend: SecretBackendKind,
    pub file: CredentialsFile,
    removed: BTreeSet<(String, String)>,
}

impl CredentialStore {
    pub fn load() -> Result<Self> {
        let backend = configured_secret_backend()?;
        let file = if backend == SecretBackendKind::File {
            load_credentials_file()?
        } else {
            migrate_legacy_credentials_file_to_keyring()?;
            CredentialsFile::default()
        };

        Ok(Self {
            backend,
            file,
            removed: BTreeSet::new(),
        })
    }

    pub fn save(&mut self) -> Result<()> {
        match self.backend {
            SecretBackendKind::File => {
                let path = credentials_path()?;
                let content = toml::to_string_pretty(&self.file)?;
                write_config_file(&path, &content)
            }
            SecretBackendKind::Keyring => {
                for (account_name, account) in &self.file.accounts {
                    for (service, credential) in &account.services {
                        keyring_set(account_name, service, credential)?;
                    }
                }
                for (account_name, service) in &self.removed {
                    keyring_delete(account_name, service)?;
                }
                self.file = CredentialsFile::default();
                self.removed.clear();
                Ok(())
            }
        }
    }

    pub fn get_oauth(&self, account_name: &str, service: &str) -> Option<StoredOauthCredential> {
        self.get_service(account_name, service)
            .and_then(|credential| match credential {
                StoredCredential::Oauth(credential) => Some(credential),
                StoredCredential::AppPassword(_) => None,
            })
    }

    pub fn get_app_password(
        &self,
        account_name: &str,
        service: &str,
    ) -> Option<StoredAppPasswordCredential> {
        self.get_service(account_name, service)
            .and_then(|credential| match credential {
                StoredCredential::Oauth(_) => None,
                StoredCredential::AppPassword(credential) => Some(credential),
            })
    }

    pub fn get_service(&self, account_name: &str, service: &str) -> Option<StoredCredential> {
        let key = (account_name.to_string(), service.to_string());
        if self.removed.contains(&key) {
            return None;
        }

        if let Some(credential) = self
            .file
            .accounts
            .get(account_name)
            .and_then(|account| account.services.get(service))
        {
            return Some(credential.clone());
        }

        match self.backend {
            SecretBackendKind::File => None,
            SecretBackendKind::Keyring => keyring_get(account_name, service).ok().flatten(),
        }
    }

    pub fn set_oauth(
        &mut self,
        account_name: String,
        service: String,
        credential: StoredOauthCredential,
    ) {
        self.removed
            .remove(&(account_name.clone(), service.clone()));
        self.file
            .accounts
            .entry(account_name)
            .or_default()
            .services
            .insert(service, StoredCredential::Oauth(credential));
    }

    pub fn set_app_password(
        &mut self,
        account_name: String,
        service: String,
        credential: StoredAppPasswordCredential,
    ) {
        self.removed
            .remove(&(account_name.clone(), service.clone()));
        self.file
            .accounts
            .entry(account_name)
            .or_default()
            .services
            .insert(service, StoredCredential::AppPassword(credential));
    }

    pub fn remove_service(&mut self, account_name: &str, service: &str) -> bool {
        let removed = self.get_service(account_name, service).is_some();
        if !removed {
            return false;
        }

        if let Some(account_entry) = self.file.accounts.get_mut(account_name) {
            account_entry.services.remove(service);
            if account_entry.services.is_empty() {
                self.file.accounts.remove(account_name);
            }
        }

        if self.backend == SecretBackendKind::Keyring {
            self.removed
                .insert((account_name.to_string(), service.to_string()));
        }

        true
    }
}

fn configured_secret_backend() -> Result<SecretBackendKind> {
    match env::var(SECRET_BACKEND_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("file") => Ok(SecretBackendKind::File),
        Some("keyring") | None | Some("") => supported_keyring_backend(),
        Some(other) => Err(YacliError::Config(format!(
            "unsupported secret backend `{other}`; expected `keyring` or `file`"
        ))),
    }
}

fn supported_keyring_backend() -> Result<SecretBackendKind> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        Ok(SecretBackendKind::Keyring)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err(YacliError::UnsupportedOperation(
            "system keyring backend is not supported on this target; set YACLI_SECRET_BACKEND=file if you need legacy plaintext storage"
                .to_string(),
        ))
    }
}

fn load_credentials_file() -> Result<CredentialsFile> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(CredentialsFile::default());
    }

    let content = fs::read_to_string(path)?;
    toml::from_str::<CredentialsFile>(&content).map_err(Into::into)
}

fn migrate_legacy_credentials_file_to_keyring() -> Result<()> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(());
    }

    let legacy = load_credentials_file()?;
    if legacy.accounts.is_empty() {
        fs::remove_file(&path)?;
        return Ok(());
    }

    for (account_name, account) in &legacy.accounts {
        for (service, credential) in &account.services {
            keyring_set(account_name, service, credential)?;
        }
    }

    fs::remove_file(path)?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn keyring_entry(account_name: &str, service: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE_NAME, &format!("{account_name}:{service}"))
        .map_err(|err| YacliError::Auth(format!("failed to create keyring entry: {err}")))
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn keyring_set(account_name: &str, service: &str, credential: &StoredCredential) -> Result<()> {
    let entry = keyring_entry(account_name, service)?;
    let payload = serde_json::to_string(credential)
        .map_err(|err| YacliError::Serialization(err.to_string()))?;
    entry
        .set_password(&payload)
        .map_err(|err| YacliError::Auth(format!("failed to store secret in system keyring: {err}")))
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn keyring_get(account_name: &str, service: &str) -> Result<Option<StoredCredential>> {
    let entry = keyring_entry(account_name, service)?;
    match entry.get_password() {
        Ok(payload) => serde_json::from_str::<StoredCredential>(&payload)
            .map(Some)
            .map_err(|err| {
                YacliError::Serialization(format!(
                    "invalid secret payload in system keyring: {err}"
                ))
            }),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(YacliError::Auth(format!(
            "failed to read secret from system keyring: {err}"
        ))),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn keyring_delete(account_name: &str, service: &str) -> Result<()> {
    let entry = keyring_entry(account_name, service)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(YacliError::Auth(format!(
            "failed to delete secret from system keyring: {err}"
        ))),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn keyring_set(_account_name: &str, _service: &str, _credential: &StoredCredential) -> Result<()> {
    Err(YacliError::UnsupportedOperation(
        "system keyring backend is not supported on this target".to_string(),
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn keyring_get(_account_name: &str, _service: &str) -> Result<Option<StoredCredential>> {
    Err(YacliError::UnsupportedOperation(
        "system keyring backend is not supported on this target".to_string(),
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn keyring_delete(_account_name: &str, _service: &str) -> Result<()> {
    Err(YacliError::UnsupportedOperation(
        "system keyring backend is not supported on this target".to_string(),
    ))
}

const fn default_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    use tempfile::tempdir;

    static CREDENTIAL_STORE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn file_backend_round_trips_credentials_file() {
        let _guard = CREDENTIAL_STORE_TEST_LOCK
            .lock()
            .expect("credential store test lock");
        let temp = tempdir().expect("tempdir");
        unsafe {
            env::set_var("YACLI_CONFIG_DIR", temp.path());
            env::set_var(SECRET_BACKEND_ENV, "file");
        }

        let mut store = CredentialStore::load().expect("load store");
        store.set_app_password(
            "personal".to_string(),
            "calendar".to_string(),
            StoredAppPasswordCredential {
                kind: "app_password".to_string(),
                secret: "secret-value".to_string(),
            },
        );
        store.save().expect("save store");

        let reloaded = CredentialStore::load().expect("reload store");
        let credential = reloaded
            .get_app_password("personal", "calendar")
            .expect("calendar secret");
        assert_eq!(credential.secret, "secret-value");

        unsafe {
            env::remove_var("YACLI_CONFIG_DIR");
            env::remove_var(SECRET_BACKEND_ENV);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn keyring_backend_migrates_legacy_credentials_file() {
        let _guard = CREDENTIAL_STORE_TEST_LOCK
            .lock()
            .expect("credential store test lock");
        let temp = tempdir().expect("tempdir");
        unsafe {
            env::set_var("YACLI_CONFIG_DIR", temp.path());
            env::set_var(SECRET_BACKEND_ENV, "keyring");
        }

        let credential = StoredCredential::Oauth(StoredOauthCredential {
            kind: "oauth_pkce".to_string(),
            access_token: "token-value".to_string(),
            token_type: "Bearer".to_string(),
            expires_at_epoch_secs: 1_900_000_000,
            scope: vec!["mail:imap_full".to_string()],
            client_id: "client".to_string(),
        });
        let legacy = CredentialsFile {
            version: 1,
            accounts: BTreeMap::from([(
                "personal".to_string(),
                AccountCredentialSet {
                    services: BTreeMap::from([("mail".to_string(), credential.clone())]),
                },
            )]),
        };
        let content = toml::to_string_pretty(&legacy).expect("legacy content");
        write_config_file(&credentials_path().expect("path"), &content).expect("legacy file");

        let mock = keyring::mock::default_credential_builder();
        keyring::set_default_credential_builder(mock);

        let store = CredentialStore::load().expect("load store");
        assert!(!credentials_path().expect("path").exists());
        assert!(matches!(store.backend, SecretBackendKind::Keyring));

        unsafe {
            env::remove_var("YACLI_CONFIG_DIR");
            env::remove_var(SECRET_BACKEND_ENV);
        }
    }
}
