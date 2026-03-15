use serde_json::{Value, json};

use crate::account_store::AccountStore;
use crate::credential_store::CredentialStore;
use crate::error::Result;
use crate::goal_router::goal_route_payload;
use crate::mcp::install::mcp_client_readiness;
use crate::runtime_context::auth_state;

pub fn onboarding_resource_payload(goal: Option<&str>) -> Result<Value> {
    let account_store = AccountStore::load()?;
    let credential_store = CredentialStore::load()?;
    let mcp_clients = mcp_client_readiness()?;
    let current_account = account_store.current_account_name().ok();
    let goal = normalize_goal(goal);
    let goal_route = if let Some(goal) = goal.as_deref() {
        Some(goal_route_payload(goal, current_account.as_deref())?)
    } else {
        None
    };

    if account_store.file.accounts.is_empty() {
        let checks = vec![
            json!({
                "id": "account",
                "title": "Добавить аккаунт",
                "status": "pending",
                "detail": "Сначала добавьте аккаунт Яндекса и выполните базовый setup.",
                "recommended_command": "yacli setup me@yandex.ru"
            }),
            json!({
                "id": "mail",
                "title": "Подключить Почту",
                "status": "blocked",
                "detail": "Почта и Диск подключаются после добавления аккаунта.",
                "recommended_command": "yacli setup me@yandex.ru"
            }),
            json!({
                "id": "calendar",
                "title": "Подключить Календарь",
                "status": "blocked",
                "detail": "Календарь подключается после добавления аккаунта и пароля приложения.",
                "recommended_command": "yacli setup me@yandex.ru --calendar-app-password <пароль>"
            }),
            json!({
                "id": "mcp_install",
                "title": "Подключить MCP-клиент",
                "status": "blocked",
                "detail": "MCP имеет смысл ставить после базового setup аккаунта.",
                "recommended_command": "yacli mcp install --client claude"
            }),
            json!({
                "id": "workflow_hub",
                "title": "Запустить workflow",
                "status": "blocked",
                "detail": "Workflow Hub станет полезен после базового setup аккаунта.",
                "recommended_command": "yacli workflow list"
            }),
        ];
        let focus_checks = focus_checks_from_goal_route(&checks, goal_route.as_ref());
        return Ok(json!({
            "status": goal_status_override("needs_setup", goal_route.as_ref()),
            "goal": goal,
            "goal_route": goal_route,
            "focus_checks": focus_checks,
            "account_count": 0,
            "current_account": null,
            "checks": checks,
        }));
    }

    let account_name = current_account.clone().unwrap_or_else(|| {
        account_store
            .file
            .accounts
            .keys()
            .next()
            .cloned()
            .unwrap_or_default()
    });
    let account = account_store.get_account(&account_name)?;

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

    let checks = vec![
        auth_check(
            "account",
            "Аккаунт",
            "completed",
            format!("Текущий аккаунт: {account_name}"),
            Some(format!("yacli setup {}", account.email)),
        ),
        service_check(
            "mail",
            "Почта",
            mail.credential_state,
            &mail.detail,
            "yacli login",
        ),
        service_check(
            "calendar",
            "Календарь",
            calendar.credential_state,
            &calendar.detail,
            "yacli login calendar --app-password <пароль>",
        ),
        service_check(
            "disk",
            "Диск",
            disk.credential_state,
            &disk.detail,
            "yacli login",
        ),
        workflow_hub_check(
            mail.credential_state,
            calendar.credential_state,
            disk.credential_state,
        ),
        mcp_install_check(&mcp_clients),
    ];

    let completed = checks
        .iter()
        .filter(|item| item["status"] == "completed")
        .count();
    let status = if completed == checks.len() {
        "ready"
    } else if completed >= 2 {
        "in_progress"
    } else {
        "needs_setup"
    };
    let focus_checks = focus_checks_from_goal_route(&checks, goal_route.as_ref());

    Ok(json!({
        "status": goal_status_override(status, goal_route.as_ref()),
        "goal": goal,
        "goal_route": goal_route,
        "focus_checks": focus_checks,
        "account_count": account_store.file.accounts.len(),
        "current_account": account_name,
        "checks": checks,
        "mcp_clients": mcp_clients,
    }))
}

fn auth_check(
    id: &str,
    title: &str,
    status: &str,
    detail: String,
    recommended_command: Option<String>,
) -> Value {
    json!({
        "id": id,
        "title": title,
        "status": status,
        "detail": detail,
        "recommended_command": recommended_command,
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
    auth_check(
        id,
        title,
        status,
        detail.to_string(),
        Some(command.to_string()),
    )
}

fn workflow_hub_check(mail: &str, calendar: &str, disk: &str) -> Value {
    let connected = [mail, calendar, disk]
        .iter()
        .filter(|state| matches!(**state, "store_present" | "env_present"))
        .count();
    let (status, detail) = if connected >= 2 {
        (
            "completed",
            "Workflow Hub уже может вести кросс-сервисные сценарии и action review.",
        )
    } else if connected == 1 {
        (
            "pending",
            "Подключите ещё один сервис, чтобы получить реальные кросс-сервисные workflows.",
        )
    } else {
        (
            "blocked",
            "Сначала подключите хотя бы Почту или Диск, чтобы workflows стали полезны.",
        )
    };
    auth_check(
        "workflow_hub",
        "Workflow Hub",
        status,
        detail.to_string(),
        Some("yacli workflow list".to_string()),
    )
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
    let (status, detail) = if installed > 0 {
        let names = clients
            .iter()
            .filter(|client| client.status == "installed")
            .map(|client| client.client)
            .collect::<Vec<_>>()
            .join(", ");
        (
            "completed",
            format!("yacli уже установлен в MCP-клиентах: {names}"),
        )
    } else if available > 0 {
        (
            "pending",
            "Подключите хотя бы один MCP-клиент, чтобы workflows и Apps работали в агентских хостах."
                .to_string(),
        )
    } else {
        (
            "attention",
            "Совместимые MCP-клиенты локально не обнаружены. Можно продолжать через CLI или поднять MCP вручную."
                .to_string(),
        )
    };
    auth_check(
        "mcp_install",
        "Подключить MCP-клиент",
        status,
        detail,
        Some("yacli mcp install --client claude".to_string()),
    )
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
        }
    }
    focused
}

fn goal_status_override(base_status: &str, goal_route: Option<&Value>) -> String {
    if goal_route.and_then(|payload| payload["remediation"]["status"].as_str())
        == Some("needs_setup")
    {
        "needs_setup".to_string()
    } else {
        base_status.to_string()
    }
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
