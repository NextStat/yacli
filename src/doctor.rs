use serde_json::{Value, json};

use crate::account_store::AccountStore;
use crate::activity_store::ActivityStore;
use crate::credential_store::{CredentialStore, configured_secret_backend_name};
use crate::error::Result;
use crate::goal_router::goal_route_payload;
use crate::mcp::install::mcp_client_readiness;
use crate::onboarding::onboarding_resource_payload;
use crate::paths::{accounts_path, activity_log_path, config_dir, credentials_path};
use crate::runtime_context::auth_state;

pub fn doctor_payload(requested_account: Option<&str>, goal: Option<&str>) -> Result<Value> {
    let config_dir = config_dir()?;
    let accounts_path = accounts_path()?;
    let credentials_path = credentials_path()?;
    let activity_log_path = activity_log_path()?;
    let secret_backend = configured_secret_backend_name()?;
    let normalized_goal = normalize_goal(goal);
    let onboarding = onboarding_resource_payload(normalized_goal.as_deref())?;
    let mcp_clients = mcp_client_readiness()?;
    let account_store = AccountStore::load()?;
    let activity_store = ActivityStore::load()?;
    let current_account_name = account_store.current_account_name().ok();
    let goal_route = if let Some(goal) = normalized_goal.as_deref() {
        Some(goal_route_payload(
            goal,
            requested_account.or(current_account_name.as_deref()),
        )?)
    } else {
        None
    };

    let mut checks = vec![
        file_check(
            "accounts_config",
            "Конфиг аккаунтов",
            accounts_path.exists(),
            format!("Путь: {}", accounts_path.display()),
            "yacli setup me@yandex.ru",
        ),
        file_check(
            "activity_log",
            "Журнал действий",
            activity_log_path.exists(),
            format!(
                "Путь: {}, записей: {}",
                activity_log_path.display(),
                activity_store.entries().len()
            ),
            "yacli activity list",
        ),
        secret_backend_check(secret_backend, credentials_path.exists()),
        mcp_install_check(&mcp_clients),
    ];

    let mut current_account = Value::Null;
    let mut email = Value::Null;
    let mut services = json!({});

    if !account_store.file.accounts.is_empty() {
        let account_name = account_store.resolved_account_name(requested_account)?;
        let account = account_store.get_account(&account_name)?;
        let credential_store = CredentialStore::load()?;
        let mail = auth_state(
            &credential_store,
            &account_name,
            account.mail.credential_ref.as_deref(),
            "mail",
        );
        let calendar = auth_state(
            &credential_store,
            &account_name,
            account.calendar.credential_ref.as_deref(),
            "calendar",
        );
        let disk = auth_state(
            &credential_store,
            &account_name,
            account.disk.credential_ref.as_deref(),
            "disk",
        );

        checks.push(service_check(
            "mail",
            "Почта",
            mail.credential_state,
            &mail.detail,
            "yacli login",
        ));
        checks.push(service_check(
            "calendar",
            "Календарь",
            calendar.credential_state,
            &calendar.detail,
            "yacli login calendar --app-password <пароль>",
        ));
        checks.push(service_check(
            "disk",
            "Диск",
            disk.credential_state,
            &disk.detail,
            "yacli login",
        ));

        let connected = [
            mail.credential_state,
            calendar.credential_state,
            disk.credential_state,
        ]
        .iter()
        .filter(|state| matches!(**state, "store_present" | "env_present"))
        .count();
        checks.push(json!({
            "id": "workflow_hub",
            "title": "Workflow Hub",
            "status": if connected >= 2 { "completed" } else if connected == 1 { "pending" } else { "blocked" },
            "detail": if connected >= 2 {
                "Есть достаточно подключённых сервисов для реальных кросс-сервисных сценариев."
            } else if connected == 1 {
                "Подключите ещё один сервис, чтобы workflows стали по-настоящему полезны."
            } else {
                "Сначала подключите хотя бы Почту, Диск или Календарь."
            },
            "recommended_command": "yacli workflow list"
        }));

        current_account = json!(account_name);
        email = json!(account.email.clone());
        services = json!({
            "mail": mail,
            "calendar": calendar,
            "disk": disk,
        });
    }

    let overall_status = doctor_status(
        &checks,
        account_store.file.accounts.is_empty(),
        goal_route.as_ref(),
    );
    let suggested_commands = collect_suggested_commands(&checks, goal_route.as_ref());
    let focus_checks = focus_checks_from_goal_route(&checks, goal_route.as_ref());

    Ok(json!({
        "status": overall_status,
        "goal": normalized_goal,
        "goal_route": goal_route,
        "focus_checks": focus_checks,
        "config": {
            "dir": config_dir,
            "accountsPath": accounts_path,
            "credentialsPath": credentials_path,
            "activityLogPath": activity_log_path,
            "secretBackend": secret_backend,
        },
        "current_account": current_account,
        "email": email,
        "services": services,
        "checks": checks,
        "mcp_clients": mcp_clients,
        "onboardingStatus": onboarding["status"].clone(),
        "suggested_commands": suggested_commands,
    }))
}

fn file_check(
    id: &str,
    title: &str,
    present: bool,
    detail: String,
    recommended_command: &str,
) -> Value {
    json!({
        "id": id,
        "title": title,
        "status": if present { "completed" } else { "pending" },
        "detail": detail,
        "recommended_command": recommended_command,
    })
}

fn secret_backend_check(secret_backend: &str, credentials_file_present: bool) -> Value {
    let (status, detail) = match secret_backend {
        "keyring" => (
            "completed",
            "Секреты будут храниться в системном keyring/keychain по умолчанию в релизных сборках."
                .to_string(),
        ),
        "file" => (
            "attention",
            format!(
                "Включён file backend. Секреты хранятся локально в {}; это нормально для debug/headless сценариев, но релизные сборки по умолчанию используют системный keyring/keychain.",
                credentials_path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| "credentials.toml".to_string())
            ),
        ),
        other => ("attention", format!("Неожиданный secret backend: {other}")),
    };

    json!({
        "id": "secret_backend",
        "title": "Хранилище секретов",
        "status": status,
        "detail": if credentials_file_present && secret_backend == "file" {
            detail
        } else if credentials_file_present {
            format!("{detail} Legacy credentials.toml still exists as migration source or fallback.")
        } else {
            detail
        },
        "recommended_command": if secret_backend == "file" {
            "unset YACLI_SECRET_BACKEND"
        } else {
            "yacli status"
        },
    })
}

fn service_check(
    id: &str,
    title: &str,
    credential_state: &str,
    detail: &str,
    command: &str,
) -> Value {
    let status = match credential_state {
        "store_present" | "env_present" => "completed",
        "not_configured" | "store_missing" | "env_missing" | "store_expired" => "pending",
        _ => "attention",
    };
    json!({
        "id": id,
        "title": title,
        "status": status,
        "detail": detail,
        "recommended_command": command,
    })
}

fn mcp_install_check(clients: &[crate::mcp::install::ClientReadiness]) -> Value {
    let installed = clients
        .iter()
        .filter(|client| client.status == "installed")
        .count();
    let available = clients
        .iter()
        .filter(|client| {
            matches!(
                client.status,
                "installed" | "not_installed" | "available_unverifiable"
            )
        })
        .count();
    let detail = if installed > 0 {
        let names = clients
            .iter()
            .filter(|client| client.status == "installed")
            .map(|client| client.client)
            .collect::<Vec<_>>()
            .join(", ");
        format!("yacli уже зарегистрирован в следующих клиентах: {names}")
    } else if available > 0 {
        "Совместимые MCP-клиенты обнаружены, но yacli ещё не зарегистрирован ни в одном из них."
            .to_string()
    } else {
        "Совместимые MCP-клиенты не обнаружены локально. Можно продолжать через CLI или поднять HTTP/stdio MCP вручную."
            .to_string()
    };

    json!({
        "id": "mcp_install",
        "title": "Установка MCP-клиентов",
        "status": if installed > 0 {
            "completed"
        } else if available > 0 {
            "pending"
        } else {
            "attention"
        },
        "detail": detail,
        "recommended_command": "yacli mcp install --client claude",
    })
}

fn doctor_status(checks: &[Value], no_accounts: bool, goal_route: Option<&Value>) -> &'static str {
    if no_accounts {
        return "needs_setup";
    }
    if goal_route.and_then(|payload| payload["remediation"]["status"].as_str())
        == Some("needs_setup")
    {
        return "needs_setup";
    }
    if checks.iter().any(|item| item["status"] == "attention") {
        return "attention";
    }
    if checks.iter().all(|item| item["status"] == "completed") {
        "ready"
    } else {
        "in_progress"
    }
}

fn collect_suggested_commands(checks: &[Value], goal_route: Option<&Value>) -> Vec<String> {
    let mut commands = Vec::new();
    if let Some(goal_route) = goal_route
        && let Some(actions) = goal_route["remediation"]["actions"].as_array()
    {
        for action in actions {
            if let Some(command) = action["command"].as_str()
                && !commands.iter().any(|existing| existing == command)
            {
                commands.push(command.to_string());
            }
        }
    }

    for command in checks
        .iter()
        .filter(|item| item["status"] != "completed")
        .filter_map(|item| item["recommended_command"].as_str())
    {
        if !commands.iter().any(|existing| existing == command) {
            commands.push(command.to_string());
        }
    }

    commands
}

fn focus_checks_from_goal_route(checks: &[Value], goal_route: Option<&Value>) -> Vec<Value> {
    let Some(goal_route) = goal_route else {
        return Vec::new();
    };
    let Some(actions) = goal_route["remediation"]["actions"].as_array() else {
        return Vec::new();
    };

    let mut focused = Vec::new();
    for action in actions {
        let Some(action_id) = action["id"].as_str() else {
            continue;
        };
        if let Some(check) = checks.iter().find(|check| check["id"] == action_id) {
            focused.push(check.clone());
        } else {
            focused.push(json!({
                "id": action["id"].clone(),
                "title": action["title"].clone(),
                "status": action["status"].clone(),
                "detail": action["detail"].clone(),
                "recommended_command": action["command"].clone(),
            }));
        }
    }
    focused
}

fn normalize_goal(goal: Option<&str>) -> Option<String> {
    goal.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}
