use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cli::{CalendarAuthModeArg, DiskAuthModeArg, MailAuthModeArg};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountConfig>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            accounts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountConfig {
    pub email: String,
    #[serde(default)]
    pub default: bool,
    pub mail: MailConfig,
    pub calendar: CalendarConfig,
    pub disk: DiskConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailConfig {
    pub enabled: bool,
    pub auth_mode: MailAuthMode,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalendarConfig {
    pub enabled: bool,
    pub auth_mode: CalendarAuthMode,
    pub caldav_base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskConfig {
    pub enabled: bool,
    pub auth_mode: DiskAuthMode,
    pub rest_base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailAuthMode {
    OauthXoauth2,
    AppPassword,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarAuthMode {
    AppPassword,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskAuthMode {
    Oauth,
}

impl From<MailAuthModeArg> for MailAuthMode {
    fn from(value: MailAuthModeArg) -> Self {
        match value {
            MailAuthModeArg::OauthXoauth2 => Self::OauthXoauth2,
            MailAuthModeArg::AppPassword => Self::AppPassword,
        }
    }
}

impl From<CalendarAuthModeArg> for CalendarAuthMode {
    fn from(value: CalendarAuthModeArg) -> Self {
        match value {
            CalendarAuthModeArg::AppPassword => Self::AppPassword,
        }
    }
}

impl From<DiskAuthModeArg> for DiskAuthMode {
    fn from(value: DiskAuthModeArg) -> Self {
        match value {
            DiskAuthModeArg::Oauth => Self::Oauth,
        }
    }
}

pub struct NewAccountInput {
    pub email: String,
    pub default: bool,
    pub mail_auth_mode: MailAuthMode,
    pub calendar_auth_mode: CalendarAuthMode,
    pub disk_auth_mode: DiskAuthMode,
    pub mail_credential_ref: Option<String>,
    pub calendar_credential_ref: Option<String>,
    pub disk_credential_ref: Option<String>,
}

impl AccountConfig {
    pub fn new(input: NewAccountInput) -> Self {
        Self {
            email: input.email,
            default: input.default,
            mail: MailConfig {
                enabled: true,
                auth_mode: input.mail_auth_mode,
                imap_host: "imap.yandex.com".to_string(),
                imap_port: 993,
                smtp_host: "smtp.yandex.com".to_string(),
                smtp_port: 465,
                credential_ref: input.mail_credential_ref,
            },
            calendar: CalendarConfig {
                enabled: true,
                auth_mode: input.calendar_auth_mode,
                caldav_base_url: "https://caldav.yandex.ru".to_string(),
                credential_ref: input.calendar_credential_ref,
            },
            disk: DiskConfig {
                enabled: true,
                auth_mode: input.disk_auth_mode,
                rest_base_url: "https://cloud-api.yandex.net".to_string(),
                credential_ref: input.disk_credential_ref,
            },
        }
    }
}

const fn default_version() -> u32 {
    1
}
