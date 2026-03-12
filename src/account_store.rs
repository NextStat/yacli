use std::collections::BTreeMap;
use std::fs;

use url::Url;

use crate::error::{Result, YacliError};
use crate::model::{AccountConfig, AccountsFile};
use crate::paths::accounts_path;

#[derive(Clone, Debug)]
pub struct AccountStore {
    pub file: AccountsFile,
}

impl AccountStore {
    pub fn load() -> Result<Self> {
        let path = accounts_path()?;
        if !path.exists() {
            return Ok(Self {
                file: AccountsFile::default(),
            });
        }

        let content = fs::read_to_string(path)?;
        let file = toml::from_str::<AccountsFile>(&content)?;
        Ok(Self { file })
    }

    pub fn save(&self) -> Result<()> {
        let path = accounts_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(&self.file)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn add_account(&mut self, name: String, mut account: AccountConfig) -> Result<()> {
        if self.file.accounts.contains_key(&name) {
            return Err(YacliError::AccountExists(name));
        }
        if account.default || self.file.accounts.is_empty() {
            self.clear_default();
            account.default = true;
        }
        self.file.accounts.insert(name, account);
        Ok(())
    }

    pub fn set_current(&mut self, name: &str) -> Result<()> {
        if !self.file.accounts.contains_key(name) {
            return Err(YacliError::AccountNotFound(name.to_string()));
        }

        self.clear_default();
        if let Some(account) = self.file.accounts.get_mut(name) {
            account.default = true;
        }
        Ok(())
    }

    pub fn resolved_account_name(&self, requested: Option<&str>) -> Result<String> {
        if let Some(name) = requested {
            if self.file.accounts.contains_key(name) {
                return Ok(name.to_string());
            }
            return Err(YacliError::AccountNotFound(name.to_string()));
        }

        if let Some(name) = self
            .file
            .accounts
            .iter()
            .find_map(|(name, account)| account.default.then(|| name.clone()))
        {
            return Ok(name);
        }

        if self.file.accounts.len() == 1 {
            return Ok(self
                .file
                .accounts
                .keys()
                .next()
                .cloned()
                .expect("single account key exists"));
        }

        Err(YacliError::CurrentAccountMissing)
    }

    pub fn current_account_name(&self) -> Result<String> {
        self.resolved_account_name(None)
    }

    pub fn is_current_account(&self, name: &str) -> bool {
        self.current_account_name()
            .map(|current| current == name)
            .unwrap_or(false)
    }

    pub fn get_account(&self, name: &str) -> Result<&AccountConfig> {
        self.file
            .accounts
            .get(name)
            .ok_or_else(|| YacliError::AccountNotFound(name.to_string()))
    }

    pub fn summaries(&self) -> BTreeMap<String, &AccountConfig> {
        self.file
            .accounts
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect()
    }

    pub fn set_service_credential_ref(
        &mut self,
        account_name: &str,
        service: &str,
        credential_ref: Option<String>,
    ) -> Result<()> {
        let account = self
            .file
            .accounts
            .get_mut(account_name)
            .ok_or_else(|| YacliError::AccountNotFound(account_name.to_string()))?;

        match service {
            "mail" => account.mail.credential_ref = credential_ref,
            "calendar" => account.calendar.credential_ref = credential_ref,
            "disk" => account.disk.credential_ref = credential_ref,
            _ => {
                return Err(YacliError::Config(format!(
                    "unknown service for credential_ref update: {service}"
                )));
            }
        }

        Ok(())
    }

    fn clear_default(&mut self) {
        for account in self.file.accounts.values_mut() {
            account.default = false;
        }
    }
}

#[derive(Debug)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
}

pub fn validate_account(name: &str, account: &AccountConfig) -> ValidationReport {
    let mut errors = Vec::new();

    if name.trim().is_empty() {
        errors.push("account name must not be empty".to_string());
    }

    if !account.email.contains('@') {
        errors.push("email must contain @".to_string());
    }

    if account.mail.imap_host.trim().is_empty() {
        errors.push("mail.imap_host must not be empty".to_string());
    }
    if account.mail.smtp_host.trim().is_empty() {
        errors.push("mail.smtp_host must not be empty".to_string());
    }
    if account.mail.imap_port == 0 {
        errors.push("mail.imap_port must not be zero".to_string());
    }
    if account.mail.smtp_port == 0 {
        errors.push("mail.smtp_port must not be zero".to_string());
    }

    if Url::parse(&account.calendar.caldav_base_url).is_err() {
        errors.push("calendar.caldav_base_url must be a valid URL".to_string());
    }
    if Url::parse(&account.disk.rest_base_url).is_err() {
        errors.push("disk.rest_base_url must be a valid URL".to_string());
    }

    for (label, value) in [
        (
            "mail.credential_ref",
            account.mail.credential_ref.as_deref(),
        ),
        (
            "disk.credential_ref",
            account.disk.credential_ref.as_deref(),
        ),
    ] {
        if let Some(reference) = value
            && !reference.starts_with("env:")
            && !reference.starts_with("store:")
        {
            errors.push(format!("{label} must use env:NAME or store:SERVICE"));
        }
    }

    if let Some(reference) = account.calendar.credential_ref.as_deref()
        && !reference.starts_with("env:")
        && !reference.starts_with("store:")
    {
        errors.push("calendar.credential_ref must use env:NAME or store:SERVICE".to_string());
    }

    ValidationReport {
        valid: errors.is_empty(),
        errors,
    }
}
