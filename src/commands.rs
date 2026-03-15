use std::collections::BTreeMap;
use std::env;
use std::io::{self, IsTerminal, Write};

use serde::Serialize;
use serde_json::json;

use crate::account_store::{AccountStore, validate_account};
use crate::activity_store::{ActivityEntry, ActivityStore, NewActivityEntry, record_activity};
use crate::activity_undo::{
    ActivityUndoApplied, ActivityUndoResult, apply_activity_undo, calendar_create_undo,
    disk_publish_undo,
};
use crate::calendar::{
    CalendarCollection, CalendarCreateRequest, CalendarCreateReview, CalendarEvent,
    CalendarEventWindow, CalendarEventsRequest, CalendarInvite, calendar_create_command,
    calendar_create_request_from_invites, create_calendar_event, delete_calendar_event,
    list_calendar_events, list_calendars, parse_event_window, review_calendar_event_creation,
};
use crate::cli::{
    AccountCommand, ActivityCommand, AuthCommand, AuthServiceArg, CalendarCommand, Cli, Command,
    DiskCommand, DiskPublicCommand, GuideTopicArg, MailAttachmentCommand, MailCommand,
    MailInviteCommand, McpClientArg, McpCommand, McpTransportArg, OutputFormat, WorkflowCommand,
};
use crate::credential_store::{CredentialStore, StoredAppPasswordCredential};
use crate::disk::{
    DEFAULT_DISK_BASE_URL, DiskInfo, DiskPublishReview, DiskResource, DiskUnpublishReview,
    DiskUploadReview, DownloadedFile, PrivateDiskDownloadRequest, PrivateDiskListRequest,
    PrivateDiskMkdirRequest, PrivateDiskPublishRequest, PrivateDiskUnpublishRequest,
    PrivateDiskUploadRequest, PublicDiskRequest, PublicDownloadRequest, PublicResource,
    TransferProgress, UnpublishedDiskResource, UploadedFile, create_private_directory,
    download_private_resource_with_progress, download_public_resource,
    download_public_resource_with_progress, fetch_disk_info, fetch_private_resource,
    fetch_public_resource, publish_private_resource, review_private_publish,
    review_private_unpublish, review_private_upload, unpublish_private_resource,
    upload_private_resource_with_progress,
};
use crate::disk_link::{
    DiskUploadLinkRequest, DiskUploadLinkResult, DiskUploadLinkReview, review_disk_upload_link,
    upload_link_to_disk,
};
use crate::doctor::doctor_payload;
use crate::error::{Result, YacliError};
use crate::goal_router::goal_route_payload;
use crate::home::home_payload;
use crate::mail::{
    ExportedMailAttachment, ForwardedMail, InspectedMailInvite, MailAttachmentExportRequest,
    MailAttachmentSelector, MailAttachmentSummary, MailFolder, MailForwardRequest,
    MailInviteInspectRequest, MailMessage, MailMessageSummary, MailReplyRequest, MailSendRequest,
    MailSendReview, RepliedMail, SentMail, export_mail_attachment, forward_mail_message,
    inspect_mail_invite, list_mail_folders, list_mail_messages, load_mail_attachments,
    read_mail_message, reply_to_mail_message, review_mail_submission, search_mail_messages,
    send_mail_message,
};
use crate::mail_invite_flow::{
    MailInviteCreateEventPartialFailure, invite_create_event_command,
    partial_failure as invite_create_event_partial_failure,
};
use crate::mail_link::{
    MailSendLinkOutcome, MailSendLinkPartialFailure, MailSendLinkRequest, MailSendLinkResult,
    MailSendLinkReview, MailSendPublishedLinkRequest, MailSendPublishedLinkReview,
    review_mail_send_link, review_mail_send_published_link, send_link_via_mail,
    send_published_link_command, send_published_link_via_mail,
};
use crate::mcp::install::execute_install;
use crate::model::{AccountConfig, CalendarAuthMode, DiskAuthMode, MailAuthMode, NewAccountInput};
use crate::next_actions::next_actions_payload;
use crate::oauth::{
    OauthService, default_yacli_client_id, exchange_authorization_code, start_pkce_authorization,
};
use crate::oauth_session_store::{PendingOauthSession, PendingOauthSessionStore};
use crate::output::RenderedOutput;
use crate::runtime_context::{
    auth_state, ensure_calendar_supports_app_password, resolve_calendar_private_context,
    resolve_disk_private_context, resolve_mail_private_context,
};
use crate::suggestions::suggestions_payload;
use crate::update::execute_update;
use crate::workflows;

pub fn execute(cli: Cli) -> Result<RenderedOutput> {
    match cli.command {
        Command::Guide { topic } => execute_guide(cli.format, topic),
        Command::Add { email, name } => execute_simple_add(cli.format, email, name),
        Command::Setup {
            email,
            name,
            calendar_app_password,
            calendar_env_var,
            client,
            mcp_transport,
            mcp_url,
            skip_login,
            skip_mcp_install,
            plan_only,
        } => execute_setup(
            cli.format,
            SetupRequest {
                email,
                name,
                calendar_app_password,
                calendar_env_var,
                clients: client,
                mcp_transport,
                mcp_url,
                skip_login,
                skip_mcp_install,
                plan_only,
            },
        ),
        Command::Home { account, goal } => execute_home(cli.format, account, goal),
        Command::Doctor {
            account,
            goal,
            apply_safe,
        } => execute_doctor(cli.format, account, goal, apply_safe),
        Command::Next { account, goal } => execute_next(cli.format, account, goal),
        Command::Suggest { account, goal } => execute_suggest(cli.format, account, goal),
        Command::Goal { query, account } => execute_goal(cli.format, query, account),
        Command::Accounts => execute_account(cli.format, AccountCommand::List),
        Command::Activity { action } => execute_activity(cli.format, action),
        Command::Workflow { action } => execute_workflow(cli.format, action),
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
        Command::Mcp {
            action:
                Some(McpCommand::Install {
                    client,
                    transport,
                    url,
                }),
            ..
        } => execute_install(cli.format, client, transport, url),
        Command::Mcp { action: None, .. } => Err(YacliError::UnsupportedOperation(
            "mcp server mode is handled in main".to_string(),
        )),
        Command::Update {
            version,
            check,
            base_url,
        } => execute_update(cli.format, &version, check, base_url.as_deref()),
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

#[derive(Serialize)]
struct SetupAccountSelection {
    account: String,
    email: String,
    created: bool,
    reused: bool,
}

struct SetupRequest {
    email: Option<String>,
    name: Option<String>,
    calendar_app_password: Option<String>,
    calendar_env_var: Option<String>,
    clients: Vec<McpClientArg>,
    mcp_transport: McpTransportArg,
    mcp_url: Option<String>,
    skip_login: bool,
    skip_mcp_install: bool,
    plan_only: bool,
}

#[derive(Serialize)]
struct SetupStep {
    id: &'static str,
    status: &'static str,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<serde_json::Value>,
}

fn execute_setup(format: OutputFormat, request: SetupRequest) -> Result<RenderedOutput> {
    let SetupRequest {
        email,
        name,
        calendar_app_password,
        calendar_env_var,
        clients,
        mcp_transport,
        mcp_url,
        skip_login,
        skip_mcp_install,
        plan_only,
    } = request;

    if calendar_app_password.is_some() && calendar_env_var.is_some() {
        return Err(YacliError::Validation(
            "setup accepts either `--calendar-app-password` or `--calendar-env-var`, but not both"
                .to_string(),
        ));
    }

    let account = ensure_setup_account(email, name, plan_only)?;
    let mut steps = vec![SetupStep {
        id: "account",
        status: if account.created {
            if plan_only { "planned" } else { "created" }
        } else if plan_only {
            "planned"
        } else {
            "reused"
        },
        detail: if account.created {
            format!(
                "account `{}` for `{}` is ready",
                account.account, account.email
            )
        } else {
            format!(
                "using existing account `{}` for `{}`",
                account.account, account.email
            )
        },
        output: None,
    }];

    let mut exit_code = 0;
    let mut next_actions = Vec::new();

    if plan_only {
        steps.push(SetupStep {
            id: "mail_disk_login",
            status: if skip_login { "skipped" } else { "planned" },
            detail: if skip_login {
                "mail/disk OAuth login was skipped by request".to_string()
            } else {
                format!(
                    "will connect mail and disk for account `{}`",
                    account.account
                )
            },
            output: None,
        });
        steps.push(SetupStep {
            id: "calendar_login",
            status: if calendar_app_password.is_some() || calendar_env_var.is_some() {
                "planned"
            } else {
                "pending"
            },
            detail: if calendar_app_password.is_some() {
                "will connect calendar with app password".to_string()
            } else if let Some(env_var) = calendar_env_var.as_deref() {
                format!("will connect calendar from env var `{env_var}`")
            } else {
                "calendar still needs `--calendar-app-password` or `--calendar-env-var`".to_string()
            },
            output: None,
        });
        steps.push(SetupStep {
            id: "mcp_install",
            status: if skip_mcp_install {
                "skipped"
            } else {
                "planned"
            },
            detail: if skip_mcp_install {
                "mcp install was skipped by request".to_string()
            } else {
                format!(
                    "will register MCP via `{}` transport",
                    mcp_transport.label()
                )
            },
            output: None,
        });

        if !skip_login {
            next_actions.push("run `yacli setup` without `--plan-only` to complete OAuth login");
        }
        if calendar_app_password.is_none() && calendar_env_var.is_none() {
            next_actions.push(
                "provide `--calendar-app-password <пароль>` or `--calendar-env-var NAME` to finish calendar setup",
            );
        }
    } else {
        if skip_login {
            steps.push(SetupStep {
                id: "mail_disk_login",
                status: "skipped",
                detail: "mail/disk OAuth login was skipped by request".to_string(),
                output: None,
            });
            next_actions.push("run `yacli login` to connect Почту и Диск");
        } else {
            let login = execute_auth(
                OutputFormat::Json,
                AuthCommand::Login {
                    account: Some(account.account.clone()),
                    service: None,
                    client_id: None,
                    env_var: None,
                    app_password: None,
                    code: None,
                    login_hint: Some(account.email.clone()),
                },
            )?;
            let login_pending = login.json["status"].as_str() == Some("pending");
            steps.push(SetupStep {
                id: "mail_disk_login",
                status: if login_pending {
                    "pending"
                } else {
                    "completed"
                },
                detail: if login_pending {
                    "mail and disk OAuth session is started; finish it with the browser code"
                        .to_string()
                } else {
                    "mail and disk are connected".to_string()
                },
                output: Some(login.json),
            });
            if login_pending {
                next_actions.push(
                    "complete mail/disk OAuth with `yacli login --account <аккаунт> --code <код>`",
                );
            }
        }

        if let Some(app_password) = calendar_app_password {
            let calendar = execute_auth(
                OutputFormat::Json,
                AuthCommand::Login {
                    account: Some(account.account.clone()),
                    service: Some(AuthServiceArg::Calendar),
                    client_id: None,
                    env_var: None,
                    app_password: Some(app_password),
                    code: None,
                    login_hint: None,
                },
            )?;
            steps.push(SetupStep {
                id: "calendar_login",
                status: "completed",
                detail: "calendar is connected with app password".to_string(),
                output: Some(calendar.json),
            });
        } else if let Some(env_var) = calendar_env_var {
            let calendar = execute_auth(
                OutputFormat::Json,
                AuthCommand::Login {
                    account: Some(account.account.clone()),
                    service: Some(AuthServiceArg::Calendar),
                    client_id: None,
                    env_var: Some(env_var.clone()),
                    app_password: None,
                    code: None,
                    login_hint: None,
                },
            )?;
            steps.push(SetupStep {
                id: "calendar_login",
                status: "completed",
                detail: format!("calendar is connected from env var `{env_var}`"),
                output: Some(calendar.json),
            });
        } else {
            steps.push(SetupStep {
                id: "calendar_login",
                status: "pending",
                detail: "calendar still needs `--calendar-app-password` or `--calendar-env-var`"
                    .to_string(),
                output: None,
            });
            next_actions.push(
                "run `yacli login calendar --app-password <пароль>` or `yacli setup --calendar-app-password <пароль>`",
            );
        }

        if skip_mcp_install {
            steps.push(SetupStep {
                id: "mcp_install",
                status: "skipped",
                detail: "mcp install was skipped by request".to_string(),
                output: None,
            });
            next_actions.push("run `yacli mcp install` when you are ready to register the server");
        } else {
            let install = execute_install(OutputFormat::Json, clients, mcp_transport, mcp_url)?;
            if install.exit_code != 0 {
                exit_code = install.exit_code;
            }
            steps.push(SetupStep {
                id: "mcp_install",
                status: if install.exit_code == 0 {
                    "completed"
                } else {
                    "partial"
                },
                detail: if install.exit_code == 0 {
                    "mcp server registration completed".to_string()
                } else {
                    "mcp install completed with partial failures; inspect items in output"
                        .to_string()
                },
                output: Some(install.json),
            });
        }
    }

    let status = if plan_only {
        None
    } else {
        Some(execute_auth(
            OutputFormat::Json,
            AuthCommand::Status {
                account: Some(account.account.clone()),
            },
        )?)
    };

    let payload = json!({
        "account": account,
        "plan_only": plan_only,
        "next_actions": next_actions,
        "steps": steps,
        "status": status.as_ref().map(|rendered| rendered.json.clone()),
    });
    let table = render_setup_table(&payload);

    let Some(mut object) = payload.as_object().cloned() else {
        return Err(YacliError::Serialization(
            "expected setup payload object".to_string(),
        ));
    };
    object.insert("ok".to_string(), json!(exit_code == 0));
    object.insert("operation".to_string(), json!("setup"));

    Ok(RenderedOutput {
        format,
        json: serde_json::Value::Object(object),
        table,
        exit_code,
    })
}

fn ensure_setup_account(
    email: Option<String>,
    name: Option<String>,
    plan_only: bool,
) -> Result<SetupAccountSelection> {
    match email {
        Some(email) => {
            let explicit_name = name.is_some();
            let requested_name = name.unwrap_or_else(|| derive_account_name_from_email(&email));
            let mut store = AccountStore::load()?;

            if let Some(existing) = store.file.accounts.get(&requested_name) {
                if existing.email != email {
                    return Err(YacliError::Validation(format!(
                        "account `{requested_name}` already exists for `{}`; choose another `--name`",
                        existing.email
                    )));
                }
                if !plan_only {
                    store.set_current(&requested_name)?;
                    store.save()?;
                }
                return Ok(SetupAccountSelection {
                    account: requested_name,
                    email,
                    created: false,
                    reused: true,
                });
            }

            if let Some((existing_name, _)) = store
                .file
                .accounts
                .iter()
                .find(|(_, account)| account.email.eq_ignore_ascii_case(&email))
            {
                if explicit_name && existing_name != &requested_name {
                    return Err(YacliError::Validation(format!(
                        "email `{email}` already exists as account `{existing_name}`; rerun without `--name` or use `--name {existing_name}`"
                    )));
                }
                let existing_name = existing_name.clone();
                if !plan_only {
                    store.set_current(&existing_name)?;
                    store.save()?;
                }
                return Ok(SetupAccountSelection {
                    account: existing_name,
                    email,
                    created: false,
                    reused: true,
                });
            }

            if !plan_only {
                let account_config = AccountConfig::new(NewAccountInput {
                    email: email.clone(),
                    default: true,
                    mail_auth_mode: MailAuthMode::OauthXoauth2,
                    calendar_auth_mode: CalendarAuthMode::AppPassword,
                    disk_auth_mode: DiskAuthMode::Oauth,
                    mail_credential_ref: None,
                    calendar_credential_ref: None,
                    disk_credential_ref: None,
                });
                let report = validate_account(&requested_name, &account_config);
                if !report.valid {
                    return Err(YacliError::Validation(report.errors.join("; ")));
                }
                store.add_account(requested_name.clone(), account_config)?;
                store.set_current(&requested_name)?;
                store.save()?;
            }

            Ok(SetupAccountSelection {
                account: requested_name,
                email,
                created: true,
                reused: false,
            })
        }
        None => {
            let mut store = AccountStore::load()?;
            let account_name = match name.as_deref() {
                Some(name) => store.resolved_account_name(Some(name))?,
                None => store.current_account_name()?,
            };
            let email = store.get_account(&account_name)?.email.clone();
            if !plan_only && !store.is_current_account(&account_name) {
                store.set_current(&account_name)?;
                store.save()?;
            }
            Ok(SetupAccountSelection {
                account: account_name,
                email,
                created: false,
                reused: true,
            })
        }
    }
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

fn execute_workflow(format: OutputFormat, action: WorkflowCommand) -> Result<RenderedOutput> {
    match action {
        WorkflowCommand::List => {
            let items = workflows::workflow_catalog();
            let mut lines = vec!["ID\tTITLE\tCONNECTS\tPROMPT\tSKILL".to_string()];
            for item in &items {
                lines.push(format!(
                    "{}\t{}\t{}\t{}\t{}",
                    item["id"].as_str().unwrap_or_default(),
                    item["title"].as_str().unwrap_or_default(),
                    item["connects"].as_str().unwrap_or_default(),
                    item["prompt_name"].as_str().unwrap_or_default(),
                    item["skill_name"].as_str().unwrap_or_default()
                ));
            }
            ok_output(
                format,
                "workflow.list",
                json!({ "items": items }),
                lines.join("\n"),
            )
        }
        WorkflowCommand::Show { id } => {
            let workflow = workflows::workflow_resource_detail(&id).ok_or_else(|| {
                YacliError::UnsupportedOperation(format!("unknown workflow: {id}"))
            })?;
            let cli_steps = workflow["cli_steps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let mcp_tools = workflow["mcp_tools"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join(",");

            ok_output(
                format,
                "workflow.show",
                json!({ "workflow": workflow }),
                render_key_value_table(&[
                    (
                        "id",
                        workflow["id"].as_str().unwrap_or_default().to_string(),
                    ),
                    (
                        "title",
                        workflow["title"].as_str().unwrap_or_default().to_string(),
                    ),
                    (
                        "connects",
                        workflow["connects"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    (
                        "prompt_name",
                        workflow["prompt_name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    (
                        "skill_name",
                        workflow["skill_name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    ("mcp_tools", mcp_tools),
                    ("cli_steps", cli_steps),
                ]),
            )
        }
    }
}

fn execute_activity(format: OutputFormat, action: ActivityCommand) -> Result<RenderedOutput> {
    match action {
        ActivityCommand::List { limit } => {
            let store = ActivityStore::load()?;
            let items = store
                .entries()
                .iter()
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            ok_output(
                format,
                "activity.list",
                json!({
                    "limit": limit,
                    "items": items,
                }),
                render_activity_list_table(&items),
            )
        }
        ActivityCommand::Show { id } => {
            let store = ActivityStore::load()?;
            let entry = store.find(&id).cloned().ok_or_else(|| {
                YacliError::Validation(format!("activity show: запись `{id}` не найдена"))
            })?;
            ok_output(
                format,
                "activity.show",
                json!({
                    "entry": entry,
                }),
                render_activity_show_table(&entry),
            )
        }
        ActivityCommand::Undo { id } => {
            let store = ActivityStore::load()?;
            let entry = store.find(&id).cloned().ok_or_else(|| {
                YacliError::Validation(format!("activity undo: запись `{id}` не найдена"))
            })?;
            let applied = apply_activity_undo(&entry)?;
            record_activity_best_effort(NewActivityEntry {
                source: "cli".to_string(),
                operation: "activity.undo".to_string(),
                account: applied.account.clone(),
                summary: applied.summary.clone(),
                replay_command: applied.replay_command.clone(),
                undo: None,
            });
            ok_output(
                format,
                "activity.undo",
                json!({
                    "entry": entry,
                    "undo": applied,
                }),
                render_activity_undo_table(&applied),
            )
        }
    }
}

fn execute_home(
    format: OutputFormat,
    account: Option<String>,
    goal: Option<String>,
) -> Result<RenderedOutput> {
    let payload = home_payload(account.as_deref(), goal.as_deref())?;
    let table = render_home_table(&payload);
    ok_output(format, "home", payload, table)
}

fn execute_doctor(
    format: OutputFormat,
    account: Option<String>,
    goal: Option<String>,
    apply_safe: bool,
) -> Result<RenderedOutput> {
    let mut payload = doctor_payload(account.as_deref(), goal.as_deref())?;
    if apply_safe {
        payload["safe_remediation"] =
            apply_safe_doctor_remediation(account.as_deref(), goal.as_deref())?;
        if let Some(entry) = doctor_safe_remediation_activity_entry(
            "cli",
            &payload["safe_remediation"],
            account.as_deref(),
            goal.as_deref(),
        ) {
            record_activity_best_effort(entry);
        }
        payload["status"] = payload["safe_remediation"]["doctor_after"]["status"].clone();
        payload["checks"] = payload["safe_remediation"]["doctor_after"]["checks"].clone();
        payload["focus_checks"] =
            payload["safe_remediation"]["doctor_after"]["focus_checks"].clone();
        payload["mcp_clients"] = payload["safe_remediation"]["doctor_after"]["mcp_clients"].clone();
        payload["suggested_commands"] =
            payload["safe_remediation"]["doctor_after"]["suggested_commands"].clone();
        payload["onboardingStatus"] =
            payload["safe_remediation"]["doctor_after"]["onboardingStatus"].clone();
        payload["services"] = payload["safe_remediation"]["doctor_after"]["services"].clone();
        payload["current_account"] =
            payload["safe_remediation"]["doctor_after"]["current_account"].clone();
        payload["email"] = payload["safe_remediation"]["doctor_after"]["email"].clone();
        payload["goal_route"] = payload["safe_remediation"]["doctor_after"]["goal_route"].clone();
    }
    let table = render_doctor_table(&payload);
    ok_output(format, "doctor", payload, table)
}

fn execute_next(
    format: OutputFormat,
    account: Option<String>,
    goal: Option<String>,
) -> Result<RenderedOutput> {
    let payload = next_actions_payload(account.as_deref(), goal.as_deref())?;
    let table = render_next_actions_table(&payload);
    ok_output(format, "next", payload, table)
}

fn execute_suggest(
    format: OutputFormat,
    account: Option<String>,
    goal: Option<String>,
) -> Result<RenderedOutput> {
    let payload = suggestions_payload(account.as_deref(), goal.as_deref())?;
    let table = render_suggestions_table(&payload);
    ok_output(format, "suggest", payload, table)
}

fn execute_goal(
    format: OutputFormat,
    query: String,
    account: Option<String>,
) -> Result<RenderedOutput> {
    let payload = goal_route_payload(&query, account.as_deref())?;
    let table = render_goal_table(&payload);
    ok_output(format, "goal", payload, table)
}

const SAFE_CALENDAR_ENV_VAR: &str = "YACLI_CALENDAR_APP_PASSWORD";

pub fn apply_safe_doctor_remediation(
    account: Option<&str>,
    goal: Option<&str>,
) -> Result<serde_json::Value> {
    let doctor_before = doctor_payload(account, goal)?;
    let mut steps = Vec::new();
    let mut applied_count = 0usize;
    let mut needs_input_count = 0usize;
    let mut failed_count = 0usize;

    if doctor_before["current_account"].is_null() {
        let command = doctor_check(&doctor_before, "account")
            .and_then(|check| check["recommended_command"].as_str())
            .unwrap_or("yacli setup me@yandex.ru")
            .to_string();
        steps.push(remediation_step(
            "account",
            "Аккаунт",
            "needs_input",
            "Для безопасного auto-fix нужен уже выбранный аккаунт или `yacli setup` с email."
                .to_string(),
            Some(command),
            None,
        ));
        needs_input_count += 1;
    }

    if let Some(account_name) = doctor_before["current_account"].as_str() {
        if let Some(check) = doctor_check(&doctor_before, "calendar")
            && check["status"] != "completed"
        {
            if env::var_os(SAFE_CALENDAR_ENV_VAR).is_some() {
                match execute_auth(
                    OutputFormat::Json,
                    AuthCommand::Login {
                        account: Some(account_name.to_string()),
                        service: Some(AuthServiceArg::Calendar),
                        client_id: None,
                        env_var: Some(SAFE_CALENDAR_ENV_VAR.to_string()),
                        app_password: None,
                        code: None,
                        login_hint: None,
                    },
                ) {
                    Ok(output) => {
                        steps.push(remediation_step(
                            "calendar",
                            "Календарь",
                            "applied",
                            format!(
                                "Календарь привязан к стандартной env-переменной `{SAFE_CALENDAR_ENV_VAR}`."
                            ),
                            Some(format!(
                                "yacli login calendar --env-var {SAFE_CALENDAR_ENV_VAR}"
                            )),
                            Some(output.json),
                        ));
                        applied_count += 1;
                    }
                    Err(err) => {
                        steps.push(remediation_step(
                            "calendar",
                            "Календарь",
                            "failed",
                            format!("Не удалось применить safe fix для календаря: {err}"),
                            check["recommended_command"]
                                .as_str()
                                .map(ToString::to_string),
                            None,
                        ));
                        failed_count += 1;
                    }
                }
            } else {
                steps.push(remediation_step(
                    "calendar",
                    "Календарь",
                    "needs_input",
                    format!(
                        "Для safe fix не хватает env-переменной `{SAFE_CALENDAR_ENV_VAR}` с app password."
                    ),
                    Some(format!(
                        "export {SAFE_CALENDAR_ENV_VAR}=<пароль> && yacli doctor --apply-safe"
                    )),
                    None,
                ));
                needs_input_count += 1;
            }
        }

        for (service_id, title) in [("mail", "Почта"), ("disk", "Диск")] {
            if let Some(check) = doctor_check(&doctor_before, service_id)
                && check["status"] != "completed"
            {
                steps.push(remediation_step(
                    service_id,
                    title,
                    "interactive",
                    "Этот шаг требует OAuth login и остаётся явным пользовательским действием."
                        .to_string(),
                    check["recommended_command"]
                        .as_str()
                        .map(ToString::to_string),
                    None,
                ));
                needs_input_count += 1;
            }
        }
    }

    if let Some(check) = doctor_check(&doctor_before, "mcp_install")
        && check["status"] != "completed"
    {
        let clients = safe_doctor_install_clients(
            doctor_before["mcp_clients"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        if clients.is_empty() {
            steps.push(remediation_step(
                "mcp_install",
                "Установка MCP-клиентов",
                "needs_input",
                "Нет проверяемых config-based клиентов для безопасной автоматической установки. Native CLI-клиенты оставлены как явный ручной шаг."
                    .to_string(),
                check["recommended_command"].as_str().map(ToString::to_string),
                None,
            ));
            needs_input_count += 1;
        } else {
            let command = format!(
                "yacli mcp install{}",
                clients
                    .iter()
                    .map(|client| format!(" --client {}", mcp_client_label(*client)))
                    .collect::<String>()
            );
            match execute_install(OutputFormat::Json, clients, McpTransportArg::Stdio, None) {
                Ok(output) => {
                    let status = if output.exit_code == 0 {
                        applied_count += 1;
                        "applied"
                    } else {
                        failed_count += 1;
                        "failed"
                    };
                    steps.push(remediation_step(
                        "mcp_install",
                        "Установка MCP-клиентов",
                        status,
                        format!(
                            "Safe remediation применил MCP install для config-based клиентов: {}.",
                            output.json["items"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|item| item["client"].as_str().unwrap_or_default())
                                .filter(|label| !label.is_empty())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        Some(command),
                        Some(output.json),
                    ));
                }
                Err(err) => {
                    steps.push(remediation_step(
                        "mcp_install",
                        "Установка MCP-клиентов",
                        "failed",
                        format!("Не удалось применить safe MCP install: {err}"),
                        Some(command),
                        None,
                    ));
                    failed_count += 1;
                }
            }
        }
    }

    let doctor_after = doctor_payload(account, goal)?;
    let status = if failed_count > 0 {
        "failed"
    } else if applied_count > 0 && needs_input_count > 0 {
        "partial"
    } else if applied_count > 0 {
        "applied"
    } else if needs_input_count > 0 {
        "needs_input"
    } else {
        "noop"
    };

    Ok(json!({
        "status": status,
        "applied_count": applied_count,
        "needs_input_count": needs_input_count,
        "failed_count": failed_count,
        "steps": steps,
        "doctor_before": doctor_before,
        "doctor_after": doctor_after,
    }))
}

pub fn doctor_safe_remediation_activity_entry(
    source: &str,
    remediation: &serde_json::Value,
    requested_account: Option<&str>,
    goal: Option<&str>,
) -> Option<NewActivityEntry> {
    if remediation["applied_count"].as_u64().unwrap_or(0) == 0
        || remediation["failed_count"].as_u64().unwrap_or(0) > 0
    {
        return None;
    }

    let applied_titles = remediation["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|step| step["status"] == "applied")
        .filter_map(|step| step["title"].as_str())
        .collect::<Vec<_>>();
    if applied_titles.is_empty() {
        return None;
    }

    let account = requested_account
        .map(ToString::to_string)
        .or_else(|| {
            remediation["doctor_after"]["current_account"]
                .as_str()
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "-".to_string());

    let mut replay_command = "yacli doctor --apply-safe".to_string();
    if let Some(account) = requested_account {
        replay_command.push_str(" --account ");
        replay_command.push_str(&shell_quote(account));
    }
    if let Some(goal) = goal
        && !goal.trim().is_empty()
    {
        replay_command.push_str(" --goal ");
        replay_command.push_str(&shell_quote(goal.trim()));
    }

    Some(NewActivityEntry {
        source: source.to_string(),
        operation: "doctor.apply_safe".to_string(),
        account,
        summary: format!("Применены safe fixes: {}", applied_titles.join(", ")),
        replay_command,
        undo: None,
    })
}

fn doctor_check<'a>(payload: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    payload["checks"]
        .as_array()?
        .iter()
        .find(|check| check["id"] == id)
}

fn remediation_step(
    id: &str,
    title: &str,
    status: &str,
    detail: String,
    command: Option<String>,
    output: Option<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "id": id,
        "title": title,
        "status": status,
        "detail": detail,
        "command": command,
        "output": output,
    })
}

fn safe_doctor_install_clients(clients: &[serde_json::Value]) -> Vec<McpClientArg> {
    let mut selected = Vec::new();
    for client in clients {
        let mechanism = client["mechanism"].as_str().unwrap_or_default();
        let status = client["status"].as_str().unwrap_or_default();
        let experimental = client["experimental"].as_bool().unwrap_or(false);
        if mechanism != "json_file" || status != "not_installed" || experimental {
            continue;
        }
        if let Some(arg) = mcp_client_arg_from_label(client["client"].as_str().unwrap_or_default())
        {
            selected.push(arg);
        }
    }
    selected
}

fn mcp_client_arg_from_label(label: &str) -> Option<McpClientArg> {
    match label {
        "claude" => Some(McpClientArg::Claude),
        "claude-desktop" => Some(McpClientArg::ClaudeDesktop),
        "codex" => Some(McpClientArg::Codex),
        "gemini" => Some(McpClientArg::Gemini),
        "warp" => Some(McpClientArg::Warp),
        "zed" => Some(McpClientArg::Zed),
        "cursor" => Some(McpClientArg::Cursor),
        "antigravity" => Some(McpClientArg::Antigravity),
        "windsurf" => Some(McpClientArg::Windsurf),
        _ => None,
    }
}

fn mcp_client_label(client: McpClientArg) -> &'static str {
    match client {
        McpClientArg::Claude => "claude",
        McpClientArg::ClaudeDesktop => "claude-desktop",
        McpClientArg::Codex => "codex",
        McpClientArg::Gemini => "gemini",
        McpClientArg::Warp => "warp",
        McpClientArg::Zed => "zed",
        McpClientArg::Cursor => "cursor",
        McpClientArg::Antigravity => "antigravity",
        McpClientArg::Windsurf => "windsurf",
    }
}

fn record_activity_best_effort(entry: NewActivityEntry) {
    if let Err(err) = record_activity(entry) {
        eprintln!(
            "{}",
            json!({
                "ok": false,
                "warning": "activity_log_unavailable",
                "message": format!("failed to record activity entry: {err}"),
            })
        );
    }
}

fn transfer_activity_summary(
    summary_prefix: &str,
    target: &str,
    attempts: usize,
    elapsed_ms: u64,
    resumed_from_bytes: Option<u64>,
) -> String {
    let mut details = vec![
        format!("попыток: {attempts}"),
        format!("время: {elapsed_ms} ms"),
    ];
    if let Some(bytes) = resumed_from_bytes.filter(|bytes| *bytes > 0) {
        details.push(format!("resume: {bytes} B"));
    }

    format!("{summary_prefix}: {target} ({})", details.join(", "))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
                    let resolved_login_hint = login_hint.or(Some(account.email.clone()));
                    let mut session_store = PendingOauthSessionStore::load()?;
                    let existing_pending = session_store
                        .get_matching(&account_name, &services, &client_id)
                        .cloned();
                    let session_reused = existing_pending.is_some();
                    let mut pending_session_persisted = false;
                    let session = if let Some(pending) = existing_pending {
                        pending_session_persisted = true;
                        pending.to_authorization_session()
                    } else {
                        let session = start_pkce_authorization(
                            &services,
                            &client_id,
                            resolved_login_hint.as_deref(),
                        )?;
                        if code.is_none() {
                            session_store.replace_matching(
                                PendingOauthSession::from_authorization_session(
                                    account_name.clone(),
                                    &services,
                                    resolved_login_hint.clone(),
                                    session.clone(),
                                ),
                            );
                            session_store.save()?;
                            pending_session_persisted = true;
                        }
                        session
                    };

                    if code.is_none() && !io::stdin().is_terminal() {
                        return oauth_login_pending_output(
                            format,
                            &account_name,
                            &services,
                            &client_id,
                            client_id_source,
                            session.request.clone(),
                            session_reused,
                        );
                    }

                    let code = match code {
                        Some(code) => code,
                        None => read_confirmation_code(&session.request.authorization_url)?,
                    };
                    let login = exchange_authorization_code(session, &code)?;
                    if pending_session_persisted {
                        session_store.remove_matching(&account_name, &services, &client_id);
                        session_store.save()?;
                    }

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
            .inspect(|_| {
                record_activity_best_effort(NewActivityEntry {
                    source: "cli".to_string(),
                    operation: "disk.mkdir".to_string(),
                    account: resolved_account.clone(),
                    summary: format!("Создана папка на Диске: {}", resource.path),
                    replay_command: format!("yacli disk mkdir {}", shell_quote(&resource.path)),
                    undo: None,
                });
            })
        }
        DiskCommand::Upload {
            account,
            source,
            path,
            overwrite,
            dry_run,
        } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let request = PrivateDiskUploadRequest {
                source,
                path: path.clone(),
                overwrite,
            };
            if dry_run {
                let review = review_private_upload(&request)?;
                ok_output(
                    format,
                    "disk.upload.review",
                    json!({
                        "account": resolved_account,
                        "path": path,
                        "dry_run": true,
                        "upload": review,
                    }),
                    render_disk_upload_review_table(&resolved_account, &review),
                )
            } else {
                let progress = cli_transfer_progress("upload", &request.path, format);
                let (resource, uploaded) = upload_private_resource_with_progress(
                    &base_url,
                    &access_token,
                    &request,
                    progress,
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
                .inspect(|_| {
                    let mut replay = format!(
                        "yacli disk upload {} {}",
                        shell_quote(&uploaded.source_path),
                        shell_quote(&uploaded.remote_path)
                    );
                    if uploaded.overwrite {
                        replay.push_str(" --overwrite");
                    }
                    replay.push_str(" --dry-run");
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "disk.upload".to_string(),
                        account: resolved_account.clone(),
                        summary: transfer_activity_summary(
                            "Загружен файл на Диск",
                            &uploaded.remote_path,
                            uploaded.attempts,
                            uploaded.elapsed_ms,
                            None,
                        ),
                        replay_command: replay,
                        undo: None,
                    });
                })
            }
        }
        DiskCommand::UploadLink {
            account,
            source,
            path,
            overwrite,
            dry_run,
        } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let request = DiskUploadLinkRequest {
                source,
                disk_path: path,
                overwrite,
            };
            if dry_run {
                let review = review_disk_upload_link(&request)?;
                ok_output(
                    format,
                    "disk.upload_link.review",
                    json!({
                        "account": resolved_account,
                        "path": request.disk_path,
                        "dry_run": true,
                        "review": review,
                    }),
                    render_disk_upload_link_review_table(&resolved_account, &review),
                )
            } else {
                let result = upload_link_to_disk(&base_url, &access_token, &request)?;
                ok_output(
                    format,
                    "disk.upload_link",
                    json!({
                        "account": resolved_account,
                        "path": request.disk_path,
                        "result": result,
                    }),
                    render_disk_upload_link_table(&resolved_account, &result),
                )
                .inspect(|_| {
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "disk.upload_link".to_string(),
                        account: resolved_account.clone(),
                        summary: format!(
                            "Загружен и опубликован ресурс на Диске: {}",
                            result
                                .resource
                                .public_url
                                .as_deref()
                                .unwrap_or(&result.resource.path)
                        ),
                        replay_command: format!(
                            "yacli disk upload-link --source {} --path {} --dry-run",
                            shell_quote(&result.upload.source_path),
                            shell_quote(&result.resource.path)
                        ),
                        undo: None,
                    });
                })
            }
        }
        DiskCommand::Download {
            account,
            path,
            output,
            force,
        } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let request = PrivateDiskDownloadRequest {
                path: path.clone(),
                output,
                force,
            };
            let progress =
                cli_transfer_progress("download", &request.output.display().to_string(), format);
            let (resource, artifact) = download_private_resource_with_progress(
                &base_url,
                &access_token,
                &request,
                progress,
            )?;

            ok_output(
                format,
                "disk.download",
                json!({
                    "account": resolved_account,
                    "path": path,
                    "resource": resource,
                    "download": artifact,
                }),
                render_disk_download_table(&resolved_account, &resource, &artifact),
            )
            .inspect(|_| {
                let mut replay = format!(
                    "yacli disk download {} --output {}",
                    shell_quote(&resource.path),
                    shell_quote(&artifact.output_path)
                );
                if force {
                    replay.push_str(" --force");
                }
                record_activity_best_effort(NewActivityEntry {
                    source: "cli".to_string(),
                    operation: "disk.download".to_string(),
                    account: resolved_account.clone(),
                    summary: transfer_activity_summary(
                        "Скачан файл с Диска",
                        &resource.path,
                        artifact.attempts,
                        artifact.elapsed_ms,
                        Some(artifact.resumed_from_bytes),
                    ),
                    replay_command: replay,
                    undo: None,
                });
            })
        }
        DiskCommand::Publish {
            account,
            path,
            dry_run,
        } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let request = PrivateDiskPublishRequest { path: path.clone() };
            if dry_run {
                let review = review_private_publish(&base_url, &access_token, &request)?;
                ok_output(
                    format,
                    "disk.publish.review",
                    json!({
                        "account": resolved_account,
                        "path": path,
                        "dry_run": true,
                        "review": review,
                    }),
                    render_disk_publish_review_table(&resolved_account, &review),
                )
            } else {
                let resource = publish_private_resource(&base_url, &access_token, &request)?;

                ok_output(
                    format,
                    "disk.publish",
                    json!({
                        "account": resolved_account,
                        "path": path,
                        "resource": resource,
                    }),
                    render_disk_publish_table(&resolved_account, &resource),
                )
                .inspect(|_| {
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "disk.publish".to_string(),
                        account: resolved_account.clone(),
                        summary: format!(
                            "Опубликован ресурс на Диске: {}",
                            resource.public_url.as_deref().unwrap_or(&resource.path)
                        ),
                        replay_command: format!(
                            "yacli disk publish {} --dry-run",
                            shell_quote(&resource.path)
                        ),
                        undo: Some(disk_publish_undo(&resource)),
                    });
                })
            }
        }
        DiskCommand::Unpublish {
            account,
            path,
            dry_run,
        } => {
            let (resolved_account, base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let request = PrivateDiskUnpublishRequest { path: path.clone() };
            if dry_run {
                let review = review_private_unpublish(&base_url, &access_token, &request)?;
                ok_output(
                    format,
                    "disk.unpublish.review",
                    json!({
                        "account": resolved_account,
                        "path": path,
                        "dry_run": true,
                        "review": review,
                    }),
                    render_disk_unpublish_review_table(&resolved_account, &review),
                )
            } else {
                let result = unpublish_private_resource(&base_url, &access_token, &request)?;

                ok_output(
                    format,
                    "disk.unpublish",
                    json!({
                        "account": resolved_account,
                        "path": path,
                        "result": result,
                    }),
                    render_disk_unpublish_table(&resolved_account, &result),
                )
                .inspect(|_| {
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "disk.unpublish".to_string(),
                        account: resolved_account.clone(),
                        summary: format!(
                            "Отозвана публичная ссылка на Диске: {}",
                            result
                                .revoked_public_url
                                .as_deref()
                                .unwrap_or(&result.resource.path)
                        ),
                        replay_command: format!(
                            "yacli disk unpublish {} --dry-run",
                            shell_quote(&result.resource.path)
                        ),
                        undo: None,
                    });
                })
            }
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
            dry_run,
        } => {
            let (resolved_account, app_password, context) =
                resolve_calendar_private_context(account.as_deref())?;
            let request = CalendarCreateRequest {
                calendar,
                summary,
                start,
                end,
                description,
                location,
            };
            if dry_run {
                let (calendar, review) = review_calendar_event_creation(
                    &context.caldav_base_url,
                    &context.email,
                    &app_password,
                    request,
                )?;
                ok_output(
                    format,
                    "calendar.create.review",
                    json!({
                        "account": resolved_account,
                        "email": context.email,
                        "calendar": calendar,
                        "dry_run": true,
                        "review": review,
                    }),
                    render_calendar_create_review_table(&resolved_account, &calendar, &review),
                )
            } else {
                let (calendar, event) = create_calendar_event(
                    &context.caldav_base_url,
                    &context.email,
                    &app_password,
                    request,
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
                .inspect(|_| {
                    let replay = calendar_create_command(
                        &CalendarCreateRequest {
                            calendar: calendar.id.clone(),
                            summary: event.summary.clone().unwrap_or_default(),
                            start: event.start.clone().unwrap_or_default(),
                            end: event.end.clone().unwrap_or_default(),
                            description: event.description.clone(),
                            location: event.location.clone(),
                        },
                        true,
                    );
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "calendar.create".to_string(),
                        account: resolved_account.clone(),
                        summary: format!(
                            "Создано событие в календаре {}: {}",
                            calendar.name,
                            event.summary.as_deref().unwrap_or("-")
                        ),
                        replay_command: replay,
                        undo: calendar_create_undo(&calendar, &event),
                    });
                })
            }
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
            attachments,
            dry_run,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let attachments = load_mail_attachments(&attachments)?;
            let request = MailSendRequest {
                to: vec![to],
                cc,
                bcc,
                subject,
                text: body,
                html,
                attachments,
                thread_headers: None,
            };

            if dry_run {
                let review = review_mail_submission(auth, request)?;
                ok_output(
                    format,
                    "mail.send.review",
                    json!({
                        "account": resolved_account,
                        "email": context.email,
                        "smtp": {
                            "host": context.smtp_host,
                            "port": context.smtp_port,
                        },
                        "dry_run": true,
                        "review": review,
                    }),
                    render_mail_send_review_table(&resolved_account, &review),
                )
            } else {
                let sent = send_mail_message(&context.smtp_host, context.smtp_port, auth, request)?;

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
                .inspect(|_| {
                    let mut replay = format!(
                        "yacli mail send {} {} {}",
                        shell_quote(sent.to.first().map(String::as_str).unwrap_or("")),
                        shell_quote(&sent.subject),
                        shell_quote("<текст письма>")
                    );
                    replay.push_str(" --dry-run");
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "mail.send".to_string(),
                        account: resolved_account.clone(),
                        summary: format!(
                            "Отправлено письмо {}: {}",
                            sent.to.first().cloned().unwrap_or_else(|| "-".to_string()),
                            sent.subject
                        ),
                        replay_command: replay,
                        undo: None,
                    });
                })
            }
        }
        MailCommand::SendLink {
            account,
            to,
            cc,
            bcc,
            subject,
            body,
            html,
            source,
            path,
            overwrite,
            dry_run,
        } => {
            let (resolved_account, disk_base_url, access_token) =
                resolve_disk_private_context(account.as_deref())?;
            let (_, auth, context) = resolve_mail_private_context(account.as_deref())?;
            let request = MailSendLinkRequest {
                source,
                disk_path: path,
                overwrite,
                to: vec![to],
                cc,
                bcc,
                subject,
                text: body,
                html,
            };

            if dry_run {
                let review = review_mail_send_link(auth, &request)?;
                ok_output(
                    format,
                    "mail.send_link.review",
                    json!({
                        "account": resolved_account,
                        "disk": {
                            "base_url": disk_base_url,
                        },
                        "smtp": {
                            "host": context.smtp_host,
                            "port": context.smtp_port,
                        },
                        "dry_run": true,
                        "review": review,
                    }),
                    render_mail_send_link_review_table(&resolved_account, &review),
                )
            } else {
                let outcome = send_link_via_mail(
                    &disk_base_url,
                    &access_token,
                    &context.smtp_host,
                    context.smtp_port,
                    auth,
                    &request,
                )?;
                match outcome {
                    MailSendLinkOutcome::Sent { result } => ok_output(
                        format,
                        "mail.send_link",
                        json!({
                            "account": resolved_account,
                            "status": "completed",
                            "result": result,
                        }),
                        render_mail_send_link_table(&resolved_account, &result),
                    )
                    .inspect(|_| {
                        let mut replay = format!(
                            "yacli mail send-link {} {} {} --source {} --path {} --dry-run",
                            shell_quote(
                                result
                                    .sent
                                    .to
                                    .first()
                                    .map(String::as_str)
                                    .unwrap_or_default()
                            ),
                            shell_quote(&result.sent.subject),
                            shell_quote("<текст письма>"),
                            shell_quote(&request.source.display().to_string()),
                            shell_quote(&request.disk_path)
                        );
                        if request.overwrite {
                            replay.push_str(" --overwrite");
                        }
                        record_activity_best_effort(NewActivityEntry {
                            source: "cli".to_string(),
                            operation: "mail.send_link".to_string(),
                            account: resolved_account.clone(),
                            summary: format!(
                                "Отправлена публичная ссылка на файл: {} -> {}",
                                result.resource.path,
                                result.resource.public_url.as_deref().unwrap_or("-")
                            ),
                            replay_command: replay,
                            undo: None,
                        });
                    }),
                    MailSendLinkOutcome::Partial { partial } => {
                        record_activity_best_effort(NewActivityEntry {
                            source: "cli".to_string(),
                            operation: "mail.send_link.partial".to_string(),
                            account: resolved_account.clone(),
                            summary: format!(
                                "Публичная ссылка создана, но письмо не отправлено: {} -> {}",
                                partial.resource.path,
                                partial.recovery.share_public_link.public_url
                            ),
                            replay_command: partial.recovery.retry_mail_step.command.clone(),
                            undo: Some(disk_publish_undo(&partial.resource)),
                        });
                        non_ok_output(
                            format,
                            "mail.send_link",
                            json!({
                                "account": resolved_account,
                                "status": "partial",
                                "partial": partial,
                            }),
                            render_mail_send_link_partial_table(&resolved_account, &partial),
                            partial.error.exit_code,
                        )
                    }
                }
            }
        }
        MailCommand::SendPublishedLink {
            account,
            to,
            cc,
            bcc,
            subject,
            body,
            html,
            public_url,
            dry_run,
        } => {
            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let request = MailSendPublishedLinkRequest {
                to: vec![to],
                cc,
                bcc,
                subject,
                text: body,
                html,
                public_url,
            };

            if dry_run {
                let review = review_mail_send_published_link(auth, &request)?;
                ok_output(
                    format,
                    "mail.send_published_link.review",
                    json!({
                        "account": resolved_account,
                        "smtp": {
                            "host": context.smtp_host,
                            "port": context.smtp_port,
                        },
                        "dry_run": true,
                        "review": review,
                    }),
                    render_mail_send_published_link_review_table(&resolved_account, &review),
                )
            } else {
                let sent = send_published_link_via_mail(
                    &context.smtp_host,
                    context.smtp_port,
                    auth,
                    &request,
                )?;
                ok_output(
                    format,
                    "mail.send_published_link",
                    json!({
                        "account": resolved_account,
                        "status": "completed",
                        "public_url": request.public_url,
                        "sent": sent,
                    }),
                    render_mail_send_published_link_table(
                        &resolved_account,
                        &request.public_url,
                        &sent,
                    ),
                )
                .inspect(|_| {
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "mail.send_published_link".to_string(),
                        account: resolved_account.clone(),
                        summary: format!(
                            "Отправлена уже опубликованная ссылка: {}",
                            request.public_url
                        ),
                        replay_command: send_published_link_command(&request, true),
                        undo: None,
                    });
                })
            }
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
        MailCommand::Attachment { action } => execute_mail_attachment(format, action),
        MailCommand::Invite { action } => execute_mail_invite(format, action),
    }
}

fn execute_mail_attachment(
    format: OutputFormat,
    action: MailAttachmentCommand,
) -> Result<RenderedOutput> {
    match action {
        MailAttachmentCommand::Export {
            account,
            folder,
            uid,
            index,
            name,
            output,
            force,
            max_bytes,
        } => {
            let selector =
                mail_attachment_selector_for_command("mail attachment export", index, name)?;

            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let exported = export_mail_attachment(
                &context.imap_host,
                context.imap_port,
                auth,
                &folder,
                MailAttachmentExportRequest {
                    uid,
                    selector,
                    output,
                    force,
                    max_bytes,
                },
            )?;

            ok_output(
                format,
                "mail.attachment.export",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "folder": folder,
                    "attachment": exported_mail_attachment_json(&exported),
                }),
                render_mail_attachment_export_table(&resolved_account, &folder, &exported),
            )
        }
    }
}

fn execute_mail_invite(format: OutputFormat, action: MailInviteCommand) -> Result<RenderedOutput> {
    match action {
        MailInviteCommand::Inspect {
            account,
            folder,
            uid,
            index,
            name,
            max_bytes,
        } => {
            let selector =
                mail_attachment_selector_for_command("mail invite inspect", index, name)?;

            let (resolved_account, auth, context) =
                resolve_mail_private_context(account.as_deref())?;
            let invite = inspect_mail_invite(
                &context.imap_host,
                context.imap_port,
                auth,
                &folder,
                MailInviteInspectRequest {
                    uid,
                    selector,
                    max_bytes,
                },
            )?;

            ok_output(
                format,
                "mail.invite.inspect",
                json!({
                    "account": resolved_account,
                    "email": context.email,
                    "folder": folder,
                    "invite": inspected_mail_invite_json(&invite),
                }),
                render_mail_invite_inspect_table(&resolved_account, &folder, &invite),
            )
        }
        MailInviteCommand::CreateEvent {
            account,
            folder,
            uid,
            index,
            name,
            calendar,
            event_index,
            max_bytes,
        } => {
            let selector =
                mail_attachment_selector_for_command("mail invite create-event", index, name)?;
            validate_positive_event_index("mail invite create-event", event_index)?;
            let (resolved_account, auth, mail_context) =
                resolve_mail_private_context(account.as_deref())?;
            let inspected = inspect_mail_invite(
                &mail_context.imap_host,
                mail_context.imap_port,
                auth,
                &folder,
                MailInviteInspectRequest {
                    uid,
                    selector: selector.clone(),
                    max_bytes,
                },
            )?;
            let (create_request, selected_invite) = calendar_create_request_from_invites(
                &calendar,
                &inspected.invites,
                event_index,
                "mail invite create-event",
            )?;
            let (_, app_password, calendar_context) =
                resolve_calendar_private_context(Some(&resolved_account))?;
            let (calendar, event) = match create_calendar_event(
                &calendar_context.caldav_base_url,
                &calendar_context.email,
                &app_password,
                create_request.clone(),
            ) {
                Ok(value) => value,
                Err(err) => {
                    let partial = invite_create_event_partial_failure(
                        &inspected,
                        &selected_invite,
                        &create_request,
                        &folder,
                        &selector,
                        max_bytes,
                        err,
                    );
                    record_activity_best_effort(NewActivityEntry {
                        source: "cli".to_string(),
                        operation: "mail.invite.create_event.partial".to_string(),
                        account: resolved_account.clone(),
                        summary: format!(
                            "Не удалось создать событие из приглашения письма {}: {}",
                            uid,
                            partial.selected_invite.summary.as_deref().unwrap_or("-")
                        ),
                        replay_command: partial.recovery.retry_calendar_step.command.clone(),
                        undo: None,
                    });
                    return non_ok_output(
                        format,
                        "mail.invite.create_event",
                        json!({
                            "status": "partial",
                            "account": resolved_account,
                            "email": mail_context.email,
                            "folder": folder,
                            "partial": partial,
                        }),
                        render_mail_invite_create_event_partial_table(
                            &resolved_account,
                            &folder,
                            event_index,
                            &partial,
                        ),
                        partial.error.exit_code,
                    );
                }
            };

            ok_output(
                format,
                "mail.invite.create_event",
                json!({
                    "account": resolved_account,
                    "email": mail_context.email,
                    "folder": folder,
                    "attachment": inspected_mail_invite_json(&inspected),
                    "selected_invite": calendar_invite_json(&selected_invite),
                    "calendar": calendar,
                    "event": calendar_event_json(&event),
                }),
                render_mail_invite_create_event_table(
                    &resolved_account,
                    &folder,
                    &inspected,
                    event_index,
                    &selected_invite,
                    &calendar,
                    &event,
                ),
            )
            .inspect(|_| {
                let replay =
                    invite_create_event_command(uid, &folder, &selector, &calendar.id, event_index);
                record_activity_best_effort(NewActivityEntry {
                    source: "cli".to_string(),
                    operation: "mail.invite.create_event".to_string(),
                    account: resolved_account.clone(),
                    summary: format!(
                        "Создано событие из приглашения письма {}: {}",
                        uid,
                        event.summary.as_deref().unwrap_or("-")
                    ),
                    replay_command: replay,
                    undo: calendar_create_undo(&calendar, &event),
                });
            })
        }
    }
}

fn mail_attachment_selector_for_command(
    command_name: &str,
    index: Option<usize>,
    name: Option<String>,
) -> Result<MailAttachmentSelector> {
    match (index, name) {
        (Some(index), None) => Ok(MailAttachmentSelector::Index(index)),
        (None, Some(name)) => Ok(MailAttachmentSelector::Filename(name)),
        (Some(_), Some(_)) => Err(YacliError::Validation(format!(
            "{command_name} accepts either --index or --name, not both"
        ))),
        (None, None) => Err(YacliError::Validation(format!(
            "{command_name} requires --index or --name"
        ))),
    }
}

fn validate_positive_event_index(command_name: &str, event_index: usize) -> Result<()> {
    if event_index == 0 {
        return Err(YacliError::Validation(format!(
            "{command_name} --event-index must be greater than zero"
        )));
    }
    Ok(())
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
            let progress = cli_transfer_progress("download", &output.display().to_string(), format);
            let request = PublicDownloadRequest {
                public_key: public_key.clone(),
                path: path.clone(),
                output,
                force,
            };
            let (resource, artifact) = if let Some(progress) = progress {
                download_public_resource_with_progress(&base_url, &request, Some(progress))?
            } else {
                download_public_resource(&base_url, &request)?
            };
            let resolved_account_for_activity = resolved_account.clone();
            let public_key_for_activity = public_key.clone();
            let path_for_activity = path.clone();

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
            .inspect(|_| {
                let mut replay = "yacli disk public download".to_string();
                if let Some(account) = resolved_account_for_activity.as_deref() {
                    replay.push_str(" --account ");
                    replay.push_str(&shell_quote(account));
                }
                replay.push_str(" --public-key ");
                replay.push_str(&shell_quote(&public_key_for_activity));
                if let Some(path) = path_for_activity.as_deref() {
                    replay.push_str(" --path ");
                    replay.push_str(&shell_quote(path));
                }
                replay.push_str(" --output ");
                replay.push_str(&shell_quote(&artifact.output_path));
                if force {
                    replay.push_str(" --force");
                }

                record_activity_best_effort(NewActivityEntry {
                    source: "cli".to_string(),
                    operation: "disk.public.download".to_string(),
                    account: resolved_account_for_activity
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                    summary: transfer_activity_summary(
                        "Скачан публичный файл Диска",
                        &artifact.output_path,
                        artifact.attempts,
                        artifact.elapsed_ms,
                        Some(artifact.resumed_from_bytes),
                    ),
                    replay_command: replay,
                    undo: None,
                });
            })
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
        exit_code: 0,
    })
}

fn non_ok_output(
    format: OutputFormat,
    operation: &'static str,
    payload: serde_json::Value,
    table: String,
    exit_code: i32,
) -> Result<RenderedOutput> {
    let Some(mut object) = payload.as_object().cloned() else {
        return Err(YacliError::Serialization(
            "expected object payload".to_string(),
        ));
    };
    object.insert("ok".to_string(), json!(false));
    object.insert("operation".to_string(), json!(operation));

    Ok(RenderedOutput {
        format,
        json: serde_json::Value::Object(object),
        table,
        exit_code,
    })
}

fn cli_transfer_progress(
    direction: &'static str,
    target: &str,
    format: OutputFormat,
) -> Option<Box<dyn FnMut(TransferProgress) + Send>> {
    if format != OutputFormat::Table || !io::stderr().is_terminal() {
        return None;
    }

    let target = target.to_string();
    let mut last_line_len = 0usize;
    Some(Box::new(move |progress: TransferProgress| {
        let total = progress
            .total_bytes
            .map(format_human_bytes)
            .unwrap_or_else(|| "unknown".to_string());
        let transferred = format_human_bytes(progress.transferred_bytes);
        let speed = format_human_bytes(progress.bytes_per_second.max(0.0) as u64);
        let percent = progress
            .total_bytes
            .filter(|total| *total > 0)
            .map(|total| {
                let ratio = progress.transferred_bytes as f64 / total as f64;
                format!("{:>5.1}%", (ratio * 100.0).min(100.0))
            })
            .unwrap_or_else(|| "  --.-%".to_string());
        let line = format!(
            "\r{} {} {} / {} ({}, {}/s)",
            if progress.finished { "done" } else { direction },
            target,
            transferred,
            total,
            percent,
            speed
        );
        let padding = if last_line_len > line.len() {
            " ".repeat(last_line_len - line.len())
        } else {
            String::new()
        };
        eprint!("{line}{padding}");
        if progress.finished {
            eprintln!();
        }
        let _ = io::stderr().flush();
        last_line_len = line.len();
    }))
}

fn format_human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
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
            path: "home",
            topic: "all",
            summary: "Показать единый home screen: onboarding, подключённые сервисы, workflows и activity.",
            requires_account: false,
            examples: vec!["yacli home", "yacli home --account work"],
        },
        GuideCommandEntry {
            path: "doctor",
            topic: "all",
            summary: "Проверить health-check продукта: конфиг, secret backend, сервисы и readiness workflows.",
            requires_account: false,
            examples: vec!["yacli doctor", "yacli doctor --account work"],
        },
        GuideCommandEntry {
            path: "next",
            topic: "all",
            summary: "Показать ranked next actions с наибольшим продуктовым эффектом.",
            requires_account: false,
            examples: vec!["yacli next", "yacli next --goal \"отправь файл по почте\""],
        },
        GuideCommandEntry {
            path: "suggest",
            topic: "all",
            summary: "Показать proactive suggestions из реальной activity history и обратимых действий.",
            requires_account: false,
            examples: vec![
                "yacli suggest",
                "yacli suggest --goal \"отправь ссылку по почте\"",
            ],
        },
        GuideCommandEntry {
            path: "goal",
            topic: "all",
            summary: "Маршрутизировать естественную цель в лучший workflow, prompt и MCP tool-path.",
            requires_account: false,
            examples: vec![
                "yacli goal \"найди приглашение и добавь событие в календарь\"",
                "yacli goal \"отправь файл по почте\" --account work",
            ],
        },
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
            path: "update",
            topic: "account",
            summary: "Проверить наличие нового release или обновить установленный yacli по месту.",
            requires_account: false,
            examples: vec!["yacli update --check", "yacli update"],
        },
        GuideCommandEntry {
            path: "status",
            topic: "auth",
            summary: "Показать, что подключено у текущего аккаунта.",
            requires_account: true,
            examples: vec!["yacli status"],
        },
        GuideCommandEntry {
            path: "activity list",
            topic: "all",
            summary: "Показать последние успешные write-действия и их replay-команды.",
            requires_account: false,
            examples: vec!["yacli activity list --limit 20"],
        },
        GuideCommandEntry {
            path: "activity show",
            topic: "all",
            summary: "Показать одну запись журнала и безопасную replay-команду.",
            requires_account: false,
            examples: vec!["yacli activity show <id>"],
        },
        GuideCommandEntry {
            path: "activity undo",
            topic: "all",
            summary: "Откатить одно обратимое действие по ID из Activity Log.",
            requires_account: false,
            examples: vec!["yacli activity undo <id>"],
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
            path: "mail attachment export",
            topic: "mail",
            summary: "Сохранить выбранное вложение письма в локальный файл.",
            requires_account: true,
            examples: vec![
                "yacli mail attachment export 1353 --index 1 --output ./invoice.pdf",
                "yacli mail attachment export 1353 --name invoice.pdf --output ./invoice.pdf",
            ],
        },
        GuideCommandEntry {
            path: "mail invite inspect",
            topic: "mail",
            summary: "Разобрать calendar invite из вложения письма и показать VEVENT поля.",
            requires_account: true,
            examples: vec![
                "yacli mail invite inspect 1353 --index 1",
                "yacli mail invite inspect 1353 --name invite.ics",
            ],
        },
        GuideCommandEntry {
            path: "mail invite create-event",
            topic: "mail",
            summary: "Создать событие в календаре из `.ics` или `text/calendar` вложения письма.",
            requires_account: true,
            examples: vec![
                "yacli mail invite create-event 1353 --index 1",
                "yacli mail invite create-event 1353 --name invite.ics --calendar team --event-index 2",
            ],
        },
        GuideCommandEntry {
            path: "mail send",
            topic: "mail",
            summary: "Отправить письмо через SMTP с OAuth XOAUTH2 или app password, при необходимости с локальными вложениями.",
            requires_account: true,
            examples: vec![
                "yacli mail send person@example.com \"Синк\" \"Привет\"",
                "yacli mail send person@example.com \"Счёт\" \"Во вложении\" --attach ./invoice.pdf",
                "yacli mail send person@example.com \"Счёт\" \"Во вложении\" --attach ./invoice.pdf --dry-run",
            ],
        },
        GuideCommandEntry {
            path: "mail send-link",
            topic: "mail",
            summary: "Загрузить большой локальный файл на Диск, опубликовать ссылку и отправить её по почте.",
            requires_account: true,
            examples: vec![
                "yacli mail send-link person@example.com \"Материалы\" \"Отправляю ссылку\" --source ./archive.zip --path disk:/docs/archive/archive.zip",
                "yacli mail send-link person@example.com \"Материалы\" \"Отправляю ссылку\" --source ./archive.zip --path disk:/docs/archive/archive.zip --dry-run",
            ],
        },
        GuideCommandEntry {
            path: "mail send-published-link",
            topic: "mail",
            summary: "Повторно отправить письмо с уже опубликованной ссылкой без повторного upload/publish.",
            requires_account: true,
            examples: vec![
                "yacli mail send-published-link person@example.com \"Материалы\" \"Отправляю ссылку\" --public-url https://disk.yandex.ru/i/archive-link",
                "yacli mail send-published-link person@example.com \"Материалы\" \"Отправляю ссылку\" --public-url https://disk.yandex.ru/i/archive-link --dry-run",
            ],
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
                "yacli calendar create \"Синк\" 2026-03-12T09:00:00Z 2026-03-12T10:00:00Z --dry-run",
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
            examples: vec![
                "yacli disk upload ./report.pdf disk:/docs/report.pdf",
                "yacli disk upload ./report.pdf disk:/docs/report.pdf --dry-run",
            ],
        },
        GuideCommandEntry {
            path: "disk upload-link",
            topic: "disk",
            summary: "Загрузить локальный файл на Диск и сразу получить public URL / public key.",
            requires_account: true,
            examples: vec![
                "yacli disk upload-link --source ./report.pdf --path disk:/docs/report.pdf",
                "yacli disk upload-link --source ./report.pdf --path disk:/docs/report.pdf --dry-run",
            ],
        },
        GuideCommandEntry {
            path: "disk download",
            topic: "disk",
            summary: "Скачать файл из приватного Яндекс Диска в локальный путь.",
            requires_account: true,
            examples: vec!["yacli disk download disk:/docs/report.pdf --output ./report.pdf"],
        },
        GuideCommandEntry {
            path: "disk publish",
            topic: "disk",
            summary: "Опубликовать приватный файл или папку и получить public URL / public key.",
            requires_account: true,
            examples: vec!["yacli disk publish disk:/docs/report.pdf"],
        },
        GuideCommandEntry {
            path: "disk unpublish",
            topic: "disk",
            summary: "Отозвать public URL / public key у приватного файла или папки.",
            requires_account: true,
            examples: vec!["yacli disk unpublish disk:/docs/report.pdf"],
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
            id: "mail_attachment_export_flow",
            topic: "mail",
            title: "Скачать вложение из письма",
            summary: "Поток от поиска письма до сохранения выбранного вложения в локальный файл.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail search \"счёт\"",
                "yacli mail read <id>",
                "yacli mail attachment export <id> --index 1 --output ./invoice.pdf",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_invite_inspect_flow",
            topic: "mail",
            title: "Разобрать приглашение из письма",
            summary: "Поток от поиска письма до чтения calendar invite из `.ics` или `text/calendar` вложения.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail search \"приглашение\"",
                "yacli mail read <id>",
                "yacli mail invite inspect <id> --index 1",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_invite_create_event_flow",
            topic: "mail",
            title: "Создать событие из приглашения в письме",
            summary: "Поток от поиска письма до импорта выбранного VEVENT в календарь через CalDAV.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli login calendar --app-password <app-password>",
                "yacli mail search \"приглашение\"",
                "yacli mail invite create-event <id> --index 1",
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
            steps: vec!["yacli add me@yandex.ru", "yacli login", "yacli disk list"],
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
            title: "Отправить письмо с вложением",
            summary: "Поток от OAuth логина до отправки письма через SMTP, включая локальный файл во вложении.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail send person@example.com \"Синк\" \"Привет\" --attach ./report.pdf",
            ],
        },
        GuideWorkflowEntry {
            id: "mail_send_link_flow",
            topic: "mail",
            title: "Отправить ссылку на большой файл",
            summary: "Поток от OAuth логина до загрузки файла на Диск, публикации ссылки и отправки письма со ссылкой.",
            steps: vec![
                "yacli add me@yandex.ru",
                "yacli login",
                "yacli mail send-link person@example.com \"Материалы\" \"Отправляю ссылку\" --source ./archive.zip --path disk:/docs/archive/archive.zip",
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

fn oauth_login_pending_output(
    format: OutputFormat,
    account_name: &str,
    services: &[OauthService],
    client_id: &str,
    client_id_source: &str,
    authorization: crate::oauth::AuthorizationRequest,
    session_reused: bool,
) -> Result<RenderedOutput> {
    let resume_command =
        oauth_login_resume_command(account_name, services, client_id_source, client_id);
    let service_names = services
        .iter()
        .map(|service| service.as_str())
        .collect::<Vec<_>>();
    let mut payload = serde_json::Map::new();
    payload.insert("account".to_string(), json!(account_name));
    payload.insert("status".to_string(), json!("pending"));
    payload.insert("services".to_string(), json!(service_names));
    payload.insert("client_id_source".to_string(), json!(client_id_source));
    payload.insert("authorization".to_string(), json!(authorization));
    payload.insert("session_reused".to_string(), json!(session_reused));
    payload.insert("resume_command".to_string(), json!(resume_command.clone()));
    if services.len() == 1 {
        payload.insert("service".to_string(), json!(services[0].as_str()));
    }

    ok_output(
        format,
        "auth.login",
        serde_json::Value::Object(payload),
        render_key_value_table(&[
            ("operation", "auth.login".to_string()),
            ("account", account_name.to_string()),
            (
                if services.len() == 1 {
                    "service"
                } else {
                    "services"
                },
                if services.len() == 1 {
                    services[0].as_str().to_string()
                } else {
                    services
                        .iter()
                        .map(|service| service.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                },
            ),
            ("status", "pending".to_string()),
            ("session_reused", session_reused.to_string()),
            ("client_id_source", client_id_source.to_string()),
            ("authorization_url", authorization.authorization_url),
            ("resume_command", resume_command),
        ]),
    )
}

fn oauth_login_resume_command(
    account_name: &str,
    services: &[OauthService],
    client_id_source: &str,
    client_id: &str,
) -> String {
    let mut command = format!("yacli login --account {}", shell_quote(account_name));
    if services.len() == 1 {
        command.push_str(&format!(" --service {}", services[0].as_str()));
    }
    if client_id_source == "override" {
        command.push_str(&format!(" --client-id {}", shell_quote(client_id)));
    }
    command.push_str(" --code <код>");
    command
}

fn render_key_value_table(items: &[(&str, String)]) -> String {
    items
        .iter()
        .map(|(key, value)| format!("{key}\t{value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_setup_table(payload: &serde_json::Value) -> String {
    let account = payload["account"]["account"].as_str().unwrap_or_default();
    let email = payload["account"]["email"].as_str().unwrap_or_default();
    let plan_only = payload["plan_only"].as_bool().unwrap_or(false);

    let mut lines = vec![
        format!("ACCOUNT\t{account}"),
        format!("EMAIL\t{email}"),
        format!("PLAN_ONLY\t{plan_only}"),
        "STEP\tSTATUS\tDETAIL".to_string(),
    ];

    if let Some(steps) = payload["steps"].as_array() {
        for step in steps {
            lines.push(format!(
                "{}\t{}\t{}",
                step["id"].as_str().unwrap_or_default(),
                step["status"].as_str().unwrap_or_default(),
                step["detail"].as_str().unwrap_or_default()
            ));
        }
    }

    if let Some(actions) = payload["next_actions"].as_array()
        && !actions.is_empty()
    {
        lines.push("NEXT_ACTIONS".to_string());
        for action in actions {
            lines.push(action.as_str().unwrap_or_default().to_string());
        }
    }

    lines.join("\n")
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
        (
            "resumed_from_bytes",
            artifact.resumed_from_bytes.to_string(),
        ),
        ("bytes_written", artifact.bytes_written.to_string()),
        ("attempts", artifact.attempts.to_string()),
        ("elapsed_ms", artifact.elapsed_ms.to_string()),
        ("sha256", artifact.sha256.clone()),
    ])
}

fn render_disk_download_table(
    account: &str,
    resource: &DiskResource,
    artifact: &DownloadedFile,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("resource.path", resource.path.clone()),
        ("resource.name", resource.name.clone()),
        ("resource_type", resource.resource_type.clone()),
        ("output.path", artifact.output_path.clone()),
        (
            "resumed_from_bytes",
            artifact.resumed_from_bytes.to_string(),
        ),
        ("bytes_written", artifact.bytes_written.to_string()),
        ("attempts", artifact.attempts.to_string()),
        ("elapsed_ms", artifact.elapsed_ms.to_string()),
        ("sha256", artifact.sha256.clone()),
    ])
}

fn render_disk_publish_table(account: &str, resource: &DiskResource) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("resource.path", resource.path.clone()),
        ("resource.name", resource.name.clone()),
        ("resource_type", resource.resource_type.clone()),
        (
            "public_url",
            resource
                .public_url
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "public_key",
            resource
                .public_key
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_disk_publish_review_table(account: &str, review: &DiskPublishReview) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("resource.path", review.path.clone()),
        ("resource.name", review.resource_name.clone()),
        ("resource_type", review.resource_type.clone()),
        ("dry_run", "true".to_string()),
        (
            "already_public",
            if review.already_public { "yes" } else { "no" }.to_string(),
        ),
        (
            "current_public_url",
            review
                .current_public_url
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "current_public_key",
            review
                .current_public_key
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_disk_unpublish_table(account: &str, result: &UnpublishedDiskResource) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("resource.path", result.resource.path.clone()),
        ("resource.name", result.resource.name.clone()),
        ("resource_type", result.resource.resource_type.clone()),
        (
            "was_public",
            if result.was_public { "yes" } else { "no" }.to_string(),
        ),
        (
            "revoked_public_url",
            result
                .revoked_public_url
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "revoked_public_key",
            result
                .revoked_public_key
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "current_public_url",
            result
                .resource
                .public_url
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "current_public_key",
            result
                .resource
                .public_key
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_disk_unpublish_review_table(account: &str, review: &DiskUnpublishReview) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("resource.path", review.path.clone()),
        ("resource.name", review.resource_name.clone()),
        ("resource_type", review.resource_type.clone()),
        ("dry_run", "true".to_string()),
        (
            "is_public",
            if review.is_public { "yes" } else { "no" }.to_string(),
        ),
        (
            "current_public_url",
            review
                .current_public_url
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "current_public_key",
            review
                .current_public_key
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
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
        ("attempts", uploaded.attempts.to_string()),
        ("elapsed_ms", uploaded.elapsed_ms.to_string()),
        ("sha256", uploaded.sha256.clone()),
        ("overwrite", uploaded.overwrite.to_string()),
    ])
}

fn render_disk_upload_review_table(account: &str, review: &DiskUploadReview) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("dry_run", "true".to_string()),
        ("source.path", review.source_path.clone()),
        ("remote.path", review.remote_path.clone()),
        ("bytes_written", review.bytes_written.to_string()),
        ("sha256", review.sha256.clone()),
        ("overwrite", review.overwrite.to_string()),
    ])
}

fn render_disk_upload_link_table(account: &str, result: &DiskUploadLinkResult) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("source.path", result.upload.source_path.clone()),
        ("resource.path", result.resource.path.clone()),
        ("resource.name", result.resource.name.clone()),
        ("resource_type", result.resource.resource_type.clone()),
        ("bytes_written", result.upload.bytes_written.to_string()),
        ("attempts", result.upload.attempts.to_string()),
        ("elapsed_ms", result.upload.elapsed_ms.to_string()),
        ("sha256", result.upload.sha256.clone()),
        (
            "public_url",
            result
                .resource
                .public_url
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "public_key",
            result
                .resource
                .public_key
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
    ])
}

fn render_disk_upload_link_review_table(account: &str, review: &DiskUploadLinkReview) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("dry_run", "true".to_string()),
        ("source.path", review.upload.source_path.clone()),
        ("remote.path", review.upload.remote_path.clone()),
        ("bytes_written", review.upload.bytes_written.to_string()),
        ("sha256", review.upload.sha256.clone()),
        ("overwrite", review.upload.overwrite.to_string()),
        ("publish_path", review.publish_path.clone()),
        ("public_url", review.link_placeholder.clone()),
    ])
}

fn render_activity_list_table(entries: &[ActivityEntry]) -> String {
    let mut lines = vec![
        format!("count\t{}", entries.len()),
        "ID\tAT\tSOURCE\tOPERATION\tACCOUNT\tUNDO\tSUMMARY".to_string(),
    ];
    lines.extend(entries.iter().map(|entry| {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            sanitize_table_cell(&entry.id),
            sanitize_table_cell(&entry.occurred_at),
            sanitize_table_cell(&entry.source),
            sanitize_table_cell(&entry.operation),
            sanitize_table_cell(&entry.account),
            if entry.undo.is_some() { "yes" } else { "-" },
            sanitize_table_cell(&entry.summary),
        )
    }));
    lines.join("\n")
}

fn render_activity_show_table(entry: &ActivityEntry) -> String {
    let mut rows = vec![
        ("id", entry.id.clone()),
        ("occurred_at", entry.occurred_at.clone()),
        ("source", entry.source.clone()),
        ("operation", entry.operation.clone()),
        ("account", entry.account.clone()),
        ("summary", entry.summary.clone()),
        ("replay_command", entry.replay_command.clone()),
    ];
    if let Some(undo_command) = entry.undo_command.clone() {
        rows.push(("undo_command", undo_command));
    }
    render_key_value_table(&rows)
}

fn render_activity_undo_table(applied: &ActivityUndoApplied) -> String {
    let mut rows = vec![
        ("original_activity_id", applied.original_activity_id.clone()),
        ("original_operation", applied.original_operation.clone()),
        ("account", applied.account.clone()),
        ("summary", applied.summary.clone()),
        ("replay_command", applied.replay_command.clone()),
    ];

    match &applied.result {
        ActivityUndoResult::CalendarDelete {
            calendar,
            deleted_event,
        } => {
            rows.push(("undo_kind", "calendar.delete".to_string()));
            rows.push(("calendar", calendar.name.clone()));
            rows.push((
                "uid",
                deleted_event.uid.clone().unwrap_or_else(|| "-".to_string()),
            ));
            rows.push((
                "deleted_summary",
                deleted_event
                    .summary
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
            ));
        }
        ActivityUndoResult::DiskUnpublish { result } => {
            rows.push(("undo_kind", "disk.unpublish".to_string()));
            rows.push(("path", result.resource.path.clone()));
            rows.push((
                "revoked_public_url",
                result
                    .revoked_public_url
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
            ));
        }
    }

    render_key_value_table(&rows)
}

fn render_home_table(payload: &serde_json::Value) -> String {
    let mut lines = vec![
        format!("STATUS\t{}", payload["status"].as_str().unwrap_or_default()),
        format!(
            "ACCOUNT\t{}",
            payload["current_account"].as_str().unwrap_or("-")
        ),
        format!("EMAIL\t{}", payload["email"].as_str().unwrap_or("-")),
        format!(
            "WORKFLOW_COUNT\t{}",
            payload["workflow_count"].as_u64().unwrap_or(0)
        ),
        format!(
            "RECENT_ACTIVITY_COUNT\t{}",
            payload["recent_activity_count"].as_u64().unwrap_or(0)
        ),
        format!(
            "SUGGESTION_COUNT\t{}",
            payload["suggestions"]["count"].as_u64().unwrap_or(0)
        ),
    ];

    if let Some(goal) = payload["goal"].as_str()
        && !goal.is_empty()
    {
        lines.push(format!("GOAL\t{goal}"));
        lines.push(format!(
            "GOAL_STATUS\t{}",
            payload["goal_route"]["status"].as_str().unwrap_or("-")
        ));
        lines.push(format!(
            "GOAL_REMEDIATION\t{}",
            payload["goal_route"]["remediation"]["status"]
                .as_str()
                .unwrap_or("-")
        ));
    }

    if let Some(latest_activity) = payload["latest_activity"].as_object() {
        lines.push(format!(
            "LATEST_ACTIVITY\t{}",
            latest_activity
                .get("summary")
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        ));
        lines.push(format!(
            "LATEST_REPLAY\t{}",
            latest_activity
                .get("replay_command")
                .and_then(|value| value.as_str())
                .unwrap_or("-")
        ));
    }

    lines.push("SERVICES".to_string());
    lines.push("SERVICE\tSTATUS\tDETAIL".to_string());
    if let Some(services) = payload["services"].as_object() {
        for service in ["mail", "calendar", "disk"] {
            if let Some(state) = services.get(service) {
                lines.push(format!(
                    "{}\t{}\t{}",
                    service_label(service),
                    credential_state_label(
                        state["credential_state"]
                            .as_str()
                            .unwrap_or("not_configured")
                    ),
                    state["detail"].as_str().unwrap_or_default()
                ));
            }
        }
    }

    lines.push("HIGHLIGHTED_WORKFLOWS".to_string());
    lines.push("ID\tTITLE\tCONNECTS".to_string());
    if let Some(workflows) = payload["highlighted_workflows"].as_array() {
        for workflow in workflows {
            lines.push(format!(
                "{}\t{}\t{}",
                workflow["id"].as_str().unwrap_or_default(),
                workflow["title"].as_str().unwrap_or_default(),
                workflow["connects"].as_str().unwrap_or_default()
            ));
        }
    }

    if let Some(commands) = payload["suggested_commands"].as_array()
        && !commands.is_empty()
    {
        lines.push("SUGGESTED_COMMANDS".to_string());
        for command in commands {
            lines.push(command.as_str().unwrap_or_default().to_string());
        }
    }

    if let Some(items) = payload["suggestions"]["suggestions"].as_array()
        && !items.is_empty()
    {
        lines.push("PROACTIVE_SUGGESTIONS".to_string());
        lines.push("TITLE\tKIND\tCOMMAND".to_string());
        for item in items.iter().take(3) {
            lines.push(format!(
                "{}\t{}\t{}",
                item["title"].as_str().unwrap_or_default(),
                item["kind"].as_str().unwrap_or_default(),
                item["command"].as_str().unwrap_or_default()
            ));
        }
    }

    if payload["safe_remediation"].is_object() {
        lines.push("SAFE_REMEDIATION".to_string());
        lines.push(format!(
            "STATUS\t{}",
            payload["safe_remediation"]["status"]
                .as_str()
                .unwrap_or_default()
        ));
        lines.push(format!(
            "APPLIED\t{}",
            payload["safe_remediation"]["applied_count"]
                .as_u64()
                .unwrap_or(0)
        ));
        lines.push(format!(
            "NEEDS_INPUT\t{}",
            payload["safe_remediation"]["needs_input_count"]
                .as_u64()
                .unwrap_or(0)
        ));
        if let Some(steps) = payload["safe_remediation"]["steps"].as_array()
            && !steps.is_empty()
        {
            lines.push("ID\tTITLE\tSTATUS\tCOMMAND".to_string());
            for step in steps {
                lines.push(format!(
                    "{}\t{}\t{}\t{}",
                    step["id"].as_str().unwrap_or_default(),
                    step["title"].as_str().unwrap_or_default(),
                    step["status"].as_str().unwrap_or_default(),
                    step["command"].as_str().unwrap_or("-"),
                ));
            }
        }
    }

    lines.join("\n")
}

fn render_doctor_table(payload: &serde_json::Value) -> String {
    let mut lines = vec![
        format!("STATUS\t{}", payload["status"].as_str().unwrap_or_default()),
        format!(
            "ACCOUNT\t{}",
            payload["current_account"].as_str().unwrap_or("-")
        ),
        format!("EMAIL\t{}", payload["email"].as_str().unwrap_or("-")),
        format!(
            "SECRET_BACKEND\t{}",
            payload["config"]["secretBackend"].as_str().unwrap_or("-")
        ),
        format!(
            "CONFIG_DIR\t{}",
            payload["config"]["dir"].as_str().unwrap_or("-")
        ),
    ];

    if let Some(goal) = payload["goal"].as_str()
        && !goal.is_empty()
    {
        lines.push(format!("GOAL\t{goal}"));
        lines.push(format!(
            "GOAL_REMEDIATION\t{}",
            payload["goal_route"]["remediation"]["status"]
                .as_str()
                .unwrap_or("-")
        ));
    }

    lines.push("CHECKS".to_string());
    lines.push("ID\tTITLE\tSTATUS\tDETAIL".to_string());
    if let Some(checks) = payload["checks"].as_array() {
        for check in checks {
            lines.push(format!(
                "{}\t{}\t{}\t{}",
                check["id"].as_str().unwrap_or_default(),
                check["title"].as_str().unwrap_or_default(),
                check["status"].as_str().unwrap_or_default(),
                sanitize_table_cell(check["detail"].as_str().unwrap_or_default()),
            ));
        }
    }

    if let Some(checks) = payload["focus_checks"].as_array()
        && !checks.is_empty()
    {
        lines.push("GOAL_FOCUS_CHECKS".to_string());
        lines.push("ID\tTITLE\tSTATUS".to_string());
        for check in checks {
            lines.push(format!(
                "{}\t{}\t{}",
                check["id"].as_str().unwrap_or_default(),
                check["title"].as_str().unwrap_or_default(),
                check["status"].as_str().unwrap_or_default(),
            ));
        }
    }

    if let Some(commands) = payload["suggested_commands"].as_array()
        && !commands.is_empty()
    {
        lines.push("SUGGESTED_COMMANDS".to_string());
        for command in commands {
            lines.push(command.as_str().unwrap_or_default().to_string());
        }
    }

    lines.join("\n")
}

fn render_next_actions_table(payload: &serde_json::Value) -> String {
    let mut lines = vec![
        format!("STATUS\t{}", payload["status"].as_str().unwrap_or_default()),
        format!(
            "ACCOUNT\t{}",
            payload["current_account"].as_str().unwrap_or("-")
        ),
        format!("COUNT\t{}", payload["count"].as_u64().unwrap_or(0)),
        format!(
            "SUMMARY\t{}",
            sanitize_table_cell(payload["summary"].as_str().unwrap_or_default())
        ),
        "ACTIONS".to_string(),
        "ID\tTITLE\tSTATUS\tCOMMAND".to_string(),
    ];

    if let Some(goal) = payload["goal"].as_str()
        && !goal.is_empty()
    {
        lines.insert(3, format!("GOAL\t{goal}"));
        lines.insert(
            4,
            format!(
                "GOAL_REMEDIATION\t{}",
                payload["goal_route"]["remediation"]["status"]
                    .as_str()
                    .unwrap_or("-")
            ),
        );
    }

    if let Some(actions) = payload["actions"].as_array() {
        for action in actions {
            lines.push(format!(
                "{}\t{}\t{}\t{}",
                action["id"].as_str().unwrap_or_default(),
                action["title"].as_str().unwrap_or_default(),
                action["status"].as_str().unwrap_or_default(),
                action["command"].as_str().unwrap_or_default()
            ));
        }
    }

    lines.join("\n")
}

fn render_suggestions_table(payload: &serde_json::Value) -> String {
    let mut lines = vec![
        format!("STATUS\t{}", payload["status"].as_str().unwrap_or_default()),
        format!(
            "ACCOUNT\t{}",
            payload["current_account"].as_str().unwrap_or("-")
        ),
        format!("COUNT\t{}", payload["count"].as_u64().unwrap_or(0)),
        format!(
            "SUMMARY\t{}",
            sanitize_table_cell(payload["summary"].as_str().unwrap_or_default())
        ),
        "SUGGESTIONS".to_string(),
        "ID\tTITLE\tKIND\tCOMMAND".to_string(),
    ];

    if let Some(goal) = payload["goal"].as_str()
        && !goal.is_empty()
    {
        lines.insert(3, format!("GOAL\t{goal}"));
    }

    if let Some(items) = payload["suggestions"].as_array() {
        for item in items {
            lines.push(format!(
                "{}\t{}\t{}\t{}",
                item["id"].as_str().unwrap_or_default(),
                item["title"].as_str().unwrap_or_default(),
                item["kind"].as_str().unwrap_or_default(),
                item["command"].as_str().unwrap_or_default(),
            ));
        }
    }

    lines.join("\n")
}

fn render_goal_table(payload: &serde_json::Value) -> String {
    let mut lines = vec![
        format!("STATUS\t{}", payload["status"].as_str().unwrap_or_default()),
        format!("QUERY\t{}", payload["query"].as_str().unwrap_or_default()),
        format!(
            "REMEDIATION_STATUS\t{}",
            payload["remediation"]["status"]
                .as_str()
                .unwrap_or_default()
        ),
        format!(
            "SUGGESTED_COMMAND\t{}",
            payload["suggested_command"].as_str().unwrap_or_default()
        ),
    ];

    if payload["best_match"].is_object() {
        lines.push(format!(
            "BEST_WORKFLOW\t{}",
            payload["best_match"]["workflow"]["id"]
                .as_str()
                .unwrap_or_default()
        ));
        lines.push(format!(
            "BEST_PROMPT\t{}",
            payload["best_match"]["route"]["prompt_name"]
                .as_str()
                .unwrap_or_default()
        ));
        lines.push(format!(
            "BEST_TOOL\t{}",
            payload["best_match"]["route"]["best_tool"]
                .as_str()
                .unwrap_or_default()
        ));
    }

    if let Some(hints) = payload["hints"].as_object() {
        let hint_rows = [
            ("GOAL_EMAIL", hints.get("email")),
            ("GOAL_LOCAL_PATH", hints.get("local_path")),
            ("GOAL_DISK_PATH", hints.get("disk_path")),
            ("GOAL_QUOTED_TEXT", hints.get("quoted_text")),
            ("GOAL_CALENDAR", hints.get("calendar")),
        ];
        for (label, value) in hint_rows {
            if let Some(text) = value.and_then(serde_json::Value::as_str)
                && !text.is_empty()
            {
                lines.push(format!("{label}\t{text}"));
            }
        }
    }

    lines.push("RECOMMENDATIONS".to_string());
    lines.push("ID\tTITLE\tSCORE\tTOOL".to_string());
    if let Some(items) = payload["recommendations"].as_array() {
        for item in items {
            lines.push(format!(
                "{}\t{}\t{}\t{}",
                item["workflow"]["id"].as_str().unwrap_or_default(),
                item["workflow"]["title"].as_str().unwrap_or_default(),
                item["score"].as_u64().unwrap_or(0),
                item["route"]["best_tool"].as_str().unwrap_or_default()
            ));
        }
    }

    if let Some(actions) = payload["remediation"]["actions"].as_array()
        && !actions.is_empty()
    {
        lines.push("REMEDIATION".to_string());
        lines.push("ID\tTITLE\tCOMMAND".to_string());
        for action in actions {
            lines.push(format!(
                "{}\t{}\t{}",
                action["id"].as_str().unwrap_or_default(),
                action["title"].as_str().unwrap_or_default(),
                action["command"].as_str().unwrap_or_default()
            ));
        }
    }

    lines.join("\n")
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

fn render_mail_send_review_table(account: &str, review: &MailSendReview) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("from\t{}", review.sent.from),
        format!("to\t{}", review.sent.to.join(", ")),
        format!(
            "cc\t{}",
            if review.sent.cc.is_empty() {
                "-".to_string()
            } else {
                review.sent.cc.join(", ")
            }
        ),
        format!("bcc_count\t{}", review.sent.bcc_count),
        format!("subject\t{}", review.sent.subject),
        format!("body_kind\t{}", review.sent.body_kind),
        format!("message_bytes\t{}", review.message_bytes),
        format!("delivery_posture\t{}", review.delivery_posture),
        format!("attachment_count\t{}", review.attachment_count),
        "dry_run\ttrue".to_string(),
    ];

    if review.attachments.is_empty() {
        lines.push("attachments\t-".to_string());
    } else {
        lines.push("attachments".to_string());
        lines.push("FILENAME\tMIME\tINLINE\tBYTES\tCONTENT_ID".to_string());
        lines.extend(review.attachments.iter().map(|attachment| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                attachment.filename.as_deref().unwrap_or("-"),
                attachment.mime_type,
                attachment.inline,
                attachment.bytes,
                attachment.content_id.as_deref().unwrap_or("-")
            )
        }));
    }

    if let Some(remediation) = &review.remediation {
        lines.push("remediation".to_string());
        lines.push("WORKFLOW\tREASON\tSUGGESTED_DISK_PATH".to_string());
        lines.push(format!(
            "{}\t{}\t{}",
            remediation.workflow,
            remediation.reason,
            remediation.suggested_disk_path.as_deref().unwrap_or("-")
        ));
    }

    lines.join("\n")
}

fn render_mail_send_link_review_table(account: &str, review: &MailSendLinkReview) -> String {
    let lines = vec![
        format!("account\t{account}"),
        format!("source_path\t{}", review.upload.source_path),
        format!("remote_path\t{}", review.upload.remote_path),
        format!("overwrite\t{}", review.upload.overwrite),
        format!("bytes_written\t{}", review.upload.bytes_written),
        format!("sha256\t{}", review.upload.sha256),
        format!("link_placeholder\t{}", review.link_placeholder),
        format!("to\t{}", review.mail_review.sent.to.join(", ")),
        format!(
            "cc\t{}",
            if review.mail_review.sent.cc.is_empty() {
                "-".to_string()
            } else {
                review.mail_review.sent.cc.join(", ")
            }
        ),
        format!("bcc_count\t{}", review.mail_review.sent.bcc_count),
        format!("subject\t{}", review.mail_review.sent.subject),
        format!("body_kind\t{}", review.mail_review.sent.body_kind),
        format!("attachment_count\t{}", review.mail_review.attachment_count),
    ];

    lines.join("\n")
}

fn render_mail_send_published_link_review_table(
    account: &str,
    review: &MailSendPublishedLinkReview,
) -> String {
    let lines = [
        format!("account\t{account}"),
        format!("public_url\t{}", review.public_url),
        format!("to\t{}", review.mail_review.sent.to.join(", ")),
        format!(
            "cc\t{}",
            if review.mail_review.sent.cc.is_empty() {
                "-".to_string()
            } else {
                review.mail_review.sent.cc.join(", ")
            }
        ),
        format!("bcc_count\t{}", review.mail_review.sent.bcc_count),
        format!("subject\t{}", review.mail_review.sent.subject),
        format!("body_kind\t{}", review.mail_review.sent.body_kind),
        format!("attachment_count\t{}", review.mail_review.attachment_count),
    ];

    lines.join("\n")
}

fn render_mail_send_link_table(account: &str, result: &MailSendLinkResult) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("source_path", result.upload.source_path.clone()),
        ("remote_path", result.upload.remote_path.clone()),
        ("bytes_written", result.upload.bytes_written.to_string()),
        ("sha256", result.upload.sha256.clone()),
        ("attempts", result.upload.attempts.to_string()),
        ("elapsed_ms", result.upload.elapsed_ms.to_string()),
        (
            "public_url",
            result
                .resource
                .public_url
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "public_key",
            result
                .resource
                .public_key
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        ("to", result.sent.to.join(", ")),
        (
            "cc",
            if result.sent.cc.is_empty() {
                "-".to_string()
            } else {
                result.sent.cc.join(", ")
            },
        ),
        ("bcc_count", result.sent.bcc_count.to_string()),
        ("subject", result.sent.subject.clone()),
        ("message_id", result.sent.message_id.clone()),
        ("body_kind", result.sent.body_kind.clone()),
    ])
}

fn render_mail_send_published_link_table(
    account: &str,
    public_url: &str,
    sent: &SentMail,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("public_url", public_url.to_string()),
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

fn render_mail_send_link_partial_table(
    account: &str,
    partial: &MailSendLinkPartialFailure,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("status", "partial".to_string()),
        ("failed_stage", partial.failed_stage.clone()),
        ("source_path", partial.upload.source_path.clone()),
        ("remote_path", partial.upload.remote_path.clone()),
        (
            "public_url",
            partial.recovery.share_public_link.public_url.clone(),
        ),
        (
            "public_key",
            partial
                .recovery
                .share_public_link
                .public_key
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        ("error_code", partial.error.code.to_string()),
        ("error_message", partial.error.message.clone()),
        (
            "retry_mail_command",
            partial.recovery.retry_mail_step.command.clone(),
        ),
        (
            "cleanup_public_link_command",
            partial.recovery.cleanup_public_link.command.clone(),
        ),
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

fn render_mail_attachment_export_table(
    account: &str,
    folder: &str,
    attachment: &ExportedMailAttachment,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("folder", folder.to_string()),
        ("message_id", attachment.message_uid.to_string()),
        ("attachment_index", attachment.attachment_index.to_string()),
        (
            "filename",
            attachment.filename.as_deref().unwrap_or("-").to_string(),
        ),
        ("mime_type", attachment.mime_type.clone()),
        (
            "content_id",
            attachment.content_id.as_deref().unwrap_or("-").to_string(),
        ),
        ("inline", attachment.inline.to_string()),
        ("output_path", attachment.output_path.clone()),
        ("bytes_written", attachment.bytes_written.to_string()),
        ("sha256", attachment.sha256.clone()),
    ])
}

fn render_mail_invite_inspect_table(
    account: &str,
    folder: &str,
    invite: &InspectedMailInvite,
) -> String {
    let mut lines = vec![
        format!("account\t{account}"),
        format!("folder\t{folder}"),
        format!("message_id\t{}", invite.message_uid),
        format!("attachment_index\t{}", invite.attachment_index),
        format!("filename\t{}", invite.filename.as_deref().unwrap_or("-")),
        format!("mime_type\t{}", invite.mime_type),
        format!(
            "content_id\t{}",
            invite.content_id.as_deref().unwrap_or("-")
        ),
        format!("inline\t{}", invite.inline),
        format!("invite_count\t{}", invite.invites.len()),
    ];
    if invite.invites.is_empty() {
        lines.push("invites\t-".to_string());
    } else {
        lines.push("invites".to_string());
        lines.push("UID\tSUMMARY\tSTART\tEND\tLOCATION\tSTATUS\tALL_DAY".to_string());
        lines.extend(invite.invites.iter().map(|entry| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                entry.uid.as_deref().unwrap_or("-"),
                entry.summary.as_deref().unwrap_or("-"),
                entry.start.as_deref().unwrap_or("-"),
                entry.end.as_deref().unwrap_or("-"),
                entry.location.as_deref().unwrap_or("-"),
                entry.status.as_deref().unwrap_or("-"),
                entry.all_day
            )
        }));
    }
    lines.join("\n")
}

fn render_mail_invite_create_event_table(
    account: &str,
    folder: &str,
    attachment: &InspectedMailInvite,
    event_index: usize,
    invite: &CalendarInvite,
    calendar: &CalendarCollection,
    event: &CalendarEvent,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("folder", folder.to_string()),
        ("message_id", attachment.message_uid.to_string()),
        ("attachment_index", attachment.attachment_index.to_string()),
        ("invite_event_index", event_index.to_string()),
        (
            "invite_uid",
            invite.uid.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "invite_summary",
            invite.summary.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "invite_start",
            invite.start.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "invite_end",
            invite.end.as_deref().unwrap_or("-").to_string(),
        ),
        ("calendar_id", calendar.id.clone()),
        ("calendar_name", calendar.name.clone()),
        ("event_id", event.uid.as_deref().unwrap_or("-").to_string()),
        ("event_href", event.href.clone()),
        (
            "event_summary",
            event.summary.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "event_start",
            event.start.as_deref().unwrap_or("-").to_string(),
        ),
        ("event_end", event.end.as_deref().unwrap_or("-").to_string()),
    ])
}

fn render_mail_invite_create_event_partial_table(
    account: &str,
    folder: &str,
    event_index: usize,
    partial: &MailInviteCreateEventPartialFailure,
) -> String {
    render_key_value_table(&[
        ("status", "partial".to_string()),
        ("account", account.to_string()),
        ("folder", folder.to_string()),
        ("message_id", partial.attachment.message_uid.to_string()),
        (
            "attachment_index",
            partial.attachment.attachment_index.to_string(),
        ),
        ("invite_event_index", event_index.to_string()),
        (
            "invite_uid",
            partial
                .selected_invite
                .uid
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "invite_summary",
            partial
                .selected_invite
                .summary
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        ("calendar_id", partial.create_request.calendar.clone()),
        ("failed_stage", partial.failed_stage.clone()),
        ("error_code", partial.error.code.to_string()),
        ("error_message", partial.error.message.clone()),
        (
            "retry_calendar_step_command",
            partial.recovery.retry_calendar_step.command.clone(),
        ),
        (
            "inspect_invite_command",
            partial.recovery.inspect_invite.command.clone(),
        ),
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

fn exported_mail_attachment_json(attachment: &ExportedMailAttachment) -> serde_json::Value {
    json!({
        "message_id": attachment.message_uid,
        "attachment_index": attachment.attachment_index,
        "filename": attachment.filename,
        "mime_type": attachment.mime_type,
        "content_id": attachment.content_id,
        "inline": attachment.inline,
        "output_path": attachment.output_path,
        "bytes_written": attachment.bytes_written,
        "sha256": attachment.sha256,
    })
}

fn calendar_invite_json(invite: &CalendarInvite) -> serde_json::Value {
    json!({
        "uid": invite.uid,
        "summary": invite.summary,
        "start": invite.start,
        "end": invite.end,
        "description": invite.description,
        "location": invite.location,
        "status": invite.status,
        "all_day": invite.all_day,
    })
}

fn inspected_mail_invite_json(invite: &InspectedMailInvite) -> serde_json::Value {
    json!({
        "message_id": invite.message_uid,
        "attachment_index": invite.attachment_index,
        "filename": invite.filename,
        "mime_type": invite.mime_type,
        "content_id": invite.content_id,
        "inline": invite.inline,
        "invites": invite.invites.iter().map(calendar_invite_json).collect::<Vec<_>>(),
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

fn render_calendar_create_review_table(
    account: &str,
    calendar: &CalendarCollection,
    review: &CalendarCreateReview,
) -> String {
    render_key_value_table(&[
        ("account", account.to_string()),
        ("calendar.id", calendar.id.clone()),
        ("calendar.name", calendar.name.clone()),
        ("dry_run", "true".to_string()),
        ("event.summary", review.summary.clone()),
        ("event.start", review.start.clone()),
        ("event.end", review.end.clone()),
        (
            "event.location",
            review.location.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "event.description",
            review
                .description
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("event.status", review.status.clone()),
        ("event.all_day", review.all_day.to_string()),
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
        format!(
            "public_url\t{}",
            resource.public_url.as_deref().unwrap_or("-")
        ),
        format!(
            "public_key\t{}",
            resource.public_key.as_deref().unwrap_or("-")
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
