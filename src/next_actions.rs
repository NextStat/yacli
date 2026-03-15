use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::doctor::doctor_payload;
use crate::error::Result;
use crate::goal_router::goal_route_payload;
use crate::onboarding::onboarding_resource_payload;

pub fn next_actions_payload(requested_account: Option<&str>, goal: Option<&str>) -> Result<Value> {
    let onboarding = onboarding_resource_payload(goal)?;
    let doctor = doctor_payload(requested_account, goal)?;
    let goal = goal.and_then(normalize_goal);
    let goal_route = if let Some(goal) = goal.as_deref() {
        Some(goal_route_payload(goal, requested_account)?)
    } else {
        None
    };
    let current_account = doctor["current_account"].clone();
    let status = combined_status(
        onboarding["status"].as_str().unwrap_or("needs_setup"),
        doctor["status"].as_str().unwrap_or("needs_setup"),
        goal_route
            .as_ref()
            .and_then(|payload| payload["remediation"]["status"].as_str()),
    );
    let actions = collect_actions(&onboarding, &doctor, goal_route.as_ref());

    Ok(json!({
        "status": status,
        "current_account": current_account,
        "count": actions.len(),
        "goal": goal,
        "goal_route": goal_route,
        "summary": summary_for_actions(&actions, goal.as_deref()),
        "actions": actions,
    }))
}

fn collect_actions(onboarding: &Value, doctor: &Value, goal_route: Option<&Value>) -> Vec<Value> {
    let mut actions = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(goal_route) = goal_route {
        append_goal_actions(&mut actions, &mut seen, goal_route);
    }

    for (source, payload) in [("onboarding", onboarding), ("doctor", doctor)] {
        let Some(checks) = payload["checks"].as_array() else {
            continue;
        };
        for check in checks {
            let status = check["status"].as_str().unwrap_or("pending");
            if status == "completed" {
                continue;
            }
            let Some(command) = check["recommended_command"].as_str() else {
                continue;
            };
            if !seen.insert(command.to_string()) {
                continue;
            }
            actions.push(json!({
                "id": check["id"].clone(),
                "title": check["title"].clone(),
                "status": status,
                "priority": priority_for_status(status),
                "reason": check["detail"].clone(),
                "command": command,
                "source": source,
            }));
        }
    }

    if actions.is_empty() {
        actions.push(json!({
            "id": "workflow_hub",
            "title": "Открыть Workflow Hub",
            "status": "ready",
            "priority": 3,
            "reason": "Продукт уже готов. Следующий лучший шаг — запускать реальные cross-service workflows.",
            "command": "yacli workflow list",
            "source": "product",
        }));
        actions.push(json!({
            "id": "activity_log",
            "title": "Проверить последние действия",
            "status": "ready",
            "priority": 4,
            "reason": "Replay и история действий помогают закрепить рабочий контур после setup.",
            "command": "yacli activity list",
            "source": "product",
        }));
    }

    actions.sort_by_key(|item| item["priority"].as_u64().unwrap_or(9));
    actions
}

fn append_goal_actions(actions: &mut Vec<Value>, seen: &mut BTreeSet<String>, goal_route: &Value) {
    if let Some(remediation_actions) = goal_route["remediation"]["actions"].as_array() {
        for action in remediation_actions {
            let Some(command) = action["command"].as_str() else {
                continue;
            };
            if !seen.insert(command.to_string()) {
                continue;
            }
            let status = action["status"].as_str().unwrap_or("pending");
            actions.push(json!({
                "id": action["id"].clone(),
                "title": action["title"].clone(),
                "status": status,
                "priority": 0,
                "reason": action["detail"].clone(),
                "command": command,
                "source": "goal",
            }));
        }
    }

    let remediation_status = goal_route["remediation"]["status"]
        .as_str()
        .unwrap_or("fallback");
    if remediation_status == "ready"
        && let Some(command) = goal_route["suggested_command"].as_str()
        && !command.is_empty()
        && seen.insert(command.to_string())
    {
        let workflow_id = goal_route["best_match"]["workflow"]["id"]
            .as_str()
            .unwrap_or_default();
        actions.push(json!({
            "id": "goal_route",
            "title": if workflow_id.is_empty() {
                "Открыть лучший маршрут для текущей цели".to_string()
            } else {
                format!("Открыть workflow `{workflow_id}` для текущей цели")
            },
            "status": "ready",
            "priority": 0,
            "reason": goal_route["summary"].clone(),
            "command": command,
            "source": "goal",
        }));
    }
}

fn priority_for_status(status: &str) -> u64 {
    match status {
        "blocked" => 0,
        "pending" => 1,
        "attention" => 2,
        "ready" => 3,
        _ => 4,
    }
}

fn combined_status(
    onboarding_status: &str,
    doctor_status: &str,
    goal_status: Option<&str>,
) -> &'static str {
    if let Some(goal_status) = goal_status
        && goal_status == "needs_setup"
    {
        return "needs_setup";
    }
    if onboarding_status == "ready" && doctor_status == "ready" {
        "ready"
    } else if onboarding_status == "needs_setup" || doctor_status == "needs_setup" {
        "needs_setup"
    } else {
        "in_progress"
    }
}

fn summary_for_actions(actions: &[Value], goal: Option<&str>) -> String {
    match actions.len() {
        0 => match goal {
            Some(goal) => {
                format!("Цель `{goal}` уже упирается не в readiness, а в выполнение workflow.")
            }
            None => "Нет обязательных следующих шагов.".to_string(),
        },
        1 => match goal {
            Some(goal) => format!("Для цели `{goal}` остался один сильный следующий шаг."),
            None => "Остался один сильный следующий шаг.".to_string(),
        },
        count => match goal {
            Some(goal) => format!(
                "Для цели `{goal}` есть {count} следующих шага с наибольшим продуктовым эффектом."
            ),
            None => format!("Есть {count} следующих шага с наибольшим продуктовым эффектом."),
        },
    }
}

fn normalize_goal(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
