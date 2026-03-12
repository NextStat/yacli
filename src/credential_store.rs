use std::collections::BTreeMap;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::paths::credentials_path;

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

#[derive(Clone, Debug)]
pub struct CredentialStore {
    pub file: CredentialsFile,
}

impl CredentialStore {
    pub fn load() -> Result<Self> {
        let path = credentials_path()?;
        if !path.exists() {
            return Ok(Self {
                file: CredentialsFile::default(),
            });
        }

        let content = fs::read_to_string(path)?;
        let file = toml::from_str::<CredentialsFile>(&content)?;
        Ok(Self { file })
    }

    pub fn save(&self) -> Result<()> {
        let path = credentials_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(&self.file)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn get_oauth(&self, account_name: &str, service: &str) -> Option<&StoredOauthCredential> {
        self.file
            .accounts
            .get(account_name)
            .and_then(|account| account.services.get(service))
            .and_then(|credential| match credential {
                StoredCredential::Oauth(credential) => Some(credential),
                StoredCredential::AppPassword(_) => None,
            })
    }

    pub fn get_app_password(
        &self,
        account_name: &str,
        service: &str,
    ) -> Option<&StoredAppPasswordCredential> {
        self.file
            .accounts
            .get(account_name)
            .and_then(|account| account.services.get(service))
            .and_then(|credential| match credential {
                StoredCredential::Oauth(_) => None,
                StoredCredential::AppPassword(credential) => Some(credential),
            })
    }

    pub fn get_service(&self, account_name: &str, service: &str) -> Option<&StoredCredential> {
        self.file
            .accounts
            .get(account_name)
            .and_then(|account| account.services.get(service))
    }

    pub fn set_oauth(
        &mut self,
        account_name: String,
        service: String,
        credential: StoredOauthCredential,
    ) {
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
        self.file
            .accounts
            .entry(account_name)
            .or_default()
            .services
            .insert(service, StoredCredential::AppPassword(credential));
    }

    pub fn remove_service(&mut self, account_name: &str, service: &str) -> bool {
        let Some(account_entry) = self.file.accounts.get_mut(account_name) else {
            return false;
        };

        let removed = account_entry.services.remove(service).is_some();
        if account_entry.services.is_empty() {
            self.file.accounts.remove(account_name);
        }
        removed
    }
}

const fn default_version() -> u32 {
    1
}
