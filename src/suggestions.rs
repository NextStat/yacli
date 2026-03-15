use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Value, json};

use crate::activity_store::{ActivityEntry, ActivityStore};
use crate::doctor::doctor_payload;
use crate::error::Result;
use crate::goal_router::goal_route_payload;
use crate::workflows;

#[derive(Clone, Debug, Serialize)]
struct SuggestionAction {
    kind: &'static str,
    label: &'static str,
    workflow_id: Option<&'static str>,
    activity_id: Option<String>,
    primary_tool: Option<&'static str>,
    primary_operation: Option<&'static str>,
    supports_review: bool,
}

#[derive(Clone, Debug, Serialize)]
struct SuggestionItem {
    id: String,
    title: String,
    status: &'static str,
    priority: u64,
    reason: String,
    command: String,
    source: &'static str,
    kind: &'static str,
    activity_id: String,
    operation: String,
    workflow_id: Option<&'static str>,
    action: SuggestionAction,
}

pub fn suggestions_payload(requested_account: Option<&str>, goal: Option<&str>) -> Result<Value> {
    let doctor = doctor_payload(requested_account, goal)?;
    let goal = normalize_goal(goal);
    let goal_route = if let Some(goal) = goal.as_deref() {
        Some(goal_route_payload(goal, requested_account)?)
    } else {
        None
    };
    let goal_workflow = goal_route
        .as_ref()
        .and_then(|route| route["best_match"]["workflow"]["id"].as_str());
    let store = ActivityStore::load()?;
    let suggestions = collect_suggestions(store.entries(), goal_workflow);
    let status = if suggestions.is_empty() {
        "idle"
    } else {
        "ready"
    };

    Ok(json!({
        "status": status,
        "current_account": doctor["current_account"].clone(),
        "goal": goal,
        "goal_route": goal_route,
        "count": suggestions.len(),
        "summary": summary_for_suggestions(&suggestions, goal.as_deref()),
        "suggestions": suggestions,
    }))
}

fn collect_suggestions(
    entries: &[ActivityEntry],
    goal_workflow: Option<&str>,
) -> Vec<SuggestionItem> {
    let mut suggestions = Vec::new();
    let mut seen = BTreeSet::new();

    for entry in entries {
        match entry.operation.as_str() {
            "mail.send_link.partial" => {
                push_suggestion(
                    &mut suggestions,
                    &mut seen,
                    SuggestionItem {
                        id: format!("recover-mail-send-link-{}", entry.id),
                        title: "Дослать письмо с уже опубликованной ссылкой".to_string(),
                        status: "ready",
                        priority: goal_adjusted_priority(0, "send-link-by-mail", goal_workflow),
                        reason: format!(
                            "{} Повторяем только почтовый шаг без новой загрузки файла.",
                            entry.summary
                        ),
                        command: entry.replay_command.clone(),
                        source: "activity",
                        kind: "recovery",
                        activity_id: entry.id.clone(),
                        operation: entry.operation.clone(),
                        workflow_id: Some("send-link-by-mail"),
                        action: workflow_action("send-link-by-mail"),
                    },
                );
                if let Some(command) = undo_command(entry) {
                    push_suggestion(
                        &mut suggestions,
                        &mut seen,
                        SuggestionItem {
                            id: format!("cleanup-published-link-{}", entry.id),
                            title: "Отозвать уже опубликованную ссылку".to_string(),
                            status: "ready",
                            priority: goal_adjusted_priority(1, "revoke-public-link", goal_workflow),
                            reason: "Если письмо больше не нужно отправлять, можно сразу закрыть публичный доступ к уже опубликованному файлу.".to_string(),
                            command,
                            source: "activity",
                            kind: "cleanup",
                            activity_id: entry.id.clone(),
                            operation: entry.operation.clone(),
                            workflow_id: Some("revoke-public-link"),
                            action: undo_activity_action(entry),
                        },
                    );
                }
            }
            "mail.invite.create_event.partial" => {
                push_suggestion(
                    &mut suggestions,
                    &mut seen,
                    SuggestionItem {
                        id: format!("recover-invite-create-event-{}", entry.id),
                        title: "Повторить календарный шаг для приглашения".to_string(),
                        status: "ready",
                        priority: goal_adjusted_priority(0, "invite-to-calendar", goal_workflow),
                        reason: format!(
                            "{} Повторяем только создание события, не перечитывая письмо заново.",
                            entry.summary
                        ),
                        command: entry.replay_command.clone(),
                        source: "activity",
                        kind: "recovery",
                        activity_id: entry.id.clone(),
                        operation: entry.operation.clone(),
                        workflow_id: Some("invite-to-calendar"),
                        action: workflow_action("invite-to-calendar"),
                    },
                );
            }
            "disk.publish" | "disk.upload_link" => {
                if let Some(command) = revoke_public_link_command(entry) {
                    push_suggestion(
                        &mut suggestions,
                        &mut seen,
                        SuggestionItem {
                            id: format!("revoke-public-link-{}", entry.id),
                            title: "Отозвать публичную ссылку".to_string(),
                            status: "ready",
                            priority: goal_adjusted_priority(
                                2,
                                "revoke-public-link",
                                goal_workflow,
                            ),
                            reason: format!(
                                "{} Если ссылка больше не нужна, revoke можно сделать одним действием.",
                                entry.summary
                            ),
                            command,
                            source: "activity",
                            kind: "undo",
                            activity_id: entry.id.clone(),
                            operation: entry.operation.clone(),
                            workflow_id: Some("revoke-public-link"),
                            action: undo_activity_action(entry),
                        },
                    );
                }
            }
            "calendar.create" => {
                if let Some(command) = undo_command(entry) {
                    push_suggestion(
                        &mut suggestions,
                        &mut seen,
                        SuggestionItem {
                            id: format!("undo-calendar-create-{}", entry.id),
                            title: "Откатить последнее созданное событие".to_string(),
                            status: "ready",
                            priority: 3,
                            reason: format!(
                                "{} Если событие было создано ошибочно, откат уже готов.",
                                entry.summary
                            ),
                            command,
                            source: "activity",
                            kind: "undo",
                            activity_id: entry.id.clone(),
                            operation: entry.operation.clone(),
                            workflow_id: None,
                            action: undo_activity_action(entry),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    suggestions.sort_by_key(|item| item.priority);
    suggestions.truncate(5);
    suggestions
}

fn push_suggestion(
    suggestions: &mut Vec<SuggestionItem>,
    seen: &mut BTreeSet<String>,
    item: SuggestionItem,
) {
    if item.command.trim().is_empty() || !seen.insert(item.command.clone()) {
        return;
    }
    suggestions.push(item);
}

fn undo_command(entry: &ActivityEntry) -> Option<String> {
    entry
        .undo_command
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn revoke_public_link_command(entry: &ActivityEntry) -> Option<String> {
    undo_command(entry).or_else(|| {
        extract_disk_path(&entry.replay_command).map(|path| format!("yacli disk unpublish {path}"))
    })
}

fn workflow_action(workflow_id: &'static str) -> SuggestionAction {
    SuggestionAction {
        kind: "open_workflow",
        label: "Open workflow",
        workflow_id: Some(workflow_id),
        activity_id: None,
        primary_tool: workflows::workflow_primary_tool(workflow_id),
        primary_operation: Some(workflows::workflow_primary_operation(workflow_id)),
        supports_review: workflows::workflow_supports_review(workflow_id),
    }
}

fn undo_activity_action(entry: &ActivityEntry) -> SuggestionAction {
    SuggestionAction {
        kind: "undo_activity",
        label: "Undo action",
        workflow_id: None,
        activity_id: Some(entry.id.clone()),
        primary_tool: Some("yacli.activity.undo"),
        primary_operation: Some("activity.undo"),
        supports_review: false,
    }
}

fn extract_disk_path(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .find(|token| token.starts_with("disk:/"))
        .map(ToString::to_string)
}

fn goal_adjusted_priority(base: u64, workflow_id: &str, goal_workflow: Option<&str>) -> u64 {
    if goal_workflow == Some(workflow_id) {
        0
    } else {
        base
    }
}

fn summary_for_suggestions(suggestions: &[SuggestionItem], goal: Option<&str>) -> String {
    if suggestions.is_empty() {
        return match goal {
            Some(goal) => format!(
                "Для цели `{goal}` пока нет proactive suggestions из уже выполненных действий."
            ),
            None => "Пока нет proactive suggestions из последних действий.".to_string(),
        };
    }

    match goal {
        Some(goal) => format!(
            "Для цели `{goal}` найдено {} proactive suggestions из реальной activity history.",
            suggestions.len()
        ),
        None => format!(
            "Найдено {} proactive suggestions из реальной activity history.",
            suggestions.len()
        ),
    }
}

fn normalize_goal(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::collect_suggestions;
    use crate::activity_store::ActivityEntry;

    #[test]
    fn collect_suggestions_promotes_partial_send_link_recovery() {
        let entries = vec![ActivityEntry {
            id: "act_20260315T100000Z_partial123".to_string(),
            occurred_at: "2026-03-15T10:00:00Z".to_string(),
            source: "cli".to_string(),
            operation: "mail.send_link.partial".to_string(),
            account: "mock".to_string(),
            summary: "Публичная ссылка создана, но письмо не отправлено".to_string(),
            replay_command:
                "yacli mail send-published-link andrei@nextstat.io \"Материалы\" --public-url https://disk.yandex.example/public".to_string(),
            undo: None,
            undo_command: Some("yacli disk unpublish disk:/docs/archive.zip".to_string()),
        }];

        let suggestions = collect_suggestions(&entries, Some("send-link-by-mail"));
        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].kind, "recovery");
        assert_eq!(suggestions[0].workflow_id, Some("send-link-by-mail"));
        assert_eq!(suggestions[0].action.kind, "open_workflow");
        assert_eq!(suggestions[0].action.workflow_id, Some("send-link-by-mail"));
        assert_eq!(
            suggestions[0].action.primary_tool,
            Some("yacli.mail.send_link")
        );
        assert!(suggestions[0].action.supports_review);
        assert_eq!(suggestions[1].kind, "cleanup");
        assert_eq!(suggestions[1].action.kind, "undo_activity");
        assert_eq!(
            suggestions[1].action.activity_id.as_deref(),
            Some(entries[0].id.as_str())
        );
    }

    #[test]
    fn collect_suggestions_promotes_partial_invite_recovery() {
        let entries = vec![ActivityEntry {
            id: "act_20260315T110000Z_partial456".to_string(),
            occurred_at: "2026-03-15T11:00:00Z".to_string(),
            source: "cli".to_string(),
            operation: "mail.invite.create_event.partial".to_string(),
            account: "mock".to_string(),
            summary: "Не удалось создать событие из приглашения письма 77: Ревью".to_string(),
            replay_command:
                "yacli calendar create 'Ревью' '2026-03-16T10:00:00Z' '2026-03-16T10:30:00Z' --calendar team --dry-run"
                    .to_string(),
            undo: None,
            undo_command: None,
        }];

        let suggestions = collect_suggestions(&entries, Some("invite-to-calendar"));
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].kind, "recovery");
        assert_eq!(suggestions[0].workflow_id, Some("invite-to-calendar"));
        assert!(suggestions[0].command.contains("calendar create"));
        assert_eq!(suggestions[0].action.kind, "open_workflow");
        assert_eq!(
            suggestions[0].action.primary_tool,
            Some("yacli.mail.invite.create_event")
        );
        assert!(!suggestions[0].action.supports_review);
    }
}
