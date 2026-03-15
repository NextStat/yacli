use serde_json::{Value, json};

use crate::account_store::AccountStore;
use crate::activity_store::ActivityStore;
use crate::credential_store::CredentialStore;
use crate::doctor::doctor_payload;
use crate::error::Result;
use crate::goal_router::goal_route_payload;
use crate::next_actions::next_actions_payload;
use crate::onboarding::onboarding_resource_payload;
use crate::runtime_context::auth_state;
use crate::workflows;

pub fn home_payload(requested_account: Option<&str>, goal: Option<&str>) -> Result<Value> {
    let onboarding = onboarding_resource_payload(goal)?;
    let doctor = doctor_payload(requested_account, goal)?;
    let next_actions = next_actions_payload(requested_account, goal)?;
    let goal = goal.and_then(normalize_goal);
    let goal_route = if let Some(goal) = goal.as_deref() {
        Some(goal_route_payload(goal, requested_account)?)
    } else {
        None
    };
    let workflow_items = workflows::workflow_catalog();
    let workflow_count = workflow_items.len();
    let highlighted_workflows = workflow_items.into_iter().take(3).collect::<Vec<_>>();
    let activity_store = ActivityStore::load()?;
    let latest_activity = activity_store.entries().first().cloned();
    let recent_activity_count = activity_store.entries().len();

    let account_store = AccountStore::load()?;
    if account_store.file.accounts.is_empty() {
        return Ok(json!({
            "status": next_actions["status"].clone(),
            "current_account": Value::Null,
            "email": Value::Null,
            "services": Value::Object(Default::default()),
            "workflow_count": workflow_count,
            "highlighted_workflows": highlighted_workflows,
            "recent_activity_count": recent_activity_count,
            "latest_activity": latest_activity,
            "onboarding": onboarding,
            "doctor": doctor,
            "goal": goal,
            "goal_route": goal_route,
            "next_actions": next_actions,
            "suggested_commands": suggested_commands_from_onboarding(&onboarding),
        }));
    }

    let account_name = account_store.resolved_account_name(requested_account)?;
    let account = account_store.get_account(&account_name)?;
    let credential_store = CredentialStore::load()?;
    let services = json!({
        "mail": auth_state(
            &credential_store,
            &account_name,
            account.mail.credential_ref.as_deref(),
            "mail",
        ),
        "calendar": auth_state(
            &credential_store,
            &account_name,
            account.calendar.credential_ref.as_deref(),
            "calendar",
        ),
        "disk": auth_state(
            &credential_store,
            &account_name,
            account.disk.credential_ref.as_deref(),
            "disk",
        ),
    });

    Ok(json!({
        "status": next_actions["status"].clone(),
        "current_account": account_name,
        "email": account.email,
        "services": services,
        "workflow_count": workflow_count,
        "highlighted_workflows": highlighted_workflows,
        "recent_activity_count": recent_activity_count,
        "latest_activity": latest_activity,
        "onboarding": onboarding,
        "doctor": doctor,
        "goal": goal,
        "goal_route": goal_route,
        "next_actions": next_actions,
        "suggested_commands": suggested_commands_from_onboarding(&onboarding),
    }))
}

fn suggested_commands_from_onboarding(onboarding: &Value) -> Vec<String> {
    onboarding["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["recommended_command"].as_str())
        .map(ToString::to_string)
        .collect()
}

fn normalize_goal(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
