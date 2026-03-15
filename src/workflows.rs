use crate::activity_store::{ActivityEntry, ActivityStore};
use crate::doctor::doctor_payload;
use crate::error::Result;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct WorkflowDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub connects: &'static str,
    pub request_example: &'static str,
    pub prompt_name: &'static str,
    pub skill_name: &'static str,
    pub cli_steps: &'static [&'static str],
    pub mcp_tools: &'static [&'static str],
}

const DAILY_BRIEFING_CLI_STEPS: &[&str] = &["mail list --limit 10", "calendar events"];
const DAILY_BRIEFING_TOOLS: &[&str] = &["yacli.mail.list", "yacli.calendar.events"];

const REPLY_WITH_CONTEXT_CLI_STEPS: &[&str] = &[
    "mail read 1353",
    "calendar events 2026-03-14 2026-03-16",
    "mail reply 1353 \"Подтверждаю, это окно подходит\"",
];
const REPLY_WITH_CONTEXT_TOOLS: &[&str] = &[
    "yacli.mail.read",
    "yacli.calendar.events",
    "yacli.mail.reply",
];

const ATTACHMENT_TO_DISK_CLI_STEPS: &[&str] = &[
    "mail search \"invoice\"",
    "mail attachment export 1353 --name invoice.pdf --output ./invoice.pdf",
];
const ATTACHMENT_TO_DISK_TOOLS: &[&str] = &["yacli.mail.search", "yacli.mail.attachment.export"];

const SEND_FILE_BY_MAIL_CLI_STEPS: &[&str] =
    &["mail send person@example.com \"Счёт\" \"Во вложении файл\" --attach ./invoice.pdf"];
const SEND_FILE_BY_MAIL_TOOLS: &[&str] = &["yacli.mail.send"];

const SEND_LINK_BY_MAIL_CLI_STEPS: &[&str] = &[
    "mail send-link person@example.com \"Материалы\" \"Отправляю ссылку\" --source ./archive.zip --path disk:/docs/archive/archive.zip",
];
const SEND_LINK_BY_MAIL_TOOLS: &[&str] = &["yacli.mail.send_link"];

const PUBLISH_FILE_LINK_CLI_STEPS: &[&str] =
    &["disk upload-link --source ./archive.zip --path disk:/docs/archive/archive.zip"];
const PUBLISH_FILE_LINK_TOOLS: &[&str] = &["yacli.disk.upload_link"];

const REVOKE_PUBLIC_LINK_CLI_STEPS: &[&str] = &["disk unpublish disk:/docs/archive/archive.zip"];
const REVOKE_PUBLIC_LINK_TOOLS: &[&str] = &["yacli.disk.unpublish"];

const INVITE_TO_CALENDAR_CLI_STEPS: &[&str] = &[
    "mail search \"приглашение\"",
    "mail invite inspect 1353 --index 1",
    "mail invite create-event 1353 --index 1 --calendar team",
];
const INVITE_TO_CALENDAR_TOOLS: &[&str] = &[
    "yacli.mail.search",
    "yacli.mail.invite.inspect",
    "yacli.mail.invite.create_event",
];

const WORKFLOWS: &[WorkflowDefinition] = &[
    WorkflowDefinition {
        id: "daily-briefing",
        title: "Утренняя сводка",
        summary: "Собрать краткую сводку по новым письмам и ближайшим встречам.",
        connects: "Почта + календарь",
        request_example: "Собери утреннюю сводку по письмам и встречам",
        prompt_name: "daily-briefing",
        skill_name: "yacli-daily-briefing",
        cli_steps: DAILY_BRIEFING_CLI_STEPS,
        mcp_tools: DAILY_BRIEFING_TOOLS,
    },
    WorkflowDefinition {
        id: "reply-with-context",
        title: "Ответ с учётом календаря",
        summary: "Прочитать письмо, проверить расписание и подготовить ответ с контекстом.",
        connects: "Почта + календарь",
        request_example: "Ответь на письмо с учётом моего расписания",
        prompt_name: "reply-with-context",
        skill_name: "yacli-reply-with-context",
        cli_steps: REPLY_WITH_CONTEXT_CLI_STEPS,
        mcp_tools: REPLY_WITH_CONTEXT_TOOLS,
    },
    WorkflowDefinition {
        id: "attachment-to-disk",
        title: "Вложение в локальный файл",
        summary: "Найти письмо и выгрузить нужное вложение в локальный файл.",
        connects: "Почта → файл",
        request_example: "Найди письмо и сохрани вложение в файл",
        prompt_name: "attachment-to-disk",
        skill_name: "yacli-attachment-to-disk",
        cli_steps: ATTACHMENT_TO_DISK_CLI_STEPS,
        mcp_tools: ATTACHMENT_TO_DISK_TOOLS,
    },
    WorkflowDefinition {
        id: "send-file-by-mail",
        title: "Файл в письмо",
        summary: "Взять локальный файл и отправить его как вложение по почте.",
        connects: "Файл → почта",
        request_example: "Отправь файл с диска по почте",
        prompt_name: "send-file-by-mail",
        skill_name: "yacli-send-file-by-mail",
        cli_steps: SEND_FILE_BY_MAIL_CLI_STEPS,
        mcp_tools: SEND_FILE_BY_MAIL_TOOLS,
    },
    WorkflowDefinition {
        id: "send-link-by-mail",
        title: "Ссылка на большой файл в письмо",
        summary: "Загрузить локальный файл на Диск, опубликовать ссылку и отправить её по почте.",
        connects: "Файл → Диск → почта",
        request_example: "Загрузи большой файл на Диск и отправь ссылку по почте",
        prompt_name: "send-link-by-mail",
        skill_name: "yacli-send-link-by-mail",
        cli_steps: SEND_LINK_BY_MAIL_CLI_STEPS,
        mcp_tools: SEND_LINK_BY_MAIL_TOOLS,
    },
    WorkflowDefinition {
        id: "publish-file-link",
        title: "Файл в публичную ссылку",
        summary: "Загрузить локальный файл на Диск и сразу получить public URL / public key.",
        connects: "Файл → Диск",
        request_example: "Загрузи файл на Диск и дай публичную ссылку",
        prompt_name: "publish-file-link",
        skill_name: "yacli-publish-file-link",
        cli_steps: PUBLISH_FILE_LINK_CLI_STEPS,
        mcp_tools: PUBLISH_FILE_LINK_TOOLS,
    },
    WorkflowDefinition {
        id: "revoke-public-link",
        title: "Отозвать публичную ссылку",
        summary: "Снять public URL / public key у приватного файла или папки на Диске.",
        connects: "Диск",
        request_example: "Отзови публичную ссылку у файла на Диске",
        prompt_name: "revoke-public-link",
        skill_name: "yacli-revoke-public-link",
        cli_steps: REVOKE_PUBLIC_LINK_CLI_STEPS,
        mcp_tools: REVOKE_PUBLIC_LINK_TOOLS,
    },
    WorkflowDefinition {
        id: "invite-to-calendar",
        title: "Приглашение в событие",
        summary: "Найти письмо с приглашением и создать событие в календаре.",
        connects: "Почта → календарь",
        request_example: "Найди приглашение и добавь встречу в календарь",
        prompt_name: "invite-to-calendar",
        skill_name: "yacli-invite-to-calendar",
        cli_steps: INVITE_TO_CALENDAR_CLI_STEPS,
        mcp_tools: INVITE_TO_CALENDAR_TOOLS,
    },
];

pub fn workflow_ids() -> Vec<&'static str> {
    WORKFLOWS.iter().map(|workflow| workflow.id).collect()
}

pub fn workflow_definition(id: &str) -> Option<&'static WorkflowDefinition> {
    WORKFLOWS.iter().find(|workflow| workflow.id == id)
}

pub fn workflow_primary_tool(id: &str) -> Option<&'static str> {
    match id {
        "daily-briefing" => Some("yacli.mail.list"),
        "reply-with-context" => Some("yacli.mail.reply"),
        "attachment-to-disk" => Some("yacli.mail.attachment.export"),
        "send-file-by-mail" => Some("yacli.mail.send"),
        "send-link-by-mail" => Some("yacli.mail.send_link"),
        "publish-file-link" => Some("yacli.disk.upload_link"),
        "revoke-public-link" => Some("yacli.disk.unpublish"),
        "invite-to-calendar" => Some("yacli.mail.invite.create_event"),
        _ => workflow_definition(id).and_then(|definition| definition.mcp_tools.first().copied()),
    }
}

pub fn workflow_primary_operation(id: &str) -> &'static str {
    match workflow_primary_tool(id) {
        Some("yacli.mail.list") => "mail.list",
        Some("yacli.mail.reply") => "mail.reply",
        Some("yacli.mail.attachment.export") => "mail.attachment.export",
        Some("yacli.mail.send") => "mail.send",
        Some("yacli.mail.send_link") => "mail.send_link",
        Some("yacli.disk.upload_link") => "disk.upload_link",
        Some("yacli.disk.unpublish") => "disk.unpublish",
        Some("yacli.mail.invite.create_event") => "mail.invite.create_event",
        _ => "",
    }
}

pub fn workflow_supports_review(id: &str) -> bool {
    matches!(
        workflow_primary_tool(id),
        Some(
            "yacli.mail.send"
                | "yacli.mail.send_link"
                | "yacli.disk.upload_link"
                | "yacli.disk.unpublish"
        )
    )
}

pub fn workflow_primary_tool_arguments(id: &str) -> Value {
    match id {
        "daily-briefing" => json!({
            "folder": "INBOX",
            "limit": 10
        }),
        "reply-with-context" => json!({
            "folder": "INBOX",
            "uid": 1353,
            "text": "Подтверждаю, это окно подходит"
        }),
        "attachment-to-disk" => json!({
            "folder": "INBOX",
            "uid": 1353,
            "name": "invoice.pdf",
            "output_path": "./invoice.pdf"
        }),
        "send-file-by-mail" => json!({
            "to": "person@example.com",
            "subject": "Счёт",
            "text": "Во вложении файл",
            "attachments": ["./invoice.pdf"]
        }),
        "send-link-by-mail" => json!({
            "to": "person@example.com",
            "subject": "Материалы",
            "text": "Отправляю ссылку",
            "source_path": "./archive.zip",
            "disk_path": "disk:/docs/archive/archive.zip",
            "overwrite": false
        }),
        "publish-file-link" => json!({
            "source": "./archive.zip",
            "path": "disk:/docs/archive/archive.zip",
            "overwrite": false
        }),
        "revoke-public-link" => json!({
            "path": "disk:/docs/archive/archive.zip"
        }),
        "invite-to-calendar" => json!({
            "folder": "INBOX",
            "uid": 1353,
            "index": 1,
            "event_index": 1,
            "calendar": "team"
        }),
        _ => json!({}),
    }
}

pub fn workflow_catalog() -> Vec<Value> {
    WORKFLOWS.iter().map(workflow_json).collect()
}

pub fn workflow_runtime_catalog(requested_account: Option<&str>) -> Result<Vec<Value>> {
    let doctor = doctor_payload(requested_account, None)?;
    let store = ActivityStore::load()?;
    Ok(WORKFLOWS
        .iter()
        .map(|definition| {
            let mut payload = workflow_json(definition);
            if let Some(object) = payload.as_object_mut() {
                object.insert(
                    "execution".to_string(),
                    workflow_execution_payload(definition.id, &doctor, store.entries()),
                );
            }
            payload
        })
        .collect())
}

pub fn workflow_json(definition: &WorkflowDefinition) -> Value {
    json!({
        "id": definition.id,
        "title": definition.title,
        "summary": definition.summary,
        "connects": definition.connects,
        "request_example": definition.request_example,
        "prompt_name": definition.prompt_name,
        "skill_name": definition.skill_name,
        "primary_tool": workflow_primary_tool(definition.id).unwrap_or_default(),
        "primary_operation": workflow_primary_operation(definition.id),
        "supports_review": workflow_supports_review(definition.id),
        "primary_tool_arguments": workflow_primary_tool_arguments(definition.id),
        "cli_steps": definition.cli_steps,
        "mcp_tools": definition.mcp_tools,
        "prompt_resource": format!("resource://yacli/skill/{}", definition.skill_name),
    })
}

pub fn workflow_resource_catalog() -> Value {
    json!({
        "workflows": workflow_catalog()
    })
}

pub fn workflow_resource_detail(id: &str) -> Option<Value> {
    workflow_definition(id).map(workflow_json)
}

pub fn workflow_runtime_detail(id: &str, requested_account: Option<&str>) -> Result<Option<Value>> {
    let Some(definition) = workflow_definition(id) else {
        return Ok(None);
    };
    let doctor = doctor_payload(requested_account, None)?;
    let store = ActivityStore::load()?;
    let mut payload = workflow_json(definition);
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "execution".to_string(),
            workflow_execution_payload(definition.id, &doctor, store.entries()),
        );
    }
    Ok(Some(payload))
}

fn workflow_execution_payload(id: &str, doctor: &Value, entries: &[ActivityEntry]) -> Value {
    let required_services = workflow_required_services(id);
    let missing_services = required_services
        .iter()
        .copied()
        .filter(|service| !doctor_service_ready(doctor, service))
        .collect::<Vec<_>>();

    let latest_activity = workflow_latest_activity(id, entries);
    let supports_review = workflow_supports_review(id);
    let supports_recovery = workflow_partial_operation(id).is_some();
    let supports_undo = latest_activity
        .as_ref()
        .map(|entry| activity_entry_supports_undo(entry))
        .unwrap_or(false);
    let can_replay = latest_activity.is_some();

    let (state, summary, next_action, available_actions) = if !missing_services.is_empty() {
        (
            "needs_input",
            format!(
                "Workflow ждёт подключения сервисов: {}.",
                missing_services.join(", ")
            ),
            "connect_services",
            vec!["connect_services"],
        )
    } else if let Some(entry) = latest_activity.as_ref() {
        if workflow_activity_is_undo(id, entry) {
            (
                "undone",
                format!("Последний запуск workflow уже откатан: {}.", entry.summary),
                "open_workflow",
                vec!["open_workflow", "review"],
            )
        } else if workflow_activity_is_partial_failure(id, entry) {
            (
                "recovery_ready",
                format!("Workflow завершился частично: {}.", entry.summary),
                "resume",
                vec!["resume", "share_replay"],
            )
        } else if workflow_activity_is_applied(id, entry) {
            (
                if activity_entry_supports_undo(entry) {
                    "undo_ready"
                } else {
                    "applied"
                },
                format!("Workflow уже выполнялся успешно: {}.", entry.summary),
                if activity_entry_supports_undo(entry) {
                    "undo"
                } else {
                    "replay"
                },
                if activity_entry_supports_undo(entry) {
                    vec!["undo", "share_replay"]
                } else {
                    vec!["share_replay", "open_workflow"]
                },
            )
        } else {
            (
                if supports_review {
                    "review_ready"
                } else {
                    "ready"
                },
                if supports_review {
                    "Workflow готов к review перед запуском.".to_string()
                } else {
                    "Workflow готов к следующему запуску.".to_string()
                },
                if supports_review {
                    "review"
                } else {
                    "open_workflow"
                },
                if supports_review {
                    vec!["review", "open_workflow"]
                } else {
                    vec!["open_workflow"]
                },
            )
        }
    } else {
        (
            if supports_review {
                "review_ready"
            } else {
                "ready"
            },
            if supports_review {
                "Workflow готов к первому review перед запуском.".to_string()
            } else {
                "Workflow готов к первому запуску.".to_string()
            },
            if supports_review {
                "review"
            } else {
                "open_workflow"
            },
            if supports_review {
                vec!["review", "open_workflow"]
            } else {
                vec!["open_workflow"]
            },
        )
    };

    let actions = workflow_execution_actions(id, next_action, &available_actions, latest_activity);

    json!({
        "state": state,
        "summary": summary,
        "required_services": required_services,
        "missing_services": missing_services,
        "supports_review": supports_review,
        "supports_recovery": supports_recovery,
        "supports_undo": supports_undo,
        "supports_replay": can_replay,
        "next_action": next_action,
        "available_actions": available_actions,
        "actions": actions,
        "latest_activity": latest_activity.map(activity_entry_json),
    })
}

fn workflow_execution_actions(
    id: &str,
    next_action: &str,
    available_actions: &[&str],
    latest_activity: Option<&ActivityEntry>,
) -> Vec<Value> {
    let mut actions = Vec::new();

    for action in available_actions {
        match *action {
            "connect_services" => actions.push(json!({
                "kind": "open_doctor",
                "label": "Connect services",
            })),
            "review" => actions.push(workflow_review_action(id)),
            "open_workflow" => actions.push(json!({
                "kind": "open_workflow_runner",
                "label": "Open workflow",
                "workflow_id": id,
            })),
            "resume" => {
                if let Some(entry) = latest_activity {
                    if let Some(action) = workflow_resume_action(id, entry) {
                        actions.push(action);
                    } else {
                        actions.push(json!({
                            "kind": "open_activity",
                            "label": "Resume workflow",
                            "workflow_id": id,
                            "activity_id": entry.id,
                            "operation": entry.operation,
                        }));
                    }
                }
            }
            "undo" => {
                if let Some(entry) = latest_activity {
                    actions.push(json!({
                        "kind": "undo_activity",
                        "label": "Undo workflow result",
                        "workflow_id": id,
                        "activity_id": entry.id,
                        "operation": entry.operation,
                    }));
                }
            }
            "share_replay" => {
                if let Some(entry) = latest_activity {
                    actions.push(json!({
                        "kind": "share_replay",
                        "label": "Share replay",
                        "workflow_id": id,
                        "activity_id": entry.id,
                        "operation": entry.operation,
                    }));
                }
            }
            _ => {}
        }
    }

    if next_action == "resume" && latest_activity.is_some() {
        if let Some(entry) = latest_activity.filter(|entry| activity_entry_supports_undo(entry)) {
            actions.push(json!({
                "kind": "undo_activity",
                "label": "Cleanup workflow result",
                "workflow_id": id,
                "activity_id": entry.id,
                "operation": entry.operation,
            }));
        }
    }

    actions
}

fn workflow_review_action(id: &str) -> Value {
    let tool_name = workflow_primary_tool(id).unwrap_or_default().to_string();
    let mut tool_arguments = workflow_primary_tool_arguments(id);
    if let Some(object) = tool_arguments.as_object_mut() {
        object.insert("dry_run".to_string(), json!(true));
    }
    json!({
        "kind": "review_tool",
        "label": "Preview workflow",
        "workflow_id": id,
        "tool_name": tool_name,
        "tool_arguments": tool_arguments,
    })
}

fn workflow_resume_action(id: &str, entry: &ActivityEntry) -> Option<Value> {
    let (tool_name, mut tool_arguments) = match id {
        "send-link-by-mail" => (
            "yacli.mail.send_published_link",
            parse_send_published_link_replay(&entry.replay_command)?,
        ),
        "invite-to-calendar" => (
            "yacli.calendar.create",
            parse_calendar_create_replay(&entry.replay_command)?,
        ),
        _ => return None,
    };
    if let Some(object) = tool_arguments.as_object_mut() {
        if !entry.account.trim().is_empty() {
            object
                .entry("account".to_string())
                .or_insert_with(|| json!(entry.account));
        }
        object.insert("dry_run".to_string(), json!(true));
    }
    Some(json!({
        "kind": "review_tool",
        "label": "Retry failed step",
        "workflow_id": id,
        "activity_id": entry.id,
        "operation": entry.operation,
        "tool_name": tool_name,
        "tool_arguments": tool_arguments,
    }))
}

fn parse_send_published_link_replay(command: &str) -> Option<Value> {
    let tokens = parse_shell_words(command)?;
    if tokens.len() < 5
        || tokens[0] != "yacli"
        || tokens[1] != "mail"
        || tokens[2] != "send-published-link"
    {
        return None;
    }

    let mut object = serde_json::Map::new();
    object.insert("to".to_string(), json!(tokens[3].clone()));
    object.insert("subject".to_string(), json!(tokens[4].clone()));
    let mut index = 5;
    if index < tokens.len() && !tokens[index].starts_with("--") {
        object.insert("text".to_string(), json!(tokens[index].clone()));
        index += 1;
    }

    let mut cc = Vec::new();
    let mut bcc = Vec::new();
    while index < tokens.len() {
        match tokens[index].as_str() {
            "--cc" => {
                index += 1;
                cc.push(tokens.get(index)?.clone());
            }
            "--bcc" => {
                index += 1;
                bcc.push(tokens.get(index)?.clone());
            }
            "--html" => {
                index += 1;
                object.insert("html".to_string(), json!(tokens.get(index)?.clone()));
            }
            "--public-url" => {
                index += 1;
                object.insert("public_url".to_string(), json!(tokens.get(index)?.clone()));
            }
            "--dry-run" => {}
            _ => return None,
        }
        index += 1;
    }
    if !cc.is_empty() {
        object.insert("cc".to_string(), json!(cc));
    }
    if !bcc.is_empty() {
        object.insert("bcc".to_string(), json!(bcc));
    }
    Some(Value::Object(object))
}

fn parse_calendar_create_replay(command: &str) -> Option<Value> {
    let tokens = parse_shell_words(command)?;
    if tokens.len() < 6 || tokens[0] != "yacli" || tokens[1] != "calendar" || tokens[2] != "create"
    {
        return None;
    }

    let mut object = serde_json::Map::new();
    object.insert("summary".to_string(), json!(tokens[3].clone()));
    object.insert("start".to_string(), json!(tokens[4].clone()));
    object.insert("end".to_string(), json!(tokens[5].clone()));

    let mut index = 6;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "--calendar" => {
                index += 1;
                object.insert("calendar".to_string(), json!(tokens.get(index)?.clone()));
            }
            "--location" => {
                index += 1;
                object.insert("location".to_string(), json!(tokens.get(index)?.clone()));
            }
            "--description" => {
                index += 1;
                object.insert("description".to_string(), json!(tokens.get(index)?.clone()));
            }
            "--dry-run" => {}
            _ => return None,
        }
        index += 1;
    }

    Some(Value::Object(object))
}

fn parse_shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '\\' if in_double => {
                current.push(chars.next()?);
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if in_single || in_double {
        return None;
    }
    if !current.is_empty() {
        words.push(current);
    }
    Some(words)
}

fn workflow_required_services(id: &str) -> &'static [&'static str] {
    match id {
        "daily-briefing" => &["mail", "calendar"],
        "reply-with-context" => &["mail", "calendar"],
        "attachment-to-disk" => &["mail"],
        "send-file-by-mail" => &["mail"],
        "send-link-by-mail" => &["mail", "disk"],
        "publish-file-link" => &["disk"],
        "revoke-public-link" => &["disk"],
        "invite-to-calendar" => &["mail", "calendar"],
        _ => &[],
    }
}

fn workflow_partial_operation(id: &str) -> Option<&'static str> {
    match id {
        "send-link-by-mail" => Some("mail.send_link.partial"),
        "invite-to-calendar" => Some("mail.invite.create_event.partial"),
        _ => None,
    }
}

fn doctor_service_ready(doctor: &Value, service: &str) -> bool {
    matches!(
        doctor["services"][service]["credential_state"].as_str(),
        Some("store_present" | "env_present")
    )
}

fn workflow_latest_activity<'a>(
    id: &str,
    entries: &'a [ActivityEntry],
) -> Option<&'a ActivityEntry> {
    let primary_operation = workflow_primary_operation(id);
    let partial_operation = workflow_partial_operation(id);
    entries.iter().find(|entry| {
        workflow_activity_is_undo(id, entry)
            || entry.operation == primary_operation
            || Some(entry.operation.as_str()) == partial_operation
    })
}

fn workflow_activity_is_applied(id: &str, entry: &ActivityEntry) -> bool {
    entry.operation == workflow_primary_operation(id)
}

fn workflow_activity_is_partial_failure(id: &str, entry: &ActivityEntry) -> bool {
    Some(entry.operation.as_str()) == workflow_partial_operation(id)
}

fn workflow_activity_is_undo(id: &str, entry: &ActivityEntry) -> bool {
    if entry.operation != "activity.undo" {
        return false;
    }

    match id {
        "publish-file-link" => entry.replay_command.starts_with("yacli disk unpublish "),
        _ => false,
    }
}

fn activity_entry_supports_undo(entry: &ActivityEntry) -> bool {
    entry
        .undo_command
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn activity_entry_json(entry: &ActivityEntry) -> Value {
    json!({
        "id": entry.id,
        "occurred_at": entry.occurred_at,
        "source": entry.source,
        "operation": entry.operation,
        "account": entry.account,
        "summary": entry.summary,
        "replay_command": entry.replay_command,
        "undo_command": entry.undo_command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_store::ActivityEntry;

    #[test]
    fn workflow_ids_cover_all_definitions() {
        let ids = workflow_ids();
        assert_eq!(ids.len(), 8);
        assert!(ids.contains(&"daily-briefing"));
        assert!(ids.contains(&"reply-with-context"));
        assert!(ids.contains(&"attachment-to-disk"));
        assert!(ids.contains(&"send-file-by-mail"));
        assert!(ids.contains(&"send-link-by-mail"));
        assert!(ids.contains(&"publish-file-link"));
        assert!(ids.contains(&"revoke-public-link"));
        assert!(ids.contains(&"invite-to-calendar"));
    }

    #[test]
    fn workflow_detail_exposes_prompt_and_skill_links() {
        let detail = workflow_resource_detail("invite-to-calendar").expect("detail");
        assert_eq!(detail["prompt_name"], "invite-to-calendar");
        assert_eq!(detail["skill_name"], "yacli-invite-to-calendar");
        assert_eq!(detail["primary_tool"], "yacli.mail.invite.create_event");
        assert_eq!(detail["primary_operation"], "mail.invite.create_event");
        assert_eq!(detail["supports_review"], false);
        assert_eq!(detail["primary_tool_arguments"]["folder"], "INBOX");
        assert_eq!(detail["primary_tool_arguments"]["calendar"], "team");
        assert_eq!(
            detail["prompt_resource"],
            "resource://yacli/skill/yacli-invite-to-calendar"
        );
    }

    #[test]
    fn workflow_primary_tool_prefers_actionable_tool_over_first_catalog_entry() {
        assert_eq!(
            workflow_primary_tool("invite-to-calendar"),
            Some("yacli.mail.invite.create_event")
        );
        assert_eq!(
            workflow_primary_tool("reply-with-context"),
            Some("yacli.mail.reply")
        );
    }

    #[test]
    fn workflow_primary_tool_arguments_are_canonical_for_send_link_flow() {
        let args = workflow_primary_tool_arguments("send-link-by-mail");
        assert_eq!(args["to"], "person@example.com");
        assert_eq!(args["source_path"], "./archive.zip");
        assert_eq!(args["disk_path"], "disk:/docs/archive/archive.zip");
        assert_eq!(args["overwrite"], false);
    }

    #[test]
    fn workflow_review_capability_tracks_primary_tool_contract() {
        assert!(workflow_supports_review("send-link-by-mail"));
        assert!(workflow_supports_review("publish-file-link"));
        assert!(workflow_supports_review("revoke-public-link"));
        assert!(!workflow_supports_review("daily-briefing"));
        assert!(!workflow_supports_review("invite-to-calendar"));
    }

    #[test]
    fn workflow_execution_marks_send_link_partial_failure() {
        let doctor = json!({
            "services": {
                "mail": { "credential_state": "store_present" },
                "disk": { "credential_state": "store_present" },
                "calendar": { "credential_state": "not_configured" }
            }
        });
        let entries = vec![ActivityEntry {
            id: "act_partial".to_string(),
            occurred_at: "2026-03-15T12:00:00Z".to_string(),
            source: "cli".to_string(),
            operation: "mail.send_link.partial".to_string(),
            account: "mock".to_string(),
            summary: "Публичная ссылка создана, но письмо не отправлено".to_string(),
            replay_command: "yacli mail send-published-link person@example.com 'Материалы' --public-url https://disk.example/public".to_string(),
            undo: None,
            undo_command: Some("yacli disk unpublish disk:/docs/archive.zip".to_string()),
        }];

        let payload = workflow_execution_payload("send-link-by-mail", &doctor, &entries);
        assert_eq!(payload["state"], "recovery_ready");
        assert_eq!(payload["next_action"], "resume");
        assert_eq!(payload["available_actions"][0], "resume");
        assert_eq!(payload["actions"][0]["kind"], "review_tool");
        assert_eq!(
            payload["actions"][0]["tool_name"],
            "yacli.mail.send_published_link"
        );
        assert_eq!(
            payload["actions"][0]["tool_arguments"]["public_url"],
            "https://disk.example/public"
        );
        assert_eq!(payload["actions"][1]["kind"], "share_replay");
        assert_eq!(payload["actions"][2]["kind"], "undo_activity");
        assert_eq!(
            payload["latest_activity"]["operation"],
            "mail.send_link.partial"
        );
    }

    #[test]
    fn workflow_execution_marks_publish_file_link_undone_after_activity_undo() {
        let doctor = json!({
            "services": {
                "mail": { "credential_state": "not_configured" },
                "disk": { "credential_state": "store_present" },
                "calendar": { "credential_state": "not_configured" }
            }
        });
        let entries = vec![ActivityEntry {
            id: "act_undo".to_string(),
            occurred_at: "2026-03-15T12:00:00Z".to_string(),
            source: "cli".to_string(),
            operation: "activity.undo".to_string(),
            account: "mock".to_string(),
            summary: "Откат действия act_publish: отозвана публичная ссылка".to_string(),
            replay_command: "yacli disk unpublish disk:/docs/archive.zip".to_string(),
            undo: None,
            undo_command: None,
        }];

        let payload = workflow_execution_payload("publish-file-link", &doctor, &entries);
        assert_eq!(payload["state"], "undone");
        assert_eq!(payload["next_action"], "open_workflow");
        assert_eq!(payload["latest_activity"]["operation"], "activity.undo");
    }

    #[test]
    fn workflow_execution_marks_needs_input_when_required_services_are_missing() {
        let doctor = json!({
            "services": {
                "mail": { "credential_state": "not_configured" },
                "disk": { "credential_state": "store_present" },
                "calendar": { "credential_state": "not_configured" }
            }
        });

        let payload = workflow_execution_payload("send-link-by-mail", &doctor, &[]);
        assert_eq!(payload["state"], "needs_input");
        assert_eq!(payload["missing_services"][0], "mail");
        assert_eq!(payload["next_action"], "connect_services");
    }

    #[test]
    fn workflow_execution_marks_review_ready_for_reviewable_flow_without_history() {
        let doctor = json!({
            "services": {
                "mail": { "credential_state": "store_present" },
                "disk": { "credential_state": "store_present" },
                "calendar": { "credential_state": "not_configured" }
            }
        });

        let payload = workflow_execution_payload("send-link-by-mail", &doctor, &[]);
        assert_eq!(payload["state"], "review_ready");
        assert_eq!(payload["next_action"], "review");
        assert_eq!(payload["available_actions"][0], "review");
        assert_eq!(payload["actions"][0]["kind"], "review_tool");
        assert_eq!(payload["actions"][0]["tool_name"], "yacli.mail.send_link");
    }

    #[test]
    fn workflow_execution_marks_undo_ready_for_reversible_applied_flow() {
        let doctor = json!({
            "services": {
                "mail": { "credential_state": "not_configured" },
                "disk": { "credential_state": "store_present" },
                "calendar": { "credential_state": "not_configured" }
            }
        });
        let entries = vec![ActivityEntry {
            id: "act_publish".to_string(),
            occurred_at: "2026-03-15T12:00:00Z".to_string(),
            source: "cli".to_string(),
            operation: "disk.upload_link".to_string(),
            account: "mock".to_string(),
            summary: "Файл загружен и опубликован".to_string(),
            replay_command:
                "yacli disk upload-link --source ./archive.zip --path disk:/docs/archive.zip"
                    .to_string(),
            undo: None,
            undo_command: Some("yacli disk unpublish disk:/docs/archive.zip".to_string()),
        }];

        let payload = workflow_execution_payload("publish-file-link", &doctor, &entries);
        assert_eq!(payload["state"], "undo_ready");
        assert_eq!(payload["next_action"], "undo");
        assert_eq!(payload["available_actions"][0], "undo");
        assert_eq!(payload["actions"][0]["kind"], "undo_activity");
    }

    #[test]
    fn parse_shell_words_supports_single_quoted_segments() {
        let words = parse_shell_words(
            "yacli mail send-published-link person@example.com 'Материалы ревью' 'Текст письма' --public-url 'https://disk.yandex.ru/i/report'",
        )
        .expect("words");
        assert_eq!(words[4], "Материалы ревью");
        assert_eq!(words[5], "Текст письма");
        assert_eq!(words[7], "https://disk.yandex.ru/i/report");
    }
}
