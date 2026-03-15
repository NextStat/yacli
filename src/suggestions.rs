use std::collections::BTreeSet;
use std::path::Path;

use chrono::{Days, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use crate::activity_store::{ActivityEntry, ActivityStore};
use crate::calendar::{CalendarEvent, CalendarEventsRequest, list_calendar_events, list_calendars};
use crate::disk::{DiskResourceItem, PrivateDiskListRequest, fetch_private_resource};
use crate::doctor::doctor_payload;
use crate::error::Result;
use crate::goal_router::goal_route_payload;
use crate::mail::{
    MailAttachmentSummary, MailMessage, MailMessageSummary, list_mail_messages, read_mail_message,
    smtp_safe_message_bytes,
};
use crate::runtime_context::{
    resolve_calendar_private_context, resolve_disk_private_context, resolve_mail_private_context,
};
use crate::workflows;

const LIVE_MAILBOX_NAME: &str = "INBOX";
const LIVE_MAIL_SCAN_LIMIT: usize = 5;
const LIVE_MAIL_MAX_BYTES: u64 = 1024 * 1024;
const LIVE_DISK_SCAN_PATH: &str = "disk:/";
const LIVE_DISK_SCAN_LIMIT: usize = 50;
const LIVE_CALENDAR_LOOKAHEAD_DAYS: u64 = 14;
const LIVE_CALENDAR_EVENT_LIMIT: usize = 20;
const LIVE_CALENDAR_MATERIALS_WINDOW_DAYS: u64 = 3;
const LIVE_CALENDAR_FOLLOW_UP_LOOKBACK_DAYS: u64 = 2;

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
    collect_live_calendar_suggestions(
        requested_account.or(current_account),
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
    let Some((resolved_account, root_items)) = fetch_live_disk_root_items(requested_account) else {
        return;
    };
    let Some(public_item) = root_items.iter().find(|item| is_public_disk_item(item)) else {
        return;
    };

    push_suggestion(
        suggestions,
        seen,
        build_live_disk_send_link_suggestion(&resolved_account, public_item, goal_workflow),
    );

    push_suggestion(
        suggestions,
        seen,
        build_live_disk_public_link_suggestion(&resolved_account, public_item, goal_workflow),
    );
}

fn collect_live_calendar_suggestions(
    requested_account: Option<&str>,
    suggestions: &mut Vec<SuggestionItem>,
    seen: &mut BTreeSet<String>,
) {
    let Ok((resolved_account, app_password, context)) =
        resolve_calendar_private_context(requested_account)
    else {
        return;
    };
    let Ok(calendars) = list_calendars(&context.caldav_base_url, &context.email, &app_password)
    else {
        return;
    };
    let now = Utc::now();
    let Some(to) = now.checked_add_days(Days::new(LIVE_CALENDAR_LOOKAHEAD_DAYS)) else {
        return;
    };
    let disk_root = fetch_live_disk_root_items(Some(&resolved_account));
    let mail_inbox = fetch_live_mail_summaries(Some(&resolved_account));
    let mut cancelled_suggestion = None;
    let mut materials_suggestion = None;
    let mut follow_up_suggestion = None;
    let mut reply_suggestion = None;

    for calendar in calendars {
        let Ok((resolved_calendar, _, events)) = list_calendar_events(
            &context.caldav_base_url,
            &context.email,
            &app_password,
            CalendarEventsRequest {
                calendar: calendar.id.clone(),
                from: now,
                to,
                limit: LIVE_CALENDAR_EVENT_LIMIT,
            },
        ) else {
            continue;
        };

        if cancelled_suggestion.is_none()
            && let Some(event) = events
                .iter()
                .find(|event| is_cancelled_calendar_event(event))
        {
            cancelled_suggestion = Some(build_live_calendar_cancelled_suggestion(
                &resolved_account,
                &resolved_calendar.id,
                event,
            ));
        }

        if materials_suggestion.is_none()
            && let Some((disk_account, root_items)) = disk_root.as_ref()
            && disk_account == &resolved_account
            && let Some(event) = first_upcoming_materials_event(&events, now)
            && let Some(path) = suggested_calendar_materials_path(event)
            && !disk_path_exists(root_items, &path)
        {
            materials_suggestion = Some(build_live_calendar_materials_suggestion(
                &resolved_account,
                event,
                &path,
            ));
        }

        if follow_up_suggestion.is_none()
            && let Some((disk_account, root_items)) = disk_root.as_ref()
            && disk_account == &resolved_account
            && let Some(event) = first_recent_follow_up_event(&events, now)
            && let Some(path) = suggested_calendar_materials_path(event)
            && let Some(item) = root_items.iter().find(|item| item.path == path)
            && !is_public_disk_item(item)
        {
            follow_up_suggestion = Some(build_live_calendar_publish_materials_suggestion(
                &resolved_account,
                event,
                item,
            ));
        }

        if reply_suggestion.is_none()
            && let Some((mail_account, messages)) = mail_inbox.as_ref()
            && mail_account == &resolved_account
            && let Some(event) = first_recent_follow_up_event(&events, now)
            && let Some(message) = first_recent_follow_up_message(messages, event)
        {
            reply_suggestion = Some(build_live_calendar_mail_follow_up_suggestion(
                &resolved_account,
                event,
                message,
            ));
        }

        if cancelled_suggestion.is_some()
            && materials_suggestion.is_some()
            && follow_up_suggestion.is_some()
            && reply_suggestion.is_some()
        {
            break;
        }
    }

    if let Some(suggestion) = cancelled_suggestion {
        push_suggestion(suggestions, seen, suggestion);
    }
    if let Some(suggestion) = materials_suggestion {
        push_suggestion(suggestions, seen, suggestion);
    }
    if let Some(suggestion) = follow_up_suggestion {
        push_suggestion(suggestions, seen, suggestion);
    }
    if let Some(suggestion) = reply_suggestion {
        push_suggestion(suggestions, seen, suggestion);
    }
}

fn fetch_live_disk_root_items(
    requested_account: Option<&str>,
) -> Option<(String, Vec<DiskResourceItem>)> {
    let Ok((resolved_account, base_url, access_token)) =
        resolve_disk_private_context(requested_account)
    else {
        return None;
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
        return None;
    };
    let items = root
        .children
        .map(|children| children.items)
        .unwrap_or_default();
    Some((resolved_account, items))
}

fn fetch_live_mail_summaries(
    requested_account: Option<&str>,
) -> Option<(String, Vec<MailMessageSummary>)> {
    let Ok((resolved_account, auth, context)) = resolve_mail_private_context(requested_account)
    else {
        return None;
    };
    let Ok(messages) = list_mail_messages(
        &context.imap_host,
        context.imap_port,
        auth,
        LIVE_MAILBOX_NAME,
        LIVE_MAIL_SCAN_LIMIT,
    ) else {
        return None;
    };
    Some((resolved_account, messages))
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

fn build_live_disk_send_link_suggestion(
    account: &str,
    item: &DiskResourceItem,
    goal_workflow: Option<&str>,
) -> SuggestionItem {
    let public_url = item.public_url.as_deref().unwrap_or("-");
    let subject = format!("Материалы: {}", item.name);
    let text = format!("Отправляю ссылку на файл {}.", item.name);
    let command = format!(
        "yacli mail send-published-link <email> {} {} --public-url {} --dry-run",
        shell_quote(&subject),
        shell_quote(&text),
        shell_quote(public_url)
    );
    SuggestionItem {
        id: format!("live-disk-send-link-{}", item.path),
        title: "Отправить уже опубликованную ссылку по почте".to_string(),
        status: "ready",
        priority: goal_adjusted_priority(1, "send-link-by-mail", goal_workflow),
        reason: format!(
            "На Диске уже опубликован ресурс {} ({public_url}). Можно сразу перейти в mail-step и отправить эту ссылку без повторного upload/publish.",
            item.path
        ),
        command,
        source: "disk_live",
        kind: "follow_up",
        activity_id: format!("disk:{}", item.path),
        operation: "disk.publish.follow_up".to_string(),
        workflow_id: Some("send-link-by-mail"),
        action: open_tool_action(
            Some("send-link-by-mail"),
            Some("mail.send_published_link"),
            "yacli.mail.send_published_link",
            json!({
                "account": account,
                "subject": subject,
                "text": text,
                "public_url": public_url,
            }),
            false,
        ),
    }
}

fn build_live_calendar_cancelled_suggestion(
    account: &str,
    calendar_id: &str,
    event: &CalendarEvent,
) -> SuggestionItem {
    let uid = event.uid.as_deref().unwrap_or_default();
    let summary = event.summary.as_deref().unwrap_or("Без названия");
    let start = event.start.as_deref().unwrap_or("-");
    let command = format!(
        "yacli calendar delete --calendar {} {}",
        shell_quote(calendar_id),
        shell_quote(uid)
    );
    SuggestionItem {
        id: format!("live-calendar-cancelled-{}", uid),
        title: "Удалить отменённое событие из календаря".to_string(),
        status: "ready",
        priority: 2,
        reason: format!(
            "В календаре {} есть событие \"{}\" со статусом CANCELLED на {}. Его можно сразу убрать как live cleanup.",
            event.calendar_name, summary, start
        ),
        command,
        source: "calendar_live",
        kind: "cleanup",
        activity_id: format!("calendar:{calendar_id}:{uid}"),
        operation: "calendar.cancelled.detected".to_string(),
        workflow_id: None,
        action: open_tool_action(
            None,
            Some("calendar.delete"),
            "yacli.calendar.delete",
            json!({
                "account": account,
                "calendar": calendar_id,
                "uid": uid,
            }),
            false,
        ),
    }
}

fn build_live_calendar_materials_suggestion(
    account: &str,
    event: &CalendarEvent,
    path: &str,
) -> SuggestionItem {
    let summary = event.summary.as_deref().unwrap_or("Без названия");
    let start = event.start.as_deref().unwrap_or("-");
    let command = format!(
        "yacli disk mkdir --account {} {}",
        shell_quote(account),
        shell_quote(path)
    );
    SuggestionItem {
        id: format!("live-calendar-materials-{}", path),
        title: "Подготовить папку на Диске для ближайшей встречи".to_string(),
        status: "ready",
        priority: 4,
        reason: format!(
            "В календаре есть ближайшее событие \"{}\" на {}. Можно заранее подготовить папку {} для материалов и файлов по встрече.",
            summary, start, path
        ),
        command,
        source: "calendar_live",
        kind: "follow_up",
        activity_id: format!("calendar:{}:{}", event.calendar_id, path),
        operation: "calendar.materials_folder.suggested".to_string(),
        workflow_id: None,
        action: open_tool_action(
            None,
            Some("disk.mkdir"),
            "yacli.disk.mkdir",
            json!({
                "account": account,
                "path": path,
            }),
            false,
        ),
    }
}

fn build_live_calendar_publish_materials_suggestion(
    account: &str,
    event: &CalendarEvent,
    item: &DiskResourceItem,
) -> SuggestionItem {
    let summary = event.summary.as_deref().unwrap_or("Без названия");
    let end = event.end.as_deref().unwrap_or("-");
    let command = format!(
        "yacli disk publish --account {} {} --dry-run",
        shell_quote(account),
        shell_quote(&item.path)
    );
    SuggestionItem {
        id: format!("live-calendar-publish-materials-{}", item.path),
        title: "Опубликовать материалы для завершившейся встречи".to_string(),
        status: "ready",
        priority: 3,
        reason: format!(
            "Событие \"{}\" уже завершилось в {}. На Диске есть папка {} без публичной ссылки, её можно сразу опубликовать для follow-up материалов.",
            summary, end, item.path
        ),
        command,
        source: "calendar_live",
        kind: "follow_up",
        activity_id: format!("calendar:{}:{}", event.calendar_id, item.path),
        operation: "calendar.materials_folder.follow_up".to_string(),
        workflow_id: Some("publish-file-link"),
        action: open_tool_action(
            Some("publish-file-link"),
            Some("disk.publish"),
            "yacli.disk.publish",
            json!({
                "account": account,
                "path": item.path,
            }),
            true,
        ),
    }
}

fn build_live_calendar_mail_follow_up_suggestion(
    account: &str,
    event: &CalendarEvent,
    message: &MailMessageSummary,
) -> SuggestionItem {
    let event_summary = event.summary.as_deref().unwrap_or("Без названия");
    let mail_subject = message.subject.as_deref().unwrap_or("Без темы");
    let from = message.from.as_deref().unwrap_or("-");
    let suggested_text =
        format!("Спасибо за встречу \"{event_summary}\". Отправляю follow-up по итогам.");
    let command = format!(
        "yacli mail reply --account {} {} {}",
        shell_quote(account),
        message.uid,
        shell_quote(&suggested_text)
    );
    SuggestionItem {
        id: format!("live-calendar-mail-follow-up-{}", message.uid),
        title: "Ответить на свежее письмо после встречи".to_string(),
        status: "ready",
        priority: 2,
        reason: format!(
            "После события \"{}\" в INBOX есть свежее письмо \"{}\" от {} с похожей темой. Можно сразу открыть follow-up reply с уже выбранным письмом.",
            event_summary, mail_subject, from
        ),
        command,
        source: "calendar_live",
        kind: "follow_up",
        activity_id: format!("calendar:{}:mail:{}", event.calendar_id, message.uid),
        operation: "calendar.mail_follow_up.suggested".to_string(),
        workflow_id: Some("reply-with-context"),
        action: open_tool_action(
            Some("reply-with-context"),
            Some("mail.reply"),
            "yacli.mail.reply",
            json!({
                "account": account,
                "uid": message.uid,
                "text": suggested_text,
            }),
            false,
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

fn is_cancelled_calendar_event(event: &CalendarEvent) -> bool {
    event
        .status
        .as_deref()
        .is_some_and(|status| status.eq_ignore_ascii_case("CANCELLED"))
}

fn first_upcoming_materials_event(
    events: &[CalendarEvent],
    now: chrono::DateTime<Utc>,
) -> Option<&CalendarEvent> {
    let latest_start = now
        .checked_add_days(Days::new(LIVE_CALENDAR_MATERIALS_WINDOW_DAYS))
        .unwrap_or(now);
    events
        .iter()
        .filter(|event| !is_cancelled_calendar_event(event))
        .filter(|event| {
            event
                .summary
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        })
        .filter(|event| {
            event
                .start
                .as_deref()
                .and_then(parse_calendar_event_start)
                .is_some_and(|start| start >= now && start <= latest_start)
        })
        .min_by_key(|event| event.start.clone())
}

fn first_recent_follow_up_event(
    events: &[CalendarEvent],
    now: chrono::DateTime<Utc>,
) -> Option<&CalendarEvent> {
    let earliest_end = now
        .checked_sub_days(Days::new(LIVE_CALENDAR_FOLLOW_UP_LOOKBACK_DAYS))
        .unwrap_or(now);
    events
        .iter()
        .filter(|event| !is_cancelled_calendar_event(event))
        .filter(|event| {
            event
                .summary
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        })
        .filter(|event| {
            event
                .end
                .as_deref()
                .and_then(parse_calendar_event_start)
                .is_some_and(|end| end <= now && end >= earliest_end)
        })
        .max_by_key(|event| event.end.clone())
}

fn first_recent_follow_up_message<'a>(
    messages: &'a [MailMessageSummary],
    event: &CalendarEvent,
) -> Option<&'a MailMessageSummary> {
    let summary = normalize_follow_up_match_text(event.summary.as_deref()?);
    if summary.is_empty() {
        return None;
    }
    messages.iter().find(|message| {
        message
            .subject
            .as_deref()
            .map(normalize_follow_up_match_text)
            .is_some_and(|subject| !subject.is_empty() && subject.contains(&summary))
    })
}

fn suggested_calendar_materials_path(event: &CalendarEvent) -> Option<String> {
    let summary = event.summary.as_deref()?.trim();
    if summary.is_empty() {
        return None;
    }
    let date = event
        .start
        .as_deref()
        .map(calendar_event_date_prefix)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "meeting".to_string());
    let name = sanitize_disk_folder_name(summary);
    Some(format!("disk:/{date} {name}"))
}

fn disk_path_exists(items: &[DiskResourceItem], path: &str) -> bool {
    items.iter().any(|item| item.path == path)
}

fn parse_calendar_event_start(value: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .ok()
}

fn calendar_event_date_prefix(value: &str) -> String {
    value.chars().take(10).collect::<String>()
}

fn sanitize_disk_folder_name(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            _ if ch.is_control() => ' ',
            _ => ch,
        })
        .collect::<String>();
    sanitized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    let sanitized = sanitized.trim_matches(['.', ' ']).to_string();
    if sanitized.is_empty() {
        "Встреча".to_string()
    } else {
        sanitized
    }
}

fn normalize_follow_up_match_text(value: &str) -> String {
    let lowercase = value.trim().to_lowercase();
    let normalized = lowercase
        .strip_prefix("re: ")
        .or_else(|| lowercase.strip_prefix("fw: "))
        .or_else(|| lowercase.strip_prefix("fwd: "))
        .unwrap_or(&lowercase);
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
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
        LiveMailMessageContext, build_live_calendar_cancelled_suggestion,
        build_live_calendar_mail_follow_up_suggestion, build_live_calendar_materials_suggestion,
        build_live_calendar_publish_materials_suggestion, build_live_disk_public_link_suggestion,
        build_live_disk_send_link_suggestion, build_live_mail_attachment_suggestion,
        build_live_mail_invite_suggestion, collect_suggestions,
        first_invite_attachment_in_summaries, first_recent_follow_up_event,
        first_recent_follow_up_message, first_regular_attachment_in_summaries,
        is_cancelled_calendar_event, is_public_disk_item, suggested_calendar_materials_path,
        summary_for_suggestions,
    };
    use crate::activity_store::ActivityEntry;
    use crate::calendar::CalendarEvent;
    use crate::disk::DiskResourceItem;
    use crate::mail::{MailAttachmentSummary, MailMessageSummary, smtp_safe_message_bytes};
    use chrono::Utc;

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

    #[test]
    fn build_live_disk_send_link_suggestion_returns_open_tool_handoff() {
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
            public_key: None,
        };
        let suggestion =
            build_live_disk_send_link_suggestion("mock", &item, Some("send-link-by-mail"));
        assert_eq!(suggestion.workflow_id, Some("send-link-by-mail"));
        assert_eq!(suggestion.source, "disk_live");
        assert_eq!(suggestion.kind, "follow_up");
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(
            suggestion.action.tool_name,
            Some("yacli.mail.send_published_link")
        );
        assert!(!suggestion.action.supports_review);
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|v| v["public_url"].as_str()),
            Some("https://disk.yandex.ru/i/report")
        );
        assert!(suggestion.command.contains("send-published-link"));
    }

    #[test]
    fn cancelled_calendar_event_detection_is_case_insensitive() {
        let event = CalendarEvent {
            calendar_id: "team".to_string(),
            calendar_name: "Команда".to_string(),
            href: "/cal/team/event.ics".to_string(),
            uid: Some("uid-123".to_string()),
            summary: Some("Ревью".to_string()),
            start: Some("2026-03-20T10:00:00Z".to_string()),
            end: Some("2026-03-20T10:30:00Z".to_string()),
            description: None,
            location: None,
            status: Some("cancelled".to_string()),
            etag: None,
            all_day: false,
        };
        assert!(is_cancelled_calendar_event(&event));
    }

    #[test]
    fn build_live_calendar_cancelled_suggestion_returns_delete_handoff() {
        let event = CalendarEvent {
            calendar_id: "team".to_string(),
            calendar_name: "Команда".to_string(),
            href: "/cal/team/event.ics".to_string(),
            uid: Some("uid-123".to_string()),
            summary: Some("Ревью".to_string()),
            start: Some("2026-03-20T10:00:00Z".to_string()),
            end: Some("2026-03-20T10:30:00Z".to_string()),
            description: None,
            location: None,
            status: Some("CANCELLED".to_string()),
            etag: None,
            all_day: false,
        };
        let suggestion = build_live_calendar_cancelled_suggestion("mock", "team", &event);
        assert_eq!(suggestion.source, "calendar_live");
        assert_eq!(suggestion.kind, "cleanup");
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(suggestion.action.tool_name, Some("yacli.calendar.delete"));
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|value| value["uid"].as_str()),
            Some("uid-123")
        );
        assert!(suggestion.reason.contains("CANCELLED"));
    }

    #[test]
    fn suggested_calendar_materials_path_uses_date_and_sanitized_summary() {
        let event = CalendarEvent {
            calendar_id: "team".to_string(),
            calendar_name: "Команда".to_string(),
            href: "/cal/team/event.ics".to_string(),
            uid: Some("uid-456".to_string()),
            summary: Some("Ревью / Платформа".to_string()),
            start: Some("2026-03-20T10:00:00Z".to_string()),
            end: Some("2026-03-20T10:30:00Z".to_string()),
            description: None,
            location: None,
            status: Some("CONFIRMED".to_string()),
            etag: None,
            all_day: false,
        };
        assert_eq!(
            suggested_calendar_materials_path(&event).as_deref(),
            Some("disk:/2026-03-20 Ревью - Платформа")
        );
    }

    #[test]
    fn build_live_calendar_materials_suggestion_returns_mkdir_handoff() {
        let event = CalendarEvent {
            calendar_id: "team".to_string(),
            calendar_name: "Команда".to_string(),
            href: "/cal/team/event.ics".to_string(),
            uid: Some("uid-789".to_string()),
            summary: Some("Планирование".to_string()),
            start: Some("2026-03-21T09:00:00Z".to_string()),
            end: Some("2026-03-21T09:30:00Z".to_string()),
            description: None,
            location: None,
            status: Some("CONFIRMED".to_string()),
            etag: None,
            all_day: false,
        };
        let suggestion = build_live_calendar_materials_suggestion(
            "mock",
            &event,
            "disk:/2026-03-21 Планирование",
        );
        assert_eq!(suggestion.source, "calendar_live");
        assert_eq!(suggestion.kind, "follow_up");
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(suggestion.action.tool_name, Some("yacli.disk.mkdir"));
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|value| value["path"].as_str()),
            Some("disk:/2026-03-21 Планирование")
        );
        assert!(suggestion.reason.contains("ближайшее событие"));
    }

    #[test]
    fn first_recent_follow_up_event_prefers_recent_finished_event() {
        let events = vec![
            CalendarEvent {
                calendar_id: "team".to_string(),
                calendar_name: "Команда".to_string(),
                href: "/cal/team/old.ics".to_string(),
                uid: Some("uid-old".to_string()),
                summary: Some("Старое".to_string()),
                start: Some("2026-03-10T08:00:00Z".to_string()),
                end: Some("2026-03-10T08:30:00Z".to_string()),
                description: None,
                location: None,
                status: Some("CONFIRMED".to_string()),
                etag: None,
                all_day: false,
            },
            CalendarEvent {
                calendar_id: "team".to_string(),
                calendar_name: "Команда".to_string(),
                href: "/cal/team/recent.ics".to_string(),
                uid: Some("uid-recent".to_string()),
                summary: Some("Ревью".to_string()),
                start: Some("2026-03-15T08:00:00Z".to_string()),
                end: Some("2026-03-15T08:30:00Z".to_string()),
                description: None,
                location: None,
                status: Some("CONFIRMED".to_string()),
                etag: None,
                all_day: false,
            },
        ];
        let now = chrono::DateTime::parse_from_rfc3339("2026-03-15T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            first_recent_follow_up_event(&events, now).and_then(|event| event.uid.as_deref()),
            Some("uid-recent")
        );
    }

    #[test]
    fn build_live_calendar_publish_materials_suggestion_returns_publish_handoff() {
        let event = CalendarEvent {
            calendar_id: "team".to_string(),
            calendar_name: "Команда".to_string(),
            href: "/cal/team/recent.ics".to_string(),
            uid: Some("uid-recent".to_string()),
            summary: Some("Ревью".to_string()),
            start: Some("2026-03-15T08:00:00Z".to_string()),
            end: Some("2026-03-15T08:30:00Z".to_string()),
            description: None,
            location: None,
            status: Some("CONFIRMED".to_string()),
            etag: None,
            all_day: false,
        };
        let item = DiskResourceItem {
            name: "2026-03-15 Ревью".to_string(),
            path: "disk:/2026-03-15 Ревью".to_string(),
            resource_type: "dir".to_string(),
            mime_type: None,
            size: None,
            created: None,
            modified: None,
            md5: None,
            revision: None,
            public_url: None,
            public_key: None,
        };
        let suggestion = build_live_calendar_publish_materials_suggestion("mock", &event, &item);
        assert_eq!(suggestion.source, "calendar_live");
        assert_eq!(suggestion.kind, "follow_up");
        assert_eq!(suggestion.workflow_id, Some("publish-file-link"));
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(suggestion.action.tool_name, Some("yacli.disk.publish"));
        assert!(suggestion.action.supports_review);
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|value| value["path"].as_str()),
            Some("disk:/2026-03-15 Ревью")
        );
        assert!(suggestion.reason.contains("завершилось"));
    }

    #[test]
    fn first_recent_follow_up_message_matches_re_subject() {
        let event = CalendarEvent {
            calendar_id: "team".to_string(),
            calendar_name: "Команда".to_string(),
            href: "/cal/team/review.ics".to_string(),
            uid: Some("uid-recent".to_string()),
            summary: Some("Ревью платформы".to_string()),
            start: Some("2026-03-15T08:00:00Z".to_string()),
            end: Some("2026-03-15T08:30:00Z".to_string()),
            description: None,
            location: None,
            status: Some("CONFIRMED".to_string()),
            etag: None,
            all_day: false,
        };
        let messages = vec![
            MailMessageSummary {
                uid: 77,
                subject: Some("Re: Ревью платформы".to_string()),
                from: Some("organizer@example.com".to_string()),
                date: None,
                flags: vec![],
                size: None,
            },
            MailMessageSummary {
                uid: 88,
                subject: Some("Другая тема".to_string()),
                from: Some("other@example.com".to_string()),
                date: None,
                flags: vec![],
                size: None,
            },
        ];
        assert_eq!(
            first_recent_follow_up_message(&messages, &event).map(|message| message.uid),
            Some(77)
        );
    }

    #[test]
    fn build_live_calendar_mail_follow_up_suggestion_returns_reply_handoff() {
        let event = CalendarEvent {
            calendar_id: "team".to_string(),
            calendar_name: "Команда".to_string(),
            href: "/cal/team/review.ics".to_string(),
            uid: Some("uid-recent".to_string()),
            summary: Some("Ревью платформы".to_string()),
            start: Some("2026-03-15T08:00:00Z".to_string()),
            end: Some("2026-03-15T08:30:00Z".to_string()),
            description: None,
            location: None,
            status: Some("CONFIRMED".to_string()),
            etag: None,
            all_day: false,
        };
        let message = MailMessageSummary {
            uid: 77,
            subject: Some("Re: Ревью платформы".to_string()),
            from: Some("organizer@example.com".to_string()),
            date: None,
            flags: vec![],
            size: None,
        };
        let suggestion = build_live_calendar_mail_follow_up_suggestion("mock", &event, &message);
        assert_eq!(suggestion.source, "calendar_live");
        assert_eq!(suggestion.kind, "follow_up");
        assert_eq!(suggestion.workflow_id, Some("reply-with-context"));
        assert_eq!(suggestion.action.kind, "open_tool");
        assert_eq!(suggestion.action.tool_name, Some("yacli.mail.reply"));
        assert_eq!(
            suggestion
                .action
                .tool_arguments
                .as_ref()
                .and_then(|value| value["uid"].as_u64()),
            Some(77)
        );
        assert!(suggestion.reason.contains("похожей темой"));
    }
}
