use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::activity_store::{ActivityEntry, ActivityStore};
use crate::disk::{DiskResourceItem, PrivateDiskListRequest, fetch_private_resource};
use crate::doctor::doctor_payload;
use crate::error::Result;
use crate::goal_router::goal_route_payload;
use crate::mail::{
    MailAttachmentSummary, MailMessage, list_mail_messages, read_mail_message,
    smtp_safe_message_bytes,
};
use crate::runtime_context::{resolve_disk_private_context, resolve_mail_private_context};
use crate::workflows;

const LIVE_MAILBOX_NAME: &str = "INBOX";
const LIVE_MAIL_SCAN_LIMIT: usize = 5;
const LIVE_MAIL_MAX_BYTES: u64 = 1024 * 1024;
const LIVE_DISK_SCAN_PATH: &str = "disk:/";
const LIVE_DISK_SCAN_LIMIT: usize = 50;

#[derive(Clone, Debug, Serialize)]
struct SuggestionAction {
    kind: &'static str,
    label: &'static str,
    workflow_id: Option<&'static str>,
    activity_id: Option<String>,
    primary_tool: Option<&'static str>,
    primary_operation: Option<&'static str>,
    supports_review: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_arguments: Option<Value>,
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

struct LiveMailMessageContext<'a> {
    account: &'a str,
    mailbox_name: &'a str,
    message_uid: u64,
    subject: Option<&'a str>,
    from: Option<&'a str>,
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
    let current_account = doctor["current_account"].as_str();
    let mut suggestions = collect_suggestions(store.entries(), goal_workflow);
    let mut seen_commands = suggestions
        .iter()
        .map(|item| item.command.clone())
        .collect::<BTreeSet<_>>();
    collect_live_mail_suggestions(
        requested_account.or(current_account),
        goal_workflow,
        &mut suggestions,
        &mut seen_commands,
    );
    collect_live_disk_suggestions(
        requested_account.or(current_account),
        goal_workflow,
        &mut suggestions,
        &mut seen_commands,
    );
    suggestions.sort_by_key(|item| item.priority);
    suggestions.truncate(5);
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
        tool_name: None,
        tool_arguments: None,
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
        tool_name: None,
        tool_arguments: None,
    }
}

fn open_tool_action(
    workflow_id: Option<&'static str>,
    primary_operation: Option<&'static str>,
    tool_name: &'static str,
    tool_arguments: Value,
    supports_review: bool,
) -> SuggestionAction {
    SuggestionAction {
        kind: "open_tool",
        label: "Open tool",
        workflow_id,
        activity_id: None,
        primary_tool: Some(tool_name),
        primary_operation,
        supports_review,
        tool_name: Some(tool_name),
        tool_arguments: Some(tool_arguments),
    }
}

fn collect_live_mail_suggestions(
    requested_account: Option<&str>,
    goal_workflow: Option<&str>,
    suggestions: &mut Vec<SuggestionItem>,
    seen: &mut BTreeSet<String>,
) {
    let Ok((resolved_account, auth, context)) = resolve_mail_private_context(requested_account)
    else {
        return;
    };
    let Ok(messages) = list_mail_messages(
        &context.imap_host,
        context.imap_port,
        auth.clone(),
        LIVE_MAILBOX_NAME,
        LIVE_MAIL_SCAN_LIMIT,
    ) else {
        return;
    };

    for message in &messages {
        if let Some(size) = message.size
            && size > LIVE_MAIL_MAX_BYTES
        {
            continue;
        }

        let Ok(full_message) = read_mail_message(
            &context.imap_host,
            context.imap_port,
            auth.clone(),
            LIVE_MAILBOX_NAME,
            message.uid,
            LIVE_MAIL_MAX_BYTES,
        ) else {
            continue;
        };

        let Some((attachment_index, attachment)) = first_invite_attachment(&full_message) else {
            continue;
        };

        push_suggestion(
            suggestions,
            seen,
            build_live_mail_invite_suggestion(
                LiveMailMessageContext {
                    account: &resolved_account,
                    mailbox_name: LIVE_MAILBOX_NAME,
                    message_uid: full_message.uid,
                    subject: full_message.subject.as_deref(),
                    from: full_message.from.as_deref(),
                },
                attachment_index,
                attachment,
                goal_workflow,
            ),
        );
        break;
    }

    for message in messages {
        if let Some(size) = message.size
            && size > LIVE_MAIL_MAX_BYTES
        {
            continue;
        }

        let Ok(full_message) = read_mail_message(
            &context.imap_host,
            context.imap_port,
            auth.clone(),
            LIVE_MAILBOX_NAME,
            message.uid,
            LIVE_MAIL_MAX_BYTES,
        ) else {
            continue;
        };

        let Some((attachment_index, attachment)) = first_regular_attachment(&full_message) else {
            continue;
        };

        push_suggestion(
            suggestions,
            seen,
            build_live_mail_attachment_suggestion(
                LiveMailMessageContext {
                    account: &resolved_account,
                    mailbox_name: LIVE_MAILBOX_NAME,
                    message_uid: full_message.uid,
                    subject: full_message.subject.as_deref(),
                    from: full_message.from.as_deref(),
                },
                full_message.size,
                attachment_index,
                attachment,
                goal_workflow,
            ),
        );
        break;
    }
}

fn collect_live_disk_suggestions(
    requested_account: Option<&str>,
    goal_workflow: Option<&str>,
    suggestions: &mut Vec<SuggestionItem>,
    seen: &mut BTreeSet<String>,
) {
    let Ok((resolved_account, base_url, access_token)) =
        resolve_disk_private_context(requested_account)
    else {
        return;
    };
    let Ok(root) = fetch_private_resource(
        &base_url,
        &access_token,
        &PrivateDiskListRequest {
            path: LIVE_DISK_SCAN_PATH.to_string(),
            limit: LIVE_DISK_SCAN_LIMIT,
            offset: 0,
        },
    ) else {
        return;
    };

    let Some(public_item) = root
        .children
        .as_ref()
        .and_then(|children| children.items.iter().find(|item| is_public_disk_item(item)))
    else {
        return;
    };

    push_suggestion(
        suggestions,
        seen,
        build_live_disk_public_link_suggestion(&resolved_account, public_item, goal_workflow),
    );
}

fn build_live_mail_invite_suggestion(
    context: LiveMailMessageContext<'_>,
    attachment_index: usize,
    attachment: &MailAttachmentSummary,
    goal_workflow: Option<&str>,
) -> SuggestionItem {
    let subject = context.subject.unwrap_or("Без темы");
    let from = context.from.unwrap_or("-");
    let command = format!(
        "yacli mail invite inspect --account {} --folder {} {} --index {}",
        shell_quote(context.account),
        shell_quote(context.mailbox_name),
        context.message_uid,
        attachment_index
    );
    SuggestionItem {
        id: format!(
            "live-mail-invite-{}-{}",
            context.message_uid, attachment_index
        ),
        title: "Разобрать свежее календарное приглашение".to_string(),
        status: "ready",
        priority: goal_adjusted_priority(2, "invite-to-calendar", goal_workflow),
        reason: format!(
            "Письмо \"{subject}\" от {from} в {} содержит calendar invite {}. Можно сразу открыть и разобрать приглашение.",
            context.mailbox_name,
            attachment_label(attachment)
        ),
        command,
        source: "mail_live",
        kind: "discovery",
        activity_id: format!("mail:{}:{}", context.message_uid, attachment_index),
        operation: "mail.invite.detected".to_string(),
        workflow_id: Some("invite-to-calendar"),
        action: open_tool_action(
            Some("invite-to-calendar"),
            Some("mail.invite.inspect"),
            "yacli.mail.invite.inspect",
            json!({
                "account": context.account,
                "folder": context.mailbox_name,
                "uid": context.message_uid,
                "index": attachment_index,
            }),
            false,
        ),
    }
}

fn build_live_mail_attachment_suggestion(
    context: LiveMailMessageContext<'_>,
    message_size: Option<u64>,
    attachment_index: usize,
    attachment: &MailAttachmentSummary,
    goal_workflow: Option<&str>,
) -> SuggestionItem {
    let subject = context.subject.unwrap_or("Без темы");
    let from = context.from.unwrap_or("-");
    let output_path =
        suggested_attachment_output_path(attachment, context.message_uid, attachment_index);
    let oversized = message_size.is_some_and(is_oversized_mail_message);
    let command = format!(
        "yacli mail attachment export --account {} --folder {} {} --index {} --output {}",
        shell_quote(context.account),
        shell_quote(context.mailbox_name),
        context.message_uid,
        attachment_index,
        shell_quote(&output_path)
    );
    SuggestionItem {
        id: format!(
            "live-mail-attachment-{}-{}",
            context.message_uid, attachment_index
        ),
        title: if oversized {
            "Сохранить крупное вложение и дальше работать через ссылку".to_string()
        } else {
            "Сохранить свежее вложение на диск".to_string()
        },
        status: "ready",
        priority: goal_adjusted_priority(
            if oversized { 1 } else { 3 },
            "attachment-to-disk",
            goal_workflow,
        ),
        reason: if oversized {
            format!(
                "Письмо \"{subject}\" от {from} в {} весит {} байт и содержит вложение {}. Безопаснее сначала выгрузить файл локально, а затем при необходимости отправлять его через workflow `send-link-by-mail`.",
                context.mailbox_name,
                message_size.unwrap_or_default(),
                attachment_label(attachment)
            )
        } else {
            format!(
                "Письмо \"{subject}\" от {from} в {} содержит вложение {}. Можно сразу открыть export с готовым путём сохранения.",
                context.mailbox_name,
                attachment_label(attachment)
            )
        },
        command,
        source: "mail_live",
        kind: "discovery",
        activity_id: format!("mail:{}:{}", context.message_uid, attachment_index),
        operation: "mail.attachment.detected".to_string(),
        workflow_id: Some("attachment-to-disk"),
        action: open_tool_action(
            Some("attachment-to-disk"),
            Some("mail.attachment.export"),
            "yacli.mail.attachment.export",
            json!({
                "account": context.account,
                "folder": context.mailbox_name,
                "uid": context.message_uid,
                "index": attachment_index,
                "output_path": output_path,
            }),
            false,
        ),
    }
}

fn build_live_disk_public_link_suggestion(
    account: &str,
    item: &DiskResourceItem,
    goal_workflow: Option<&str>,
) -> SuggestionItem {
    let command = format!(
        "yacli disk unpublish --account {} {} --dry-run",
        shell_quote(account),
        shell_quote(&item.path)
    );
    let public_url = item.public_url.as_deref().unwrap_or("-");
    SuggestionItem {
        id: format!("live-disk-public-{}", item.path),
        title: "Отозвать живую публичную ссылку".to_string(),
        status: "ready",
        priority: goal_adjusted_priority(2, "revoke-public-link", goal_workflow),
        reason: format!(
            "На Диске уже опубликован ресурс {} ({public_url}). Если ссылка больше не нужна, revoke доступен сразу из live state.",
            item.path
        ),
        command,
        source: "disk_live",
        kind: "cleanup",
        activity_id: format!("disk:{}", item.path),
        operation: "disk.publish.detected".to_string(),
        workflow_id: Some("revoke-public-link"),
        action: open_tool_action(
            Some("revoke-public-link"),
            Some("disk.unpublish"),
            "yacli.disk.unpublish",
            json!({
                "account": account,
                "path": item.path,
            }),
            true,
        ),
    }
}

fn first_invite_attachment(message: &MailMessage) -> Option<(usize, &MailAttachmentSummary)> {
    first_invite_attachment_in_summaries(&message.attachments)
}

fn first_regular_attachment(message: &MailMessage) -> Option<(usize, &MailAttachmentSummary)> {
    first_regular_attachment_in_summaries(&message.attachments)
}

fn first_invite_attachment_in_summaries(
    attachments: &[MailAttachmentSummary],
) -> Option<(usize, &MailAttachmentSummary)> {
    attachments
        .iter()
        .enumerate()
        .find(|(_, attachment)| is_calendar_invite_attachment(attachment))
        .map(|(index, attachment)| (index + 1, attachment))
}

fn first_regular_attachment_in_summaries(
    attachments: &[MailAttachmentSummary],
) -> Option<(usize, &MailAttachmentSummary)> {
    attachments
        .iter()
        .enumerate()
        .find(|(_, attachment)| !is_calendar_invite_attachment(attachment))
        .map(|(index, attachment)| (index + 1, attachment))
}

fn is_calendar_invite_attachment(attachment: &MailAttachmentSummary) -> bool {
    attachment.mime_type.eq_ignore_ascii_case("text/calendar")
        || attachment
            .filename
            .as_deref()
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".ics"))
}

fn is_oversized_mail_message(size: u64) -> bool {
    size > smtp_safe_message_bytes()
}

fn is_public_disk_item(item: &DiskResourceItem) -> bool {
    item.public_url.is_some() || item.public_key.is_some()
}

fn attachment_label(attachment: &MailAttachmentSummary) -> String {
    attachment
        .filename
        .clone()
        .unwrap_or_else(|| attachment.mime_type.clone())
}

fn suggested_attachment_output_path(
    attachment: &MailAttachmentSummary,
    message_uid: u64,
    attachment_index: usize,
) -> String {
    let filename = attachment
        .filename
        .as_deref()
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("attachment-{message_uid}-{attachment_index}"));
    format!("./{filename}")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if !value
        .chars()
        .any(|ch| matches!(ch, ' ' | '\t' | '\n' | '\'' | '"' | '\\'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
                "Для цели `{goal}` пока нет proactive suggestions из последних действий и live product signals."
            ),
            None => "Пока нет proactive suggestions из последних действий и live product signals."
                .to_string(),
        };
    }

    match goal {
        Some(goal) => format!(
            "Для цели `{goal}` найдено {} proactive suggestions из activity history и live product signals.",
            suggestions.len()
        ),
        None => format!(
            "Найдено {} proactive suggestions из activity history и live product signals.",
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
    use super::{
        LiveMailMessageContext, build_live_disk_public_link_suggestion,
        build_live_mail_attachment_suggestion, build_live_mail_invite_suggestion,
        collect_suggestions, first_invite_attachment_in_summaries,
        first_regular_attachment_in_summaries, is_public_disk_item, summary_for_suggestions,
    };
    use crate::activity_store::ActivityEntry;
    use crate::disk::DiskResourceItem;
    use crate::mail::{MailAttachmentSummary, smtp_safe_message_bytes};

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

    #[test]
    fn first_invite_attachment_prefers_ics_and_text_calendar() {
        let attachments = vec![
            MailAttachmentSummary {
                filename: Some("report.pdf".to_string()),
                mime_type: "application/pdf".to_string(),
                content_id: None,
                inline: false,
            },
            MailAttachmentSummary {
                filename: Some("invite.ics".to_string()),
                mime_type: "application/octet-stream".to_string(),
                content_id: None,
                inline: false,
            },
        ];

        let (index, attachment) =
            first_invite_attachment_in_summaries(&attachments).expect("invite attachment");
        assert_eq!(index, 2);
        assert_eq!(attachment.filename.as_deref(), Some("invite.ics"));
    }

    #[test]
    fn suggestions_summary_mentions_live_signals() {
        let summary = summary_for_suggestions(&[], Some("добавь встречу из письма"));
        assert!(summary.contains("live product signals"));
    }

    #[test]
    fn build_live_mail_invite_suggestion_returns_open_tool_handoff() {
        let attachments = [MailAttachmentSummary {
            filename: Some("invite.ics".to_string()),
            mime_type: "text/calendar".to_string(),
            content_id: None,
            inline: false,
        }];
        let suggestion = build_live_mail_invite_suggestion(
            LiveMailMessageContext {
                account: "mock",
                mailbox_name: "INBOX",
                message_uid: 1353,
                subject: Some("Ревью yacli"),
                from: Some("organizer@example.com"),
            },
            1,
            &attachments[0],
            Some("invite-to-calendar"),
        );
        assert_eq!(suggestion.workflow_id, Some("invite-to-calendar"));
        assert_eq!(suggestion.source, "mail_live");
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(
            suggestion.action.tool_name,
            Some("yacli.mail.invite.inspect")
        );
        assert_eq!(
            suggestion.action.primary_tool,
            Some("yacli.mail.invite.inspect")
        );
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|v| v["uid"].as_u64()),
            Some(1353)
        );
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|v| v["index"].as_u64()),
            Some(1)
        );
    }

    #[test]
    fn first_regular_attachment_skips_calendar_invites() {
        let attachments = vec![
            MailAttachmentSummary {
                filename: Some("invite.ics".to_string()),
                mime_type: "text/calendar".to_string(),
                content_id: None,
                inline: false,
            },
            MailAttachmentSummary {
                filename: Some("report.pdf".to_string()),
                mime_type: "application/pdf".to_string(),
                content_id: None,
                inline: false,
            },
        ];

        let (index, attachment) =
            first_regular_attachment_in_summaries(&attachments).expect("regular attachment");
        assert_eq!(index, 2);
        assert_eq!(attachment.filename.as_deref(), Some("report.pdf"));
    }

    #[test]
    fn build_live_mail_attachment_suggestion_returns_open_tool_handoff() {
        let attachments = [MailAttachmentSummary {
            filename: Some("quarterly-report.pdf".to_string()),
            mime_type: "application/pdf".to_string(),
            content_id: None,
            inline: false,
        }];
        let suggestion = build_live_mail_attachment_suggestion(
            LiveMailMessageContext {
                account: "mock",
                mailbox_name: "INBOX",
                message_uid: 2468,
                subject: Some("Материалы"),
                from: Some("sender@example.com"),
            },
            Some(1024),
            1,
            &attachments[0],
            Some("attachment-to-disk"),
        );
        assert_eq!(suggestion.workflow_id, Some("attachment-to-disk"));
        assert_eq!(suggestion.source, "mail_live");
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(
            suggestion.action.tool_name,
            Some("yacli.mail.attachment.export")
        );
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|v| v["uid"].as_u64()),
            Some(2468)
        );
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|v| v["output_path"].as_str()),
            Some("./quarterly-report.pdf")
        );
    }

    #[test]
    fn build_live_mail_attachment_suggestion_for_oversized_mail_mentions_send_link_path() {
        let attachments = [MailAttachmentSummary {
            filename: Some("archive.zip".to_string()),
            mime_type: "application/zip".to_string(),
            content_id: None,
            inline: false,
        }];
        let suggestion = build_live_mail_attachment_suggestion(
            LiveMailMessageContext {
                account: "mock",
                mailbox_name: "INBOX",
                message_uid: 777,
                subject: Some("Большой архив"),
                from: Some("sender@example.com"),
            },
            Some(smtp_safe_message_bytes() + 1),
            1,
            &attachments[0],
            None,
        );
        assert!(suggestion.title.contains("крупное вложение"));
        assert!(suggestion.reason.contains("send-link-by-mail"));
        assert_eq!(suggestion.workflow_id, Some("attachment-to-disk"));
        assert_eq!(
            suggestion.action.tool_name,
            Some("yacli.mail.attachment.export")
        );
    }

    #[test]
    fn public_disk_item_detection_uses_public_url_or_key() {
        let public_item = DiskResourceItem {
            name: "report.pdf".to_string(),
            path: "disk:/docs/report.pdf".to_string(),
            resource_type: "file".to_string(),
            mime_type: Some("application/pdf".to_string()),
            size: Some(1024),
            created: None,
            modified: None,
            md5: None,
            revision: None,
            public_url: Some("https://disk.yandex.ru/i/report".to_string()),
            public_key: None,
        };
        let private_item = DiskResourceItem {
            public_url: None,
            public_key: None,
            ..public_item.clone()
        };
        assert!(is_public_disk_item(&public_item));
        assert!(!is_public_disk_item(&private_item));
    }

    #[test]
    fn build_live_disk_public_link_suggestion_returns_open_tool_handoff() {
        let item = DiskResourceItem {
            name: "report.pdf".to_string(),
            path: "disk:/docs/report.pdf".to_string(),
            resource_type: "file".to_string(),
            mime_type: Some("application/pdf".to_string()),
            size: Some(1024),
            created: None,
            modified: None,
            md5: None,
            revision: None,
            public_url: Some("https://disk.yandex.ru/i/report".to_string()),
            public_key: Some("public-key-123".to_string()),
        };
        let suggestion =
            build_live_disk_public_link_suggestion("mock", &item, Some("revoke-public-link"));
        assert_eq!(suggestion.workflow_id, Some("revoke-public-link"));
        assert_eq!(suggestion.source, "disk_live");
        assert_eq!(suggestion.kind, "cleanup");
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(suggestion.action.tool_name, Some("yacli.disk.unpublish"));
        assert_eq!(suggestion.action.primary_tool, Some("yacli.disk.unpublish"));
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|v| v["path"].as_str()),
            Some("disk:/docs/report.pdf")
        );
        assert!(suggestion.command.contains("--dry-run"));
    }
}
