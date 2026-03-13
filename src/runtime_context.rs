use serde::Serialize;

use crate::account_store::AccountStore;
use crate::credential_store::{CredentialStore, StoredCredential};
use crate::error::{Result, YacliError};
use crate::mail::MailSessionAuth;
use crate::model::{AccountConfig, CalendarAuthMode, MailAuthMode};
use crate::oauth::{OauthService, unix_timestamp_now};

#[derive(Clone, Debug, Serialize)]
pub struct CredentialState {
    pub credential_ref: Option<String>,
    pub credential_state: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct MailConnectionContext {
    pub email: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
}

#[derive(Clone, Debug)]
pub struct CalendarConnectionContext {
    pub email: String,
    pub caldav_base_url: String,
}

#[derive(Clone, Copy)]
enum CredentialReference<'a> {
    Env(&'a str),
    Store(&'a str),
}

pub fn auth_state(
    credential_store: &CredentialStore,
    account_name: &str,
    reference: Option<&str>,
    service: &str,
) -> CredentialState {
    match reference {
        None => CredentialState {
            credential_ref: None,
            credential_state: "not_configured",
            detail: "служба еще не подключена".to_string(),
        },
        Some(raw) => match parse_credential_ref(raw) {
            Some(CredentialReference::Env(var_name)) => match std::env::var_os(var_name) {
                Some(_) => CredentialState {
                    credential_ref: Some(raw.to_string()),
                    credential_state: "env_present",
                    detail: format!("используется переменная окружения {var_name}"),
                },
                None => CredentialState {
                    credential_ref: Some(raw.to_string()),
                    credential_state: "env_missing",
                    detail: format!("переменная окружения {var_name} не задана"),
                },
            },
            Some(CredentialReference::Store(store_service)) => {
                if store_service != service {
                    return CredentialState {
                        credential_ref: Some(raw.to_string()),
                        credential_state: "store_mismatch",
                        detail: format!("ссылка указывает на store:{store_service}"),
                    };
                }

                match credential_store.get_service(account_name, service) {
                    Some(StoredCredential::Oauth(credential)) => {
                        let now = unix_timestamp_now();
                        if credential.expires_at_epoch_secs > now.saturating_add(60) {
                            CredentialState {
                                credential_ref: Some(raw.to_string()),
                                credential_state: "store_present",
                                detail: format!(
                                    "сохраненный OAuth-токен действует до {}",
                                    credential.expires_at_epoch_secs
                                ),
                            }
                        } else {
                            CredentialState {
                                credential_ref: Some(raw.to_string()),
                                credential_state: "store_expired",
                                detail: "сохраненный OAuth-токен истек или скоро истечет; выполните `yacli login` еще раз".to_string(),
                            }
                        }
                    }
                    Some(StoredCredential::AppPassword(_)) => CredentialState {
                        credential_ref: Some(raw.to_string()),
                        credential_state: "store_present",
                        detail: "пароль приложения сохранен локально".to_string(),
                    },
                    None => CredentialState {
                        credential_ref: Some(raw.to_string()),
                        credential_state: "store_missing",
                        detail: format!("локальный секрет для {service} не найден"),
                    },
                }
            }
            None => CredentialState {
                credential_ref: Some(raw.to_string()),
                credential_state: "unsupported_reference",
                detail: "поддерживаются только env:NAME и store:SERVICE".to_string(),
            },
        },
    }
}

pub fn ensure_calendar_supports_app_password(account: &AccountConfig) -> Result<()> {
    if !matches!(account.calendar.auth_mode, CalendarAuthMode::AppPassword) {
        return Err(YacliError::UnsupportedOperation(
            "calendar login currently supports only account.calendar.auth_mode=app_password"
                .to_string(),
        ));
    }

    Ok(())
}

pub fn resolve_disk_private_context(account: Option<&str>) -> Result<(String, String, String)> {
    let account_store = AccountStore::load()?;
    let name = account_store.resolved_account_name(account)?;
    let account = account_store.get_account(&name)?;
    let access_token = resolve_oauth_access_token(
        &name,
        account.disk.credential_ref.as_deref(),
        OauthService::Disk,
    )?;

    Ok((name, account.disk.rest_base_url.clone(), access_token))
}

pub fn resolve_mail_private_context(
    account: Option<&str>,
) -> Result<(String, MailSessionAuth, MailConnectionContext)> {
    let account_store = AccountStore::load()?;
    let name = account_store.resolved_account_name(account)?;
    let account = account_store.get_account(&name)?;
    let email = account.email.clone();
    let imap_host = account.mail.imap_host.clone();
    let imap_port = account.mail.imap_port;
    let smtp_host = account.mail.smtp_host.clone();
    let smtp_port = account.mail.smtp_port;

    let auth = match account.mail.auth_mode {
        MailAuthMode::OauthXoauth2 => {
            let access_token = resolve_oauth_access_token(
                &name,
                account.mail.credential_ref.as_deref(),
                OauthService::Mail,
            )?;
            MailSessionAuth::OauthXoauth2 {
                account: email.clone(),
                access_token,
            }
        }
        MailAuthMode::AppPassword => {
            let app_password =
                resolve_app_password_secret(&name, "mail", account.mail.credential_ref.as_deref())?;
            MailSessionAuth::AppPassword {
                account: email.clone(),
                app_password,
            }
        }
    };

    Ok((
        name,
        auth,
        MailConnectionContext {
            email,
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
        },
    ))
}

pub fn resolve_calendar_private_context(
    account: Option<&str>,
) -> Result<(String, String, CalendarConnectionContext)> {
    let account_store = AccountStore::load()?;
    let name = account_store.resolved_account_name(account)?;
    let account = account_store.get_account(&name)?;
    ensure_calendar_supports_app_password(account)?;

    let app_password = resolve_app_password_secret(
        &name,
        "calendar",
        account.calendar.credential_ref.as_deref(),
    )?;

    Ok((
        name,
        app_password,
        CalendarConnectionContext {
            email: account.email.clone(),
            caldav_base_url: account.calendar.caldav_base_url.clone(),
        },
    ))
}

pub fn resolve_oauth_access_token(
    account_name: &str,
    reference: Option<&str>,
    service: OauthService,
) -> Result<String> {
    let Some(reference) = reference else {
        return Err(YacliError::Auth(format!(
            "{} has no credential configured for {}",
            account_name,
            service.as_str()
        )));
    };

    match parse_credential_ref(reference) {
        Some(CredentialReference::Env(var_name)) => required_env(var_name),
        Some(CredentialReference::Store(store_service)) => {
            if store_service != service.as_str() {
                return Err(YacliError::Config(format!(
                    "credential_ref {} does not match requested service {}",
                    reference,
                    service.as_str()
                )));
            }

            let credential_store = CredentialStore::load()?;
            let stored = credential_store
                .get_oauth(account_name, service.as_str())
                .ok_or_else(|| {
                    YacliError::Auth(format!(
                        "no stored OAuth credential for account {} service {}",
                        account_name,
                        service.as_str()
                    ))
                })?;

            if stored.expires_at_epoch_secs <= unix_timestamp_now().saturating_add(60) {
                return Err(YacliError::Auth(format!(
                    "stored OAuth token expired for account {} service {}; run `yacli login --account {}` again",
                    account_name,
                    service.as_str(),
                    account_name
                )));
            }

            Ok(stored.access_token.clone())
        }
        None => Err(YacliError::Config(format!(
            "unsupported credential_ref format: {reference}"
        ))),
    }
}

pub fn resolve_app_password_secret(
    account_name: &str,
    service: &str,
    reference: Option<&str>,
) -> Result<String> {
    let Some(reference) = reference else {
        return Err(YacliError::Auth(format!(
            "{account_name} has no credential configured for {service} app password"
        )));
    };

    match parse_credential_ref(reference) {
        Some(CredentialReference::Env(var_name)) => required_env(var_name),
        Some(CredentialReference::Store(store_service)) => {
            if store_service != service {
                return Err(YacliError::Config(format!(
                    "credential_ref {} does not match requested service {}",
                    reference, service
                )));
            }

            let credential_store = CredentialStore::load()?;
            let stored = credential_store
                .get_app_password(account_name, service)
                .ok_or_else(|| {
                    YacliError::Auth(format!(
                        "no stored app password for account {} service {}",
                        account_name, service
                    ))
                })?;
            Ok(stored.secret.clone())
        }
        None => Err(YacliError::Config(format!(
            "unsupported credential_ref format: {reference}"
        ))),
    }
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| {
        YacliError::Config(format!("required environment variable is missing: {name}"))
    })
}

fn parse_credential_ref(raw: &str) -> Option<CredentialReference<'_>> {
    if let Some(value) = raw.strip_prefix("env:") {
        return Some(CredentialReference::Env(value));
    }
    if let Some(value) = raw.strip_prefix("store:") {
        return Some(CredentialReference::Store(value));
    }
    None
}
