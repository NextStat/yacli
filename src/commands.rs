use std::collections::BTreeMap;
use std::io::{self, Write};

use serde::Serialize;
use serde_json::json;

use crate::account_store::{AccountStore, validate_account};
use crate::calendar::{
    CalendarCollection, CalendarCreateRequest, CalendarEvent, CalendarEventWindow,
    CalendarEventsRequest, create_calendar_event, delete_calendar_event, list_calendar_events,
    list_calendars, parse_event_window,
};
use crate::cli::{
    AccountCommand, AuthCommand, AuthServiceArg, CalendarCommand, Cli, Command, DiskCommand,
    DiskPublicCommand, GuideTopicArg, MailCommand, OutputFormat,
};
use crate::credential_store::{CredentialStore, StoredAppPasswordCredential, StoredCredential};
use crate::disk::{
    DEFAULT_DISK_BASE_URL, DiskInfo, DiskResource, DownloadedFile, PrivateDiskListRequest,
    PrivateDiskMkdirRequest, PrivateDiskUploadRequest, PublicDiskRequest, PublicDownloadRequest,
    PublicResource, UploadedFile, create_private_directory, download_public_resource,
    fetch_disk_info, fetch_private_resource, fetch_public_resource, upload_private_resource,
};
use crate::error::{Result, YacliError};
use crate::mail::{
    ForwardedMail, MailAttachmentSummary, MailFolder, MailForwardRequest, MailMessage,
    MailMessageSummary, MailReplyRequest, MailSendRequest, MailSessionAuth, RepliedMail, SentMail,
    forward_mail_message, list_mail_folders, list_mail_messages, read_mail_message,
    reply_to_mail_message, search_mail_messages, send_mail_message,
};
use crate::model::{AccountConfig, CalendarAuthMode, MailAuthMode, NewAccountInput};
use crate::oauth::{
    OauthService, default_yacli_client_id, exchange_authorization_code, start_pkce_authorization,
    unix_timestamp_now,
};
use crate::output::RenderedOutput;

pub fn execute(cli: Cli) -> Result<RenderedOutput> {
    match cli.command {
        Command::Guide { topic } => execute_guide(cli.format, topic),
        Command::Add { email, name } => execute_simple_add(cli.format, email, name),
        Command::Accounts => execute_account(cli.format, AccountCommand::List),
        Command::Use { name } => execute_account(cli.format, AccountCommand::Use { name }),
        Command::Whoami => execute_account(cli.format, AccountCommand::Current),
        Command::Status { account } => execute_auth(cli.format, AuthCommand::Status { account }),
        Command::Login {
            service,
            account,
            client_id,
            env_var,
            app_password,
            code,
            login_hint,
        } => execute_auth(
            cli.format,
            AuthCommand::Login {
                account,
                service,
                client_id,
                env_var,
                app_password,
                code,
                login_hint,
            },
        ),
        Command::Logout { service, account } => execute_simple_logout(cli.format, account, service),
        Command::Account { action } => execute_account(cli.format, action),
        Command::Auth { action } => execute_auth(cli.format, action),
        Command::Disk { action } => execute_disk(cli.format, action),
        Command::Calendar { action } => execute_calendar(cli.format, action),
        Command::Mail { action } => execute_mail(cli.format, action),
    }
}

fn execute_simple_add(
    format: OutputFormat,
    email: String,
    name: Option<String>,
) -> Result<RenderedOutput> {
    let name = name.unwrap_or_else(|| derive_account_name_from_email(&email));
    execute_account(
        format,
        AccountCommand::Add {
            name,
            email,
            use_as_current: true,
            mail_auth_mode: crate::cli::MailAuthModeArg::OauthXoauth2,
            calendar_auth_mode: crate::cli::CalendarAuthModeArg::AppPassword,
            disk_auth_mode: crate::cli::DiskAuthModeArg::Oauth,
            mail_credential_ref: None,
            calendar_credential_ref: None,
            disk_credential_ref: None,
        },
    )
}

fn execute_simple_logout(
    format: OutputFormat,
    account: Option<String>,
    service: Option<AuthServiceArg>,
) -> Result<RenderedOutput> {
    let services = match service {
        Some(service) => vec![service],
        None => vec![
            AuthServiceArg::Mail,
            AuthServiceArg::Disk,
            AuthServiceArg::Calendar,
        ],
    };

    let mut items = Vec::with_capacity(services.len());
    for service in services {
        let output = execute_auth(
            OutputFormat::Json,
            AuthCommand::Logout {
                account: account.clone(),
                service: Some(service),
            },
        )?;
        items.push(output.json);
    }

    let account_name = items
        .first()
        .and_then(|item| item.get("account"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let table = if items.is_empty() {
        "No services logged out".to_string()
    } else {
        let mut lines = vec!["SERVICE\tREMOVED\tCLEARED_ACCOUNT_REF".to_string()];
        for item in &items {
            lines.push(format!(
                "{}\t{}\t{}",
                item["service"].as_str().unwrap_or_default(),
                item["removed"].as_bool().unwrap_or(false),
                item["cleared_account_ref"].as_bool().unwrap_or(false)
            ));
        }
        lines.join("\n")
    };

    ok_output(
        format,
        "logout",
        json!({
            "account": account_name,
            "items": items,
        }),
        table,
    )
}

fn execute_guide(format: OutputFormat, topic: GuideTopicArg) -> Result<RenderedOutput> {
    let topic_name = guide_topic_name(topic);
    let commands = guide_commands(topic);
    let workflows = guide_workflows(topic);

    ok_output(
        format,
        "guide.show",
        json!({
            "topic": topic_name,
            "version": env!("CARGO_PKG_VERSION"),
            "commands": commands,
            "workflows": workflows,
        }),
        render_guide_table(topic_name, &commands, &workflows),
    )
}

fn execute_account(format: OutputFormat, action: AccountCommand) -> Result<RenderedOutput> {
    match action {
        AccountCommand::Add {
            name,
            email,
            use_as_current,
            mail_auth_mode,
            calendar_auth_mode,
            disk_auth_mode,
            mail_credential_ref,
            calendar_credential_ref,
            disk_credential_ref,
        } => {
            let account_config = AccountConfig::new(NewAccountInput {
                email,
                default: use_as_current,
                mail_auth_mode: mail_auth_mode.into(),
                calendar_auth_mode: calendar_auth_mode.into(),
                disk_auth_mode: disk_auth_mode.into(),
                mail_credential_ref,
                calendar_credential_ref,
                disk_credential_ref,
            });
            let report = validate_account(&name, &account_config);
            if !report.valid {
                return Err(YacliError::Validation(report.errors.join("; ")));
            }

            let mut store = AccountStore::load()?;
            store.add_account(name.clone(), account_config.clone())?;
            let current = store.current_account_name()? == name;
            store.save()?;

            ok_output(
                format,
                "account.add",
                json!({
                    "account": name,
                    "config": account_config,
                    "current": current,
                }),
                render_key_value_table(&[
                    ("operation", "account.add".to_string()),
                    ("account", name),
                    ("email", account_config.email),
                    ("current", current.to_string()),
                ]),
            )
        }
        AccountCommand::List => {
            let store = AccountStore::load()?;
            let items: Vec<_> = store
                .summaries()
                .into_iter()
                .map(|(name, account)| {
                    let current = store.is_current_account(&name);
                    json!({
                        "name": name,
                        "email": account.email,
                        "current": current,
                        "services": {
                            "mail": account.mail.enabled,
                            "calendar": account.calendar.enabled,
                            "disk": account.disk.enabled,
                        }
                    })
                })
                .collect();

            let table = if items.is_empty() {
                "No accounts configured".to_string()
            } else {
                let mut lines = vec!["NAME\tEMAIL\tCURRENT".to_string()];
                for item in &items {
                    lines.push(format!(
                        "{}\t{}\t{}",
                        item["name"].as_str().unwrap_or_default(),
                        item["email"].as_str().unwrap_or_default(),
                        item["current"].as_bool().unwrap_or(false)
                    ));
                }
                lines.join("\n")
            };

            ok_output(format, "account.list", json!({ "items": items }), table)
        }
        AccountCommand::Show { account } => {
            let store = AccountStore::load()?;
            let name = store.resolved_account_name(account.as_deref())?;
            let account = store.get_account(&name)?;
            let current = store.is_current_account(&name);
            ok_output(
                format,
                "account.show",
                json!({
                    "account": name,
                    "current": current,
                    "config": account,
                }),
                render_account_table(&name, account, current),
            )
        }
        AccountCommand::Validate { account } => {
            let store = AccountStore::load()?;
            let name = store.resolved_account_name(account.as_deref())?;
            let account = store.get_account(&name)?;
            let report = validate_account(&name, account);
            ok_output(
                format,
                "account.validate",
                json!({
                    "account": name,
                    "valid": report.valid,
                    "errors": report.errors,
                }),
                render_account_validation_table(&name, &report.errors),
            )
        }
        AccountCommand::Use { name } => {
            let mut store = AccountStore::load()?;
            store.set_current(&name)?;
            store.save()?;
            ok_output(
                format,
                "account.use",
                json!({
                    "account": name,
                    "current": true,
                }),
                render_key_value_table(&[
                    ("operation", "account.use".to_string()),
                    ("account", name),
                    ("current", "true".to_string()),
                ]),
            )
        }
        AccountCommand::Current => {
            let store = AccountStore::load()?;
            let name = store.current_account_name()?;
            let account = store.get_account(&name)?;
            ok_output(
                format,
                "account.current",
                json!({
                    "account": name,
                    "email": account.email,
                }),
                render_key_value_table(&[
                    ("operation", "account.current".to_string()),
                    ("account", name),
                    ("email", account.email.clone()),
                ]),
            )
        }
    }
}

fn execute_auth(format: OutputFormat, action: AuthCommand) -> Result<RenderedOutput> {
    match action {
        AuthCommand::Status { account } => {
            let account_store = AccountStore::load()?;
            let name = account_store.resolved_account_name(account.as_deref())?;
            let account = account_store.get_account(&name)?;
            let credential_store = CredentialStore::load()?;
            let services = BTreeMap::from([
                (
                    "mail",
                    auth_state(
                        &credential_store,
                        &name,
                        account.mail.credential_ref.as_deref(),
                        "mail",
                    ),
                ),
                (
                    "calendar",
                    auth_state(
                        &credential_store,
                        &name,
                        account.calendar.credential_ref.as_deref(),
                        "calendar",
                    ),
                ),
                (
                    "disk",
                    auth_state(
                        &credential_store,
                        &name,
                        account.disk.credential_ref.as_deref(),
                        "disk",
                    ),
                ),
            ]);

            let table = {
                let mut lines = vec![
                    format!("ACCOUNT\t{}", name),
                    format!("EMAIL\t{}", account.email),
                    "СЛУЖБА\tСТАТУС\tПОДРОБНОСТИ".to_string(),
                ];
                for (service, state) in &services {
                    lines.push(format!(
                        "{}\t{}\t{}",
                        service_label(service),
                        credential_state_label(state.credential_state),
                        state.detail
                    ));
                }
                lines.join("\n")
            };

            ok_output(
                format,
                "auth.status",
                json!({
                    "account": name,
                    "email": account.email,
                    "services": services,
                }),
                table,
            )
        }
        AuthCommand::Login {
            account,
            service,
            client_id,
            env_var,
            app_password,
            code,
            login_hint,
        } => {
            let mut account_store = AccountStore::load()?;
            let account_name = account_store.resolved_account_name(account.as_deref())?;
            let requested_service =
                if service.is_none() && (env_var.is_some() || app_password.is_some()) {
                    Some(AuthServiceArg::Calendar)
                } else {
                    service
                };
            match requested_service {
                Some(AuthServiceArg::Calendar) => {
                    let account = account_store.get_account(&account_name)?;
                    ensure_calendar_supports_app_password(account)?;
                    if client_id.is_some() || code.is_some() || login_hint.is_some() {
                        return Err(YacliError::Validation(
                            "calendar login uses app password auth; rerun with `yacli login calendar --app-password <app-password>` or `yacli login calendar --env-var NAME`".to_string(),
                        ));
                    }
                    if env_var.is_some() && app_password.is_some() {
                        return Err(YacliError::Validation(
                            "calendar login accepts either `--app-password` or `--env-var`, but not both".to_string(),
                        ));
                    }

                    let (credential_ref, mode) = match (app_password, env_var) {
                        (Some(app_password), None) => {
                            let mut credential_store = CredentialStore::load()?;
                            credential_store.set_app_password(
                                account_name.clone(),
                                "calendar".to_string(),
                                StoredAppPasswordCredential {
                                    kind: "app_password".to_string(),
                                    secret: app_password,
                                },
                            );
                            credential_store.save()?;
                            ("store:calendar".to_string(), "app_password_store")
                        }
                        (None, Some(env_var)) => (format!("env:{env_var}"), "app_password_env"),
                        (None, None) => {
                            return Err(YacliError::Validation(
                                "calendar login requires `--app-password <app-password>` or `--env-var NAME`".to_string(),
                            ));
                        }
                        (Some(_), Some(_)) => unreachable!(),
                    };

                    account_store.set_service_credential_ref(
                        &account_name,
                        "calendar",
                        Some(credential_ref.clone()),
                    )?;
                    account_store.save()?;

                    ok_output(
                        format,
                        "auth.login",
                        json!({
                            "account": account_name,
                            "service": "calendar",
                            "credential_ref": credential_ref,
                            "mode": mode,
                        }),
                        render_key_value_table(&[
                            ("operation", "auth.login".to_string()),
                            ("account", account_name),
                            ("service", "calendar".to_string()),
                            ("credential_ref", credential_ref),
                            ("mode", mode.to_string()),
                        ]),
                    )
                }
                Some(AuthServiceArg::Mail) | Some(AuthServiceArg::Disk) | None => {
                    let account = account_store.get_account(&account_name)?;
                    let services = oauth_login_services(account, requested_service)?;
                    if env_var.is_some() || app_password.is_some() {
                        let command_hint = match requested_service {
                            Some(AuthServiceArg::Mail) => {
                                "mail login uses built-in OAuth by default; run `yacli login mail`"
                            }
                            Some(AuthServiceArg::Disk) => {
                                "disk login uses built-in OAuth by default; run `yacli login disk`"
                            }
                            None => {
                                "plain `yacli login` connects Почту и Диск; calendar still requires `yacli login calendar --app-password <app-password>` or `--env-var`"
                            }
                            Some(AuthServiceArg::Calendar) => unreachable!(),
                        };
                        return Err(YacliError::Validation(command_hint.to_string()));
                    }
                    let client_id_source = if client_id.is_some() {
                        "override"
                    } else {
                        "built_in"
                    };
                    let client_id =
                        client_id.unwrap_or_else(|| default_yacli_client_id().to_string());
                    let resolved_login_hint =
                        login_hint.as_deref().or(Some(account.email.as_str()));
                    let session =
                        start_pkce_authorization(&services, &client_id, resolved_login_hint)?;
                    let code = match code {
                        Some(code) => code,
                        None => read_confirmation_code(&session.request.authorization_url)?,
                    };
                    let login = exchange_authorization_code(session, &code)?;

                    let mut credential_store = CredentialStore::load()?;
                    let credential_refs: Vec<_> = services
                        .iter()
                        .map(|service| {
                            credential_store.set_oauth(
                                account_name.clone(),
                                service.as_str().to_string(),
                                login.credential.clone(),
                            );
                            account_store.set_service_credential_ref(
                                &account_name,
                                service.as_str(),
                                Some(service.store_ref().to_string()),
                            )?;
                            Ok((
                                service.as_str().to_string(),
                                service.store_ref().to_string(),
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    credential_store.save()?;
                    account_store.save()?;

                    if services.len() == 1 {
                        let service = services[0];
                        let credential_ref = service.store_ref().to_string();
                        return ok_output(
                            format,
                            "auth.login",
                            json!({
                                "account": account_name,
                                "service": service.as_str(),
                                "credential_ref": credential_ref,
                                "client_id_source": client_id_source,
                                "authorization": login.authorization,
                                "token": {
                                    "token_type": login.credential.token_type,
                                    "expires_at_epoch_secs": login.credential.expires_at_epoch_secs,
                                    "scope": login.credential.scope,
                                }
                            }),
                            render_key_value_table(&[
                                ("operation", "auth.login".to_string()),
                                ("account", account_name),
                                ("service", service.as_str().to_string()),
                                ("credential_ref", credential_ref),
                                ("client_id_source", client_id_source.to_string()),
                                (
                                    "expires_at_epoch_secs",
                                    login.credential.expires_at_epoch_secs.to_string(),
                                ),
                            ]),
                        );
                    }

                    ok_output(
                        format,
                        "auth.login",
                        json!({
                            "account": account_name,
                            "services": services.iter().map(|service| service.as_str()).collect::<Vec<_>>(),
                            "credential_refs": credential_refs,
                            "client_id_source": client_id_source,
                            "authorization": login.authorization,
                            "token": {
                                "token_type": login.credential.token_type,
                                "expires_at_epoch_secs": login.credential.expires_at_epoch_secs,
                                "scope": login.credential.scope,
                            }
                        }),
                        render_key_value_table(&[
                            ("operation", "auth.login".to_string()),
                            ("account", account_name),
                            (
                                "services",
                                services
                                    .iter()
                                    .map(|service| service.as_str())
                                    .collect::<Vec<_>>()
                                    .join(","),
                            ),
                            (
                                "credential_refs",
                                credential_refs
                                    .iter()
                                    .map(|(service, credential_ref)| {
                                        format!("{service}={credential_ref}")
                                    })
                                    .collect::<Vec<_>>()
                                    .join(","),
                            ),
                            ("client_id_source", client_id_source.to_string()),
                            (
                                "expires_at_epoch_secs",
                                login.credential.expires_at_epoch_secs.to_string(),
                            ),
                        ]),
                    )
                }
            }
        }
        AuthCommand::Logout { account, service } => {
            let service = service.ok_or_else(|| {
                YacliError::Validation(
                    "logout without service is handled by the top-level `yacli logout`; internal `auth logout` still requires an explicit service".to_string(),
                )
            })?;
            let mut account_store = AccountStore::load()?;
            let account_name = account_store.resolved_account_name(account.as_deref())?;
            match service {
                AuthServiceArg::Calendar => {
                    let current_ref = account_store
                        .get_account(&account_name)?
                        .calendar
                        .credential_ref
                        .clone();
                    let mut credential_store = CredentialStore::load()?;
                    let removed = current_ref.as_deref() == Some("store:calendar")
                        && credential_store.remove_service(&account_name, "calendar");
                    if removed {
                        credential_store.save()?;
                    }
                    let cleared_account_ref = current_ref.is_some();
                    if cleared_account_ref {
                        account_store.set_service_credential_ref(
                            &account_name,
                            "calendar",
                            None,
                        )?;
                        account_store.save()?;
                    }

                    ok_output(
                        format,
                        "auth.logout",
                        json!({
                            "account": account_name,
                            "service": "calendar",
                            "removed": removed,
                            "cleared_account_ref": cleared_account_ref,
                        }),
                        render_key_value_table(&[
                            ("operation", "auth.logout".to_string()),
                            ("account", account_name),
                            ("service", "calendar".to_string()),
                            ("removed", removed.to_string()),
                            ("cleared_account_ref", cleared_account_ref.to_string()),
                        ]),
                    )
                }
                AuthServiceArg::Mail | AuthServiceArg::Disk => {
                    let service = oauth_service(service)?;
                    let current_ref =
                        service_credential_ref(account_store.get_account(&account_name)?, service)
                            .map(ToString::to_string);

                    let mut credential_store = CredentialStore::load()?;
                    let removed = credential_store.remove_service(&account_name, service.as_str());
                    credential_store.save()?;

                    let cleared_account_ref = current_ref.as_deref() == Some(service.store_ref());
                    if cleared_account_ref {
                        account_store.set_service_credential_ref(
                            &account_name,
                            service.as_str(),
                            None,
                        )?;
                        account_store.save()?;
                    }

                    ok_output(
                        format,
                        "auth.logout",
                        json!({
                            "account": account_name,
                            "service": service.as_str(),
                            "removed": removed,
                            "cleared_account_ref": cleared_account_ref,
                        }),
                        render_key_value_table(&[
                            ("operation", "auth.logout".to_string()),
                            ("account", account_name),
                            ("service", service.as_str().to_string()),
                            ("removed", removed.to_string()),
                            ("cleared_account_ref", cleared_account_ref.to_string()),
                        ]),
                    )
                }
            }
        }
    }
}

fn execute_disk(format: OutputFormat, action: DiskCommand) -> Result<RenderedOutput> {
    match action {
        DiskCommand::Public { action } => execute_disk_public(format, action),
        DiskCommand::Mkdir { account, path } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let resource = create_private_directory(
                &base_url,
                &access_token,
                &PrivateDiskMkdirRequest { path: path.clone() },
            )?;

            ok_output(
                format,
                "disk.mkdir",
                json!({
                    "account": resolved_account,
                    "path": path,
                    "resource": resource,
                }),
                render_disk_mkdir_table(&resolved_account, &resource),
            )
        }
        DiskCommand::Upload {
            account,
            source,
            path,
            overwrite,
        } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let (resource, uploaded) = upload_private_resource(
                &base_url,
                &access_token,
                &PrivateDiskUploadRequest {
                    source,
                    path: path.clone(),
                    overwrite,
                },
            )?;

            ok_output(
                format,
                "disk.upload",
                json!({
                    "account": resolved_account,
                    "path": path,
                    "resource": resource,
                    "upload": uploaded,
                }),
                render_disk_upload_table(&resolved_account, &resource, &uploaded),
            )
        }
        DiskCommand::List {
            account,
            path,
            limit,
            offset,
        } => {
            let path = path.unwrap_or_else(|| "disk:/".to_string());
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let resource = fetch_private_resource(
                &base_url,
                &access_token,
                &PrivateDiskListRequest {
                    path: path.clone(),
                    limit,
                    offset,
                },
            )?;

            ok_output(
                format,
                "disk.list",
                json!({
                    "account": resolved_account,
                    "path": path,
                    "limit": limit,
                    "offset": offset,
                    "resource": resource,
                }),
                render_disk_resource_table(&resolved_account, &resource),
            )
        }
        DiskCommand::Info { account } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let info = fetch_disk_info(&base_url, &access_token)?;

            ok_output(
                format,
                "disk.info",
                json!({
                    "account": resolved_account,
                    "disk": info,
                }),
                render_disk_info_table(&resolved_account, &info),
            )
        }
    }
}

fn execute_calendar(format: OutputFormat, action: CalendarCommand) -> Result<RenderedOutput> {
    match action {
        CalendarCommand::Calendars { account } => {
            let (resolved_account, app_password, context) =
                resolve_calendar_private_context(account.as_deref())?;
            let calendars =
                list_calendars(&context.caldav_base_url, &context.email, &app_password)?;

            ok_output(
                format,
                "calendar.calendars",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "caldav": {
                        "base_url": context.caldav_base_url,
                    },
                    "calendars": calendars,
                }),
                render_calendar_collections_table(&resolved_account, &calendars),
            )
        }
        CalendarCommand::Events {
            account,
            calendar,
            from,
            to,
            limit,
        } => {
            let window = parse_event_window(from.as_deref(), to.as_deref(), limit)?;
            let (resolved_account, app_password, context) =
                resolve_calendar_private_context(account.as_deref())?;
            let request = CalendarEventsRequest {
                calendar,
                from: chrono::DateTime::parse_from_rfc3339(&window.from)
                    .map_err(|err| {
                        YacliError::Serialization(format!(
                            "failed to rebuild calendar --from boundary: {err}"
                        ))
                    })?
                    .with_timezone(&chrono::Utc),
                to: chrono::DateTime::parse_from_rfc3339(&window.to)
                    .map_err(|err| {
                        YacliError::Serialization(format!(
                            "failed to rebuild calendar --to boundary: {err}"
                        ))
                    })?
                    .with_timezone(&chrono::Utc),
                limit: window.limit,
            };
            let (calendar, window, events) = list_calendar_events(
                &context.caldav_base_url,
                &context.email,
                &app_password,
                request,
            )?;

            ok_output(
                format,
                "calendar.events",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "calendar": calendar,
                    "window": window,
                    "events": events.iter().map(calendar_event_json).collect::<Vec<_>>(),
                }),
                render_calendar_events_table(&resolved_account, &calendar, &window, &events),
            )
        }
        CalendarCommand::Create {
            account,
            calendar,
            summary,
            start,
            end,
            description,
            location,
        } => {
            let (resolved_account, app_password, context) =
                resolve_calendar_private_context(account.as_deref())?;
            let (calendar, event) = create_calendar_event(
                &context.caldav_base_url,
                &context.email,
                &app_password,
                CalendarCreateRequest {
                    calendar,
                    summary,
                    start,
                    end,
                    description,
                    location,
                },
            )?;

            ok_output(
                format,
                "calendar.create",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "calendar": calendar,
                    "event": calendar_event_json(&event),
                }),
                render_calendar_create_table(&resolved_account, &calendar, &event),
            )
        }
        CalendarCommand::Delete {
            account,
            calendar,
            uid,
        } => {
            let (resolved_account, app_password, context) =
                resolve_calendar_private_context(account.as_deref())?;
            let (calendar, event) = delete_calendar_event(
                &context.caldav_base_url,
                &context.email,
                &app_password,
                &calendar,
                &uid,
            )?;

            ok_output(
                format,
                "calendar.delete",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "calendar": calendar,
                    "deleted_event": calendar_event_json(&event),
                }),
                render_calendar_delete_table(&resolved_account, &calendar, &event),
            )
        }
    }
}

fn execute_mail(format: OutputFormat, action: MailCommand) -> Result<RenderedOutput> {
    match action {
        MailCommand::Folders { account } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let folders = list_mail_folders(&context.imap_host, context.imap_port, auth)?;

            ok_output(
                format,
                "mail.folders",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "imap": {
                        "host": context.imap_host,
                        "port": context.imap_port,
                    },
                    "folders": folders,
                }),
                render_mail_folders_table(&resolved_account, &folders),
            )
        }
        MailCommand::List {
            account,
            folder,
            limit,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let messages =
                list_mail_messages(&context.imap_host, context.imap_port, auth, &folder, limit)?;

            ok_output(
                format,
                "mail.list",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "folder": folder,
                    "limit": limit,
                    "messages": messages.iter().map(mail_summary_json).collect::<Vec<_>>(),
                }),
                render_mail_list_table(&resolved_account, &folder, &messages),
            )
        }
        MailCommand::Search {
            account,
            folder,
            query,
            limit,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let messages = search_mail_messages(
                &context.imap_host,
                context.imap_port,
                auth,
                &folder,
                &query,
                limit,
            )?;

            ok_output(
                format,
                "mail.search",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "folder": folder,
                    "query": query,
                    "limit": limit,
                    "messages": messages.iter().map(mail_summary_json).collect::<Vec<_>>(),
                }),
                render_mail_search_table(&resolved_account, &folder, &query, &messages),
            )
        }
        MailCommand::Read {
            account,
            folder,
            uid,
            max_bytes,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let message = read_mail_message(
                &context.imap_host,
                context.imap_port,
                auth,
                &folder,
                uid,
                max_bytes,
            )?;

            ok_output(
                format,
                "mail.read",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "folder": folder,
                    "id": uid,
                    "message": mail_message_json(&message),
                }),
                render_mail_read_table(&resolved_account, &folder, &message),
            )
        }
        MailCommand::Send {
            account,
            to,
            cc,
            bcc,
            subject,
            body,
            html,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let sent = send_mail_message(
                &context.smtp_host,
                context.smtp_port,
                auth,
                MailSendRequest {
                    to: vec![to],
                    cc,
                    bcc,
                    subject,
                    text: body,
                    html,
                    attachments: Vec::new(),
                    thread_headers: None,
                },
            )?;

            ok_output(
                format,
                "mail.send",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "smtp": {
                        "host": context.smtp_host,
                        "port": context.smtp_port,
                    },
                    "sent": sent,
                }),
                render_mail_send_table(&resolved_account, &sent),
            )
        }
        MailCommand::Reply {
            account,
            folder,
            uid,
            body,
            cc,
            html,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let replied = reply_to_mail_message(
                &context.imap_host,
                context.imap_port,
                &context.smtp_host,
                context.smtp_port,
                auth,
                &folder,
                MailReplyRequest {
                    uid,
                    cc,
                    text: body,
                    html,
                },
            )?;

            ok_output(
                format,
                "mail.reply",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "folder": folder,
                    "reply": replied_mail_json(&replied),
                }),
                render_mail_reply_table(&resolved_account, &folder, &replied),
            )
        }
        MailCommand::Forward {
            account,
            folder,
            uid,
            to,
            body,
            cc,
            bcc,
            html,
            max_source_bytes,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let forwarded = forward_mail_message(
                &context.imap_host,
                context.imap_port,
                &context.smtp_host,
                context.smtp_port,
                auth,
                &folder,
                MailForwardRequest {
                    uid,
                    to: vec![to],
                    cc,
                    bcc,
                    text: body,
                    html,
                    max_source_bytes,
                },
            )?;

            ok_output(
                format,
                "mail.forward",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "folder": folder,
                    "forward": forwarded_mail_json(&forwarded),
                }),
                render_mail_forward_table(&resolved_account, &folder, &forwarded),
            )
        }
    }
}

fn execute_disk_public(format: OutputFormat, action: DiskPublicCommand) -> Result<RenderedOutput> {
    match action {
        DiskPublicCommand::Show {
            account,
            public_key,
            path,
        } => {
            let (resolved_account, base_url) = resolve_disk_public_context(account.as_deref())?;
            let resource = fetch_public_resource(
                &base_url,
                &PublicDiskRequest {
                    public_key: public_key.clone(),
                    path: path.clone(),
                },
            )?;

            ok_output(
                format,
                "disk.public.show",
                json!({
                    "account": resolved_account,
                    "public_key": public_key,
                    "path": path,
                    "resource": resource,
                }),
                render_public_resource_table(
                    resolved_account.as_deref(),
                    &public_key,
                    path.as_deref(),
                    &resource,
                ),
            )
        }
        DiskPublicCommand::Download {
            account,
            public_key,
            path,
            output,
            force,
        } => {
            let (resolved_account, base_url) = resolve_disk_public_context(account.as_deref())?;
            let (resource, artifact) = download_public_resource(
                &base_url,
                &PublicDownloadRequest {
                    public_key: public_key.clone(),
                    path: path.clone(),
                    output,
                    force,
                },
            )?;

            ok_output(
                format,
                "disk.public.download",
                json!({
                    "account": resolved_account,
                    "public_key": public_key,
                    "path": path,
                    "resource": resource,
                    "download": artifact,
                }),
                render_public_download_table(
                    resolved_account.as_deref(),
                    &public_key,
                    path.as_deref(),
                    &resource,
                    &artifact,
                ),
            )
        }
    }
}

fn ok_output(
    format: OutputFormat,
    operation: &'static str,
    payload: serde_json::Value,
    table: String,
) -> Result<RenderedOutput> {
    let Some(mut object) = payload.as_object().cloned() else {
        return Err(YacliError::Serialization(
            "expected object payload".to_string(),
        ));
    };
    object.insert("ok".to_string(), json!(true));
    object.insert("operation".to_string(), json!(operation));

    Ok(RenderedOutput {
        format,
        json: serde_json::Value::Object(object),
        table,
    })
}

#[derive(Serialize)]
struct CredentialState {
    credential_ref: Option<String>,
    credential_state: &'static str,
    detail: String,
}

#[derive(Clone, Copy)]
enum CredentialReference<'a> {
    Env(&'a str),
    Store(&'a str),
}

struct MailConnectionContext {
    email: String,
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
}

struct CalendarConnectionContext {
    email: String,
    caldav_base_url: String,
}

#[derive(Serialize)]
struct GuideCommandEntry {
    path: &'static str,
    topic: &'static str,
    summary: &'static str,
    requires_account: bool,
    examples: Vec<&'static str>,
}

#[derive(Serialize)]
struct GuideWorkflowEntry {
    id: &'static str,
    topic: &'static str,
    title: &'static str,
    summary: &'static str,
    steps: Vec<&'static str>,
}

fn auth_state(
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

fn parse_credential_ref(raw: &str) -> Option<CredentialReference<'_>> {
    if let Some(value) = raw.strip_prefix("env:") {
        return Some(CredentialReference::Env(value));
    }
    if let Some(value) = raw.strip_prefix("store:") {
        return Some(CredentialReference::Store(value));
    }
    None
}

fn service_label(service: &str) -> String {
    match service {
        "mail" => "Почта".to_string(),
        "calendar" => "Календарь".to_string(),
        "disk" => "Диск".to_string(),
        _ => service.to_string(),
    }
}

fn credential_state_label(state: &str) -> String {
    match state {
        "not_configured" => "Не подключено".to_string(),
        "env_present" | "store_present" => "Подключено".to_string(),
        "env_missing" | "store_missing" => "Не найдено".to_string(),
        "store_expired" => "Нужен вход".to_string(),
        "store_mismatch" | "unsupported_reference" => "Ошибка настройки".to_string(),
        _ => state.to_string(),
    }
}

fn guide_topic_name(topic: GuideTopicArg) -> &'static str {
    match topic {
        GuideTopicArg::All => "all",
        GuideTopicArg::Account => "account",
        GuideTopicArg::Auth => "auth",
        GuideTopicArg::Mail => "mail",
        GuideTopicArg::Calendar => "calendar",
        GuideTopicArg::Disk => "disk",
    }
}

fn guide_commands(topic: GuideTopicArg) -> Vec<GuideCommandEntry> {
    all_guide_commands()
        .into_iter()
        .filter(|entry| topic == GuideTopicArg::All || guide_topic_name(topic) == entry.topic)
        .collect()
}

fn guide_workflows(topic: GuideTopicArg) -> Vec<GuideWorkflowEntry> {
    all_guide_workflows()
        .into_iter()
        .filter(|entry| topic == GuideTopicArg::All || guide_topic_name(topic) == entry.topic)
        .collect()
}

fn all_guide_commands() -> Vec<GuideCommandEntry> {
    vec![
        GuideCommandEntry {
            path: "add",
            topic: "account",
            summary: "Добавить аккаунт Яндекса и сразу сделать его текущим.",
            requires_account: false,
            examples: vec!["yacli add me@yandex.ru"],
        },
        GuideCommandEntry {
            path: "accounts",
            topic: "account",
            summary: "Показать все настроенные аккаунты.",
            requires_account: false,
            examples: vec!["yacli accounts"],
        },
        GuideCommandEntry {
            path: "use",
            topic: "account",
            summary: "Сделать аккаунт текущим для последующих команд.",
            requires_account: false,
            examples: vec!["yacli use work"],
        },
        GuideCommandEntry {
            path: "whoami",
            topic: "account",
            summary: "Показать текущий активный аккаунт.",
            requires_account: false,
            examples: vec!["yacli whoami"],
        },
        GuideCommandEntry {
            path: "status",
            topic: "auth",
            summary: "Показать, что подключено у текущего аккаунта.",
            requires_account: true,
            examples: vec!["yacli status"],
        },
        GuideCommandEntry {
            path: "login",
            topic: "auth",
            summary: "Подключить Почту и Диск одной OAuth-командой через встроенное приложение yacli.",
            requires_account: true,
            examples: vec!["yacli login", "yacli login mail", "yacli login disk"],
        },
        GuideCommandEntry {
            path: "logout",
            topic: "auth",
            summary: "Отключить все сервисы у текущего аккаунта или только один выбранный сервис.",
            requires_account: true,
            examples: vec!["yacli logout", "yacli logout mail", "yacli logout calendar"],
        },
        GuideCommandEntry {
            path: "login calendar",
            topic: "auth",
            summary: "Подключить Календарь через пароль приложения Яндекс ID или env-переменную.",
            requires_account: true,
            examples: vec!["yacli login calendar --app-password <app-password>"],
        },
        GuideCommandEntry {
            path: "mail folders",
            topic: "mail",
            summary: "Показать доступные папки в почтовом ящике.",
            requires_account: true,
            examples: vec!["yacli mail folders"],
        },
        GuideCommandEntry {
            path: "mail list",
            topic: "mail",
            summary: "Показать список писем в папке. По умолчанию это INBOX.",
            requires_account: true,
            examples: vec!["yacli mail list --limit 10"],
        },
        GuideCommandEntry {
            path: "mail search",
            topic: "mail",
            summary: "Найти письма по тексту. По умолчанию поиск идет в INBOX.",
            requires_account: true,
            examples: vec!["yacli mail search \"смета\""],
        },
        GuideCommandEntry {
            path: "mail reply",
            topic: "mail",
            summary: "Ответить на письмо по ID из `mail list` или `mail search`.",
            requires_account: true,
            examples: vec!["yacli mail reply 1353 \"Принято\""],
        },
        GuideCommandEntry {
            path: "mail forward",
            topic: "mail",
            summary: "Переслать письмо по ID из `mail list` или `mail search`.",
            requires_account: true,
            examples: vec!["yacli mail forward 1353 person@example.com \"FYI\""],
        },
        GuideCommandEntry {
            path: "mail read",
            topic: "mail",
            summary: "Открыть письмо по ID из `mail list` или `mail search`.",
            requires_account: true,
            examples: vec!["yacli mail read 1353"],
        },
        GuideCommandEntry {
            path: "mail send",
            topic: "mail",
            summary: "Отправить письмо через SMTP с OAuth XOAUTH2 или app password.",
            requires_account: true,
            examples: vec!["yacli mail send person@example.com \"Синк\" \"Привет\""],
        },
        GuideCommandEntry {
            path: "calendar calendars",
            topic: "calendar",
            summary: "Показать доступные календари через CalDAV.",
            requires_account: true,
            examples: vec!["yacli calendar calendars"],
        },
        GuideCommandEntry {
            path: "calendar events",
            topic: "calendar",
            summary: "Показать события в окне дат. По умолчанию используется календарь default и ближайшие 30 дней.",
            requires_account: true,
            examples: vec![
                "yacli calendar events",
                "yacli calendar events 2026-03-12 2026-03-19 --limit 20",
            ],
        },
        GuideCommandEntry {
            path: "calendar create",
            topic: "calendar",
            summary: "Создать событие в выбранном календаре через CalDAV PUT.",
            requires_account: true,
            examples: vec![
                "yacli calendar create \"Синк\" 2026-03-12T09:00:00Z 2026-03-12T10:00:00Z",
            ],
        },
        GuideCommandEntry {
            path: "calendar delete",
            topic: "calendar",
            summary: "Удалить событие по ID из выбранного календаря.",
            requires_account: true,
            examples: vec!["yacli calendar delete <id>"],
        },
        GuideCommandEntry {
            path: "disk list",
            topic: "disk",
            summary: "Показать содержимое папки или метаданные ресурса в приватном Яндекс Диске. По умолчанию это root `disk:/`.",
            requires_account: true,
            examples: vec!["yacli disk list", "yacli disk list disk:/docs --limit 50"],
        },
        GuideCommandEntry {
            path: "disk mkdir",
            topic: "disk",
            summary: "Создать папку в приватном Яндекс Диске через REST API.",
            requires_account: true,
            examples: vec!["yacli disk mkdir disk:/docs/archive"],
        },
        GuideCommandEntry {
            path: "disk upload",
            topic: "disk",
            summary: "Загрузить локальный файл в приватный Яндекс Диск.",
            requires_account: true,
            examples: vec!["yacli disk upload ./report.pdf disk:/docs/report.pdf"],
        },
        GuideCommandEntry {
            path: "disk info",
            topic: "disk",
            summary: "Показать приватную информацию о Диске по сохраненному токену.",
            requires_account: true,
            examples: vec!["yacli disk info"],
        },
        GuideCommandEntry {
            path: "disk public show",
            topic: "disk",
            summary: "Показать метаданные публичного файла или папки на Яндекс Диске.",
            requires_account: false,
            examples: vec![
                "yacli disk public show --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA",
            ],
        },
        GuideCommandEntry {
            path: "disk public download",
            topic: "disk",
            summary: "Скачать публичный файл с Яндекс Диска в локальный путь.",
            requires_account: false,
            examples: vec![
                "yacli disk public download --public-key https://disk.yandex.ru/i/WhGpLnQWR9efCA --output ./sample.pdf",
            ],
        },
    ]
}

fn all_guide_workflows() -> Vec<GuideWorkflowEntry> {
    vec![
        GuideWorkflowEntry {
            id: "mail_read_flow",
            topic: "mail",
            title: "Прочитать письмо из Яндекс Почты",
            summary: "Полный поток от добавления аккаунта и логина до чтения письма по ID из списка.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail list --limit 10",
                "yacli mail read <id>",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_search_flow",
            topic: "mail",
            title: "Найти письмо по тексту",
            summary: "Поток от OAuth логина до поиска письма и открытия результата по ID.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail search \"смета\"",
                "yacli mail read <id>",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_reply_flow",
            topic: "mail",
            title: "Ответить на письмо",
            summary: "Поток от поиска письма до отправки ответа в тот же thread.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail search \"смета\"",
                "yacli mail reply <id> \"Принято\"",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_forward_flow",
            topic: "mail",
            title: "Переслать письмо",
            summary: "Поток от поиска письма до пересылки новому получателю вместе с вложениями.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail search \"смета\"",
                "yacli mail forward <id> person@example.com \"FYI\"",
            ],
        },
        GuideWorkflowEntry {
            id: "disk_public_flow",
            topic: "disk",
            title: "Посмотреть и скачать публичный файл с Диска",
            summary: "Read-only поток без локального аккаунта.",
            steps: vec![
                "yacli disk public show --public-key <public-url-or-key>",
                "yacli disk public download --public-key <public-url-or-key> --output ./file.bin",
            ],
        },
        GuideWorkflowEntry {
            id: "disk_browse_flow",
            topic: "disk",
            title: "Просмотреть приватный Яндекс Диск",
            summary: "Поток добавления аккаунта, OAuth-логина и просмотра содержимого папки.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli disk list",
            ],
        },
        GuideWorkflowEntry {
            id: "disk_write_flow",
            topic: "disk",
            title: "Создать папку и загрузить файл на Диск",
            summary: "Поток от OAuth-логина до создания директории и загрузки локального файла.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli disk mkdir disk:/docs/archive",
                "yacli disk upload ./report.pdf disk:/docs/archive/report.pdf",
                "yacli disk list disk:/docs/archive --limit 50",
            ],
        },
        GuideWorkflowEntry {
            id: "disk_private_flow",
            topic: "disk",
            title: "Подключить приватный Яндекс Диск",
            summary: "Поток добавления аккаунта и OAuth для приватного Диска.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli disk info",
                "yacli disk list",
                "yacli disk mkdir disk:/docs/archive",
                "yacli disk upload ./report.pdf disk:/docs/archive/report.pdf",
            ],
        },
        GuideWorkflowEntry {
            id: "multi_account_mail_flow",
            topic: "mail",
            title: "Работать с несколькими почтовыми аккаунтами",
            summary: "Независимый доступ к нескольким ящикам через именованные аккаунты и `use`.",
            steps: vec![
                "yacli add personal@yandex.ru",
                "yacli add work@company.ru",
                "yacli login",
                "yacli use work",
                "yacli login",
                "yacli use personal",
                "yacli mail list --limit 5",
                "yacli use work",
                "yacli mail list --limit 5",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_send_flow",
            topic: "mail",
            title: "Отправить письмо",
            summary: "Поток от OAuth логина до отправки письма через SMTP.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail send person@example.com \"Синк\" \"Привет\"",
            ],
        },
        GuideWorkflowEntry {
            id: "calendar_read_flow",
            topic: "calendar",
            title: "Посмотреть календари и события",
            summary: "Поток от сохранения app password до чтения событий через CalDAV.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login calendar --app-password <app-password>",
                "yacli calendar calendars",
                "yacli calendar events",
            ],
        },
        GuideWorkflowEntry {
            id: "calendar_write_flow",
            topic: "calendar",
            title: "Создать и удалить событие",
            summary: "Поток от логина до create/delete календарного события.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login calendar --app-password <app-password>",
                "yacli calendar create \"Синк\" 2026-03-12T09:00:00Z 2026-03-12T10:00:00Z",
                "yacli calendar delete <id>",
            ],
        },
    ]
}

fn service_credential_ref(account: &AccountConfig, service: OauthService) -> Option<&str> {
    match service {
        OauthService::Mail => account.mail.credential_ref.as_deref(),
        OauthService::Disk => account.disk.credential_ref.as_deref(),
    }
}

fn ensure_service_supports_oauth(account: &AccountConfig, service: OauthService) -> Result<()> {
    match service {
        OauthService::Mail if !matches!(account.mail.auth_mode, MailAuthMode::OauthXoauth2) => {
            Err(YacliError::UnsupportedOperation(
                "mail OAuth login requires account.mail.auth_mode=oauth_xoauth2".to_string(),
            ))
        }
        _ => Ok(()),
    }
}

fn oauth_login_services(
    account: &AccountConfig,
    requested: Option<AuthServiceArg>,
) -> Result<Vec<OauthService>> {
    let services = match requested {
        Some(AuthServiceArg::Mail) => vec![OauthService::Mail],
        Some(AuthServiceArg::Disk) => vec![OauthService::Disk],
        Some(AuthServiceArg::Calendar) => {
            return Err(YacliError::UnsupportedOperation(
                "calendar does not use OAuth login in the stable surface; use `yacli login calendar --app-password <app-password>`".to_string(),
            ));
        }
        None => {
            let mut services = Vec::new();
            if matches!(account.mail.auth_mode, MailAuthMode::OauthXoauth2) {
                services.push(OauthService::Mail);
            }
            services.push(OauthService::Disk);
            services
        }
    };

    for service in &services {
        ensure_service_supports_oauth(account, *service)?;
    }

    Ok(services)
}

fn ensure_calendar_supports_app_password(account: &AccountConfig) -> Result<()> {
    if !matches!(account.calendar.auth_mode, CalendarAuthMode::AppPassword) {
        return Err(YacliError::UnsupportedOperation(
            "calendar login currently supports only account.calendar.auth_mode=app_password"
                .to_string(),
        ));
    }

    Ok(())
}

fn oauth_service(value: AuthServiceArg) -> Result<OauthService> {
    match value {
        AuthServiceArg::Mail => Ok(OauthService::Mail),
        AuthServiceArg::Disk => Ok(OauthService::Disk),
        AuthServiceArg::Calendar => Err(YacliError::UnsupportedOperation(
            "calendar does not use OAuth login in the stable surface; use `yacli login calendar --app-password <app-password>`"
                .to_string(),
        )),
    }
}

fn derive_account_name_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    let mut derived = String::with_capacity(local.len());
    let mut last_was_dash = false;

    for character in local.chars() {
        let normalized = match character {
            'a'..='z' | '0'..='9' => Some(character),
            'A'..='Z' => Some(character.to_ascii_lowercase()),
            '.' | '_' | '-' => Some(character),
            _ => Some('-'),
        };

        if let Some(value) = normalized {
            if value == '-' {
                if !last_was_dash {
                    derived.push(value);
                }
                last_was_dash = true;
            } else {
                derived.push(value);
                last_was_dash = false;
            }
        }
    }

    let trimmed = derived.trim_matches('-');
    if trimmed.is_empty() {
        "account".to_string()
    } else {
        trimmed.to_string()
    }
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| {
        YacliError::Config(format!("required environment variable is missing: {name}"))
    })
}

fn resolve_disk_private_context(account: Option<&str>) -> Result<(String, String, String)> {
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

fn resolve_mail_private_context(
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

fn resolve_calendar_private_context(
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

fn resolve_oauth_access_token(
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

fn resolve_app_password_secret(
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

fn read_confirmation_code(authorization_url: &str) -> Result<String> {
    eprintln!(
        "Откройте ссылку в браузере, разрешите доступ и вставьте код подтверждения:\n{}",
        authorization_url
    );
    eprint!("Код подтверждения: ");
    io::stderr().flush()?;

    let mut code = String::new();
    io::stdin().read_line(&mut code)?;
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err(YacliError::Auth(
            "для завершения входа нужен код подтверждения".to_string(),
        ));
    }

    Ok(code)
}

fn render_key_value_table(items: &[(&str, String)]) -> String {
    items
        .iter()
        .map(|(key, value)| format!("{key}\t{value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_guide_table(
    topic: &str,
    commands: &[GuideCommandEntry],
    workflows: &[GuideWorkflowEntry],
) -> String {
    let mut lines = vec![
        format!("topic\t{topic}"),
        format!("version\t{}", env!("CARGO_PKG_VERSION")),
        "commands".to_string(),
        "PATH\tTOPIC\tREQUIRES_ACCOUNT\tSUMMARY".to_string(),
    ];
    lines.extend(commands.iter().map(|command| {
        format!(
            "{}\t{}\t{}\t{}",
            command.path, command.topic, command.requires_account, command.summary
        )
    }));

    lines.push("workflows".to_string());
    lines.push("ID\tTOPIC\tTITLE\tSUMMARY".to_string());
    lines.extend(workflows.iter().map(|workflow| {
        format!(
            "{}\t{}\t{}\t{}",
            workflow.id, workflow.topic, workflow.title, workflow.summary
        )
    }));

    lines.join("\n")
}

fn render_account_table(name: &str, account: &AccountConfig, current: bool) -> String {
    render_key_value_table(&[
        ("account", name.to_string()),
        ("email", account.email.clone()),
        ("current", current.to_string()),
        (
            "mail.auth_mode",
            format!("{:?}", account.mail.auth_mode).to_lowercase(),
        ),
        (
            "mail.imap",
            format!("{}:{}", account.mail.imap_host, account.mail.imap_port),
        ),
        (
            "mail.smtp",
            format!("{}:{}", account.mail.smtp_host, account.mail.smtp_port),
        ),
        (
            "calendar.auth_mode",
            format!("{:?}", account.calendar.auth_mode).to_lowercase(),
        ),
        ("calendar.caldav", account.calendar.caldav_base_url.clone()),
        (
            "disk.auth_mode",
            format!("{:?}", account.disk.auth_mode).to_lowercase(),
        ),
        ("disk.rest", account.disk.rest_base_url.clone()),
    ])
}

fn render_account_validation_table(name: &str, errors: &[String]) -> String {
    if errors.is_empty() {
        return render_key_value_table(&[
            ("account", name.to_string()),
            ("valid", "true".to_string()),
        ]);
    }

    let mut lines = vec![
        format!("account\t{name}"),
        "valid\tfalse".to_string(),
        "errors".to_string(),
    ];
    lines.extend(errors.iter().map(|error| format!("- {error}")));
    lines.join("\n")
}

fn resolve_disk_public_context(account: Option<&str>) -> Result<(Option<String>, String)> {
    let store = AccountStore::load()?;

    match account {
        Some(requested) => {
            let name = store.resolved_account_name(Some(requested))?;
            let account = store.get_account(&name)?;
            Ok((Some(name), account.disk.rest_base_url.clone()))
        }
        None => match store.resolved_account_name(None) {
            Ok(name) => {
                let account = store.get_account(&name)?;
                Ok((Some(name), account.disk.rest_base_url.clone()))
            }
            Err(YacliError::CurrentAccountMissing) => Ok((None, DEFAULT_DISK_BASE_URL.to_string())),
            Err(err) => Err(err),
        },
    }
}

fn render_public_resource_table(
    account: Option<&str>,
    public_key: &str,
    path: Option<&str>,
    resource: &PublicResource,
) -> String {
    let mut lines = vec![
        format!("account\t{}", account.unwrap_or("-")),
        format!("public_key\t{public_key}"),
        format!("path\t{}", path.unwrap_or("-")),
        format!("name\t{}", resource.name),
        format!("resource_type\t{}", resource.resource_type),
        format!(
            "mime_type\t{}",
            resource.mime_type.as_deref().unwrap_or("-")
        ),
        format!(
            "size\t{}",
            resource
                .size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        format!(
            "public_url\t{}",
            resource.public_url.as_deref().unwrap_or("-")
        ),
        format!(
            "download_url\t{}",
            resource.download_url.as_deref().unwrap_or("-")
        ),
    ];

    if let Some(children) = &resource.children {
        lines.push(format!("children.total\t{}", children.total));
        lines.push("children".to_string());
        lines.push("TYPE\tNAME\tPATH\tSIZE".to_string());
        lines.extend(children.items.iter().map(|item| {
            format!(
                "{}\t{}\t{}\t{}",
                item.resource_type,
                item.name,
                item.path.as_deref().unwrap_or("-"),
                item.size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string())
            )
        }));
    }

    lines.join("\n")
}

fn render_public_download_table(
    account: Option<&str>,
    public_key: &str,
    path: Option<&str>,
    resource: &PublicResource,
    artifact: &DownloadedFile,
) -> String {
    render_key_value_table(&[
        ("account", account.unwrap_or("-").to_string()),
        ("public_key", public_key.to_string()),
        ("path", path.unwrap_or("-").to_string()),
        ("resource.name", resource.name.clone()),
        ("resource_type", resource.resource_type.clone()),
        ("output.path", artifact.output_path.clone()),
        ("bytes_written", artifact.bytes_written.to_string()),
        ("sha256", artifact.sha256.clone()),
    ])
}

fn render_disk_mkdir_table(account: &str, resource: &DiskResource) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("resource.path", resource.path.clone()),
        ("resource.name", resource.name.clone()),
        ("resource_type", resource.resource_type.clone()),
        (
            "children.total",
            resource
                .children
                .as_ref()
                .map(|children| children.total.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "revision",
            resource
                .revision
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_disk_upload_table(
    account: &str,
    resource: &DiskResource,
    uploaded: &UploadedFile,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("source.path", uploaded.source_path.clone()),
        ("resource.path", resource.path.clone()),
        ("resource.name", resource.name.clone()),
        ("resource_type", resource.resource_type.clone()),
        (
            "resource.size",
            resource
                .size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("bytes_written", uploaded.bytes_written.to_string()),
        ("sha256", uploaded.sha256.clone()),
        ("overwrite", uploaded.overwrite.to_string()),
    ])
}

fn render_mail_folders_table(account: &str, folders: &[MailFolder]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("count\t{}", folders.len()),
        "NAME\tDELIMITER\tATTRIBUTES\tRAW_NAME".to_string(),
    ];
    lines.extend(folders.iter().map(|folder| {
        format!(
            "{}\t{}\t{}\t{}",
            sanitize_table_cell(&folder.name),
            folder
                .delimiter
                .as_deref()
                .map(sanitize_table_cell)
                .unwrap_or_else(|| "-".to_string()),
            if folder.attributes.is_empty() {
                "-".to_string()
            } else {
                sanitize_table_cell(&folder.attributes.join(","))
            },
            sanitize_table_cell(&folder.raw_name)
        )
    }));
    lines.join("\n")
}

fn render_mail_list_table(account: &str, folder: &str, messages: &[MailMessageSummary]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("folder\t{folder}"),
        format!("count\t{}", messages.len()),
        "ID\tDATE\tFROM\tSUBJECT\tFLAGS\tSIZE".to_string(),
    ];
    lines.extend(messages.iter().map(|message| {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            message.uid,
            message
                .date
                .as_deref()
                .map(sanitize_table_cell)
                .unwrap_or_else(|| "-".to_string()),
            message
                .from
                .as_deref()
                .map(sanitize_table_cell)
                .unwrap_or_else(|| "-".to_string()),
            message
                .subject
                .as_deref()
                .map(sanitize_table_cell)
                .unwrap_or_else(|| "-".to_string()),
            if message.flags.is_empty() {
                "-".to_string()
            } else {
                sanitize_table_cell(&message.flags.join(","))
            },
            message
                .size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        )
    }));
    lines.join("\n")
}

fn render_mail_search_table(
    account: &str,
    folder: &str,
    query: &str,
    messages: &[MailMessageSummary],
) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("folder\t{folder}"),
        format!("query\t{query}"),
        format!("count\t{}", messages.len()),
        "ID\tDATE\tFROM\tSUBJECT\tFLAGS\tSIZE".to_string(),
    ];
    lines.extend(messages.iter().map(|message| {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            message.uid,
            message
                .date
                .as_deref()
                .map(sanitize_table_cell)
                .unwrap_or_else(|| "-".to_string()),
            message
                .from
                .as_deref()
                .map(sanitize_table_cell)
                .unwrap_or_else(|| "-".to_string()),
            message
                .subject
                .as_deref()
                .map(sanitize_table_cell)
                .unwrap_or_else(|| "-".to_string()),
            if message.flags.is_empty() {
                "-".to_string()
            } else {
                sanitize_table_cell(&message.flags.join(","))
            },
            message
                .size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        )
    }));
    lines.join("\n")
}

fn sanitize_table_cell(value: &str) -> String {
    let normalized = value.replace(['\r', '\n', '\t'], " ");
    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "-".to_string()
    } else {
        collapsed
    }
}

fn render_mail_read_table(account: &str, folder: &str, message: &MailMessage) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("folder\t{folder}"),
        format!("id\t{}", message.uid),
        format!("date\t{}", message.date.as_deref().unwrap_or("-")),
        format!("from\t{}", message.from.as_deref().unwrap_or("-")),
        format!("to\t{}", message.to.as_deref().unwrap_or("-")),
        format!("cc\t{}", message.cc.as_deref().unwrap_or("-")),
        format!("subject\t{}", message.subject.as_deref().unwrap_or("-")),
        format!(
            "message_id\t{}",
            message.message_id.as_deref().unwrap_or("-")
        ),
        format!(
            "flags\t{}",
            if message.flags.is_empty() {
                "-".to_string()
            } else {
                message.flags.join(",")
            }
        ),
        format!(
            "size\t{}",
            message
                .size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        "text_body".to_string(),
        message
            .text_body
            .as_deref()
            .unwrap_or("-")
            .trim_end()
            .to_string(),
        "html_body".to_string(),
        message
            .html_body
            .as_deref()
            .unwrap_or("-")
            .trim_end()
            .to_string(),
    ];

    if message.attachments.is_empty() {
        lines.push("attachments\t-".to_string());
    } else {
        lines.push("attachments".to_string());
        lines.push("FILENAME\tMIME\tINLINE\tCONTENT_ID".to_string());
        lines.extend(message.attachments.iter().map(|attachment| {
            format!(
                "{}\t{}\t{}\t{}",
                attachment.filename.as_deref().unwrap_or("-"),
                attachment.mime_type,
                attachment.inline,
                attachment.content_id.as_deref().unwrap_or("-")
            )
        }));
    }

    lines.join("\n")
}

fn render_mail_send_table(account: &str, sent: &SentMail) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("from", sent.from.clone()),
        ("to", sent.to.join(", ")),
        (
            "cc",
            if sent.cc.is_empty() {
                "-".to_string()
            } else {
                sent.cc.join(", ")
            },
        ),
        ("bcc_count", sent.bcc_count.to_string()),
        ("subject", sent.subject.clone()),
        ("message_id", sent.message_id.clone()),
        ("body_kind", sent.body_kind.clone()),
    ])
}

fn render_mail_reply_table(account: &str, folder: &str, replied: &RepliedMail) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("folder", folder.to_string()),
        ("original_id", replied.original_uid.to_string()),
        ("recipient", replied.recipient.clone()),
        ("original_subject", replied.original_subject.clone()),
        ("original_message_id", replied.original_message_id.clone()),
        ("reply_subject", replied.sent.subject.clone()),
        ("reply_message_id", replied.sent.message_id.clone()),
        ("body_kind", replied.sent.body_kind.clone()),
    ])
}

fn render_mail_forward_table(account: &str, folder: &str, forwarded: &ForwardedMail) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("folder", folder.to_string()),
        ("original_id", forwarded.original_uid.to_string()),
        (
            "original_subject",
            forwarded
                .original_subject
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "original_message_id",
            forwarded
                .original_message_id
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        ("attachment_count", forwarded.attachments.len().to_string()),
        ("forward_subject", forwarded.sent.subject.clone()),
        ("to", forwarded.sent.to.join(", ")),
        (
            "cc",
            if forwarded.sent.cc.is_empty() {
                "-".to_string()
            } else {
                forwarded.sent.cc.join(", ")
            },
        ),
        ("bcc_count", forwarded.sent.bcc_count.to_string()),
        ("forward_message_id", forwarded.sent.message_id.clone()),
        ("body_kind", forwarded.sent.body_kind.clone()),
    ])
}

fn mail_summary_json(message: &MailMessageSummary) -> serde_json::Value {
    json!({
        "id": message.uid,
        "subject": message.subject,
        "from": message.from,
        "date": message.date,
        "flags": message.flags,
        "size": message.size,
    })
}

fn mail_attachment_json(attachment: &MailAttachmentSummary) -> serde_json::Value {
    json!({
        "filename": attachment.filename,
        "mime_type": attachment.mime_type,
        "content_id": attachment.content_id,
        "inline": attachment.inline,
    })
}

fn mail_message_json(message: &MailMessage) -> serde_json::Value {
    json!({
        "id": message.uid,
        "subject": message.subject,
        "from": message.from,
        "to": message.to,
        "cc": message.cc,
        "date": message.date,
        "message_id": message.message_id,
        "flags": message.flags,
        "size": message.size,
        "text_body": message.text_body,
        "html_body": message.html_body,
        "attachments": message.attachments.iter().map(mail_attachment_json).collect::<Vec<_>>(),
    })
}

fn sent_mail_json(sent: &SentMail) -> serde_json::Value {
    json!({
        "from": sent.from,
        "to": sent.to,
        "cc": sent.cc,
        "bcc_count": sent.bcc_count,
        "subject": sent.subject,
        "message_id": sent.message_id,
        "body_kind": sent.body_kind,
    })
}

fn replied_mail_json(replied: &RepliedMail) -> serde_json::Value {
    json!({
        "original_id": replied.original_uid,
        "recipient": replied.recipient,
        "original_subject": replied.original_subject,
        "original_message_id": replied.original_message_id,
        "sent": sent_mail_json(&replied.sent),
    })
}

fn forwarded_mail_json(forwarded: &ForwardedMail) -> serde_json::Value {
    json!({
        "original_id": forwarded.original_uid,
        "original_subject": forwarded.original_subject,
        "original_message_id": forwarded.original_message_id,
        "attachments": forwarded.attachments.iter().map(mail_attachment_json).collect::<Vec<_>>(),
        "sent": sent_mail_json(&forwarded.sent),
    })
}

fn calendar_event_json(event: &CalendarEvent) -> serde_json::Value {
    json!({
        "calendar_id": event.calendar_id,
        "calendar_name": event.calendar_name,
        "href": event.href,
        "id": event.uid,
        "summary": event.summary,
        "start": event.start,
        "end": event.end,
        "description": event.description,
        "location": event.location,
        "status": event.status,
        "etag": event.etag,
        "all_day": event.all_day,
    })
}

fn render_calendar_collections_table(account: &str, calendars: &[CalendarCollection]) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("count\t{}", calendars.len()),
        "ID\tNAME\tHREF\tDESCRIPTION".to_string(),
    ];
    lines.extend(calendars.iter().map(|calendar| {
        format!(
            "{}\t{}\t{}\t{}",
            calendar.id,
            calendar.name,
            calendar.href,
            calendar.description.as_deref().unwrap_or("-")
        )
    }));
    lines.join("\n")
}

fn render_calendar_events_table(
    account: &str,
    calendar: &CalendarCollection,
    window: &CalendarEventWindow,
    events: &[CalendarEvent],
) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("calendar.id\t{}", calendar.id),
        format!("calendar.name\t{}", calendar.name),
        format!("window.from\t{}", window.from),
        format!("window.to\t{}", window.to),
        format!("count\t{}", events.len()),
        "ID\tSTART\tEND\tSUMMARY\tLOCATION\tSTATUS\tALL_DAY".to_string(),
    ];
    lines.extend(events.iter().map(|event| {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            event.uid.as_deref().unwrap_or("-"),
            event.start.as_deref().unwrap_or("-"),
            event.end.as_deref().unwrap_or("-"),
            event.summary.as_deref().unwrap_or("-"),
            event.location.as_deref().unwrap_or("-"),
            event.status.as_deref().unwrap_or("-"),
            event.all_day
        )
    }));
    lines.join("\n")
}

fn render_calendar_create_table(
    account: &str,
    calendar: &CalendarCollection,
    event: &CalendarEvent,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("calendar.id", calendar.id.clone()),
        ("calendar.name", calendar.name.clone()),
        (
            "event.id",
            event.uid.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("event.href", event.href.clone()),
        (
            "event.summary",
            event.summary.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "event.start",
            event.start.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "event.end",
            event.end.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "event.location",
            event.location.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "event.description",
            event.description.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("event.all_day", event.all_day.to_string()),
    ])
}

fn render_calendar_delete_table(
    account: &str,
    calendar: &CalendarCollection,
    event: &CalendarEvent,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("calendar.id", calendar.id.clone()),
        ("calendar.name", calendar.name.clone()),
        (
            "deleted.id",
            event.uid.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("deleted.href", event.href.clone()),
        (
            "deleted.summary",
            event.summary.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "deleted.start",
            event.start.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "deleted.end",
            event.end.clone().unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_disk_info_table(account: &str, info: &DiskInfo) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("used_space", info.used_space.to_string()),
        ("total_space", info.total_space.to_string()),
        ("trash_size", info.trash_size.to_string()),
        (
            "user.login",
            info.user
                .as_ref()
                .and_then(|user| user.login.clone())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "user.display_name",
            info.user
                .as_ref()
                .and_then(|user| user.display_name.clone())
                .unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_disk_resource_table(account: &str, resource: &DiskResource) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("path\t{}", resource.path),
        format!("name\t{}", resource.name),
        format!("resource_type\t{}", resource.resource_type),
        format!(
            "mime_type\t{}",
            resource.mime_type.as_deref().unwrap_or("-")
        ),
        format!(
            "size\t{}",
            resource
                .size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        format!("created\t{}", resource.created.as_deref().unwrap_or("-")),
        format!("modified\t{}", resource.modified.as_deref().unwrap_or("-")),
        format!(
            "revision\t{}",
            resource
                .revision
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
    ];

    if let Some(children) = &resource.children {
        lines.push(format!("children.total\t{}", children.total));
        lines.push(format!("children.limit\t{}", children.limit));
        lines.push(format!("children.offset\t{}", children.offset));
        lines.push("children".to_string());
        lines.push("TYPE\tNAME\tPATH\tSIZE\tMODIFIED".to_string());
        lines.extend(children.items.iter().map(|item| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                item.resource_type,
                item.name,
                item.path,
                item.size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                item.modified.as_deref().unwrap_or("-")
            )
        }));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{render_mail_search_table, sanitize_table_cell};
    use crate::mail::MailMessageSummary;

    #[test]
    fn sanitize_table_cell_collapses_newlines_tabs_and_empty_values() {
        assert_eq!(sanitize_table_cell("one\ttwo\nthree"), "one two three");
        assert_eq!(sanitize_table_cell("   \r\n\t  "), "-");
    }

    #[test]
    fn render_mail_search_table_keeps_one_row_per_message() {
        let table = render_mail_search_table(
            "mock",
            "INBOX",
            "смета",
            &[MailMessageSummary {
                uid: 42,
                date: Some("Fri,\n13 Mar 2026".to_string()),
                from: Some("Sender\tName <sender@example.com>".to_string()),
                subject: Some("Тема\r\nписьма".to_string()),
                flags: vec!["\\Seen".to_string(), "custom".to_string()],
                size: Some(128),
            }],
        );

        let lines = table.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 6);
        assert_eq!(
            lines[5],
            "42\tFri, 13 Mar 2026\tSender Name <sender@example.com>\tТема письма\t\\Seen,custom\t128"
        );
    }
}
