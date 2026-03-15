use serde::Serialize;
use serde_json::{Value, json};

use crate::calendar::{CalendarCreateRequest, CalendarInvite, calendar_create_command};
use crate::error::YacliError;
use crate::mail::{InspectedMailInvite, MailAttachmentSelector};

#[derive(Clone, Debug, Serialize)]
pub struct MailInviteCreateEventPartialFailure {
    pub attachment: InspectedMailInvite,
    pub selected_invite: CalendarInvite,
    pub create_request: CalendarCreateRequest,
    pub failed_stage: String,
    pub error: MailInviteCreateEventPartialError,
    pub recovery: MailInviteCreateEventRecovery,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailInviteCreateEventPartialError {
    pub code: &'static str,
    pub message: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailInviteCreateEventRecovery {
    pub inspect_invite: InspectInviteRecovery,
    pub retry_calendar_step: RetryCalendarStepRecovery,
}

#[derive(Clone, Debug, Serialize)]
pub struct InspectInviteRecovery {
    pub tool: String,
    pub command: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct RetryCalendarStepRecovery {
    pub tool: String,
    pub command: String,
    pub arguments: Value,
}

pub fn invite_create_event_command(
    uid: u64,
    folder: &str,
    selector: &MailAttachmentSelector,
    calendar: &str,
    event_index: usize,
) -> String {
    let mut command = format!(
        "yacli mail invite create-event {} --folder {} --calendar {} --event-index {}",
        uid,
        shell_quote(folder),
        shell_quote(calendar),
        event_index
    );
    match selector {
        MailAttachmentSelector::Index(index) => {
            command.push_str(&format!(" --index {}", index));
        }
        MailAttachmentSelector::Filename(name) => {
            command.push_str(&format!(" --name {}", shell_quote(name)));
        }
    }
    command
}

pub fn partial_failure(
    attachment: &InspectedMailInvite,
    selected_invite: &CalendarInvite,
    create_request: &CalendarCreateRequest,
    folder: &str,
    selector: &MailAttachmentSelector,
    max_bytes: u64,
    err: YacliError,
) -> MailInviteCreateEventPartialFailure {
    MailInviteCreateEventPartialFailure {
        attachment: attachment.clone(),
        selected_invite: selected_invite.clone(),
        create_request: create_request.clone(),
        failed_stage: "calendar_create".to_string(),
        error: MailInviteCreateEventPartialError {
            code: err.code(),
            message: err.to_string(),
            exit_code: err.exit_code(),
        },
        recovery: MailInviteCreateEventRecovery {
            inspect_invite: InspectInviteRecovery {
                tool: "yacli.mail.invite.inspect".to_string(),
                command: inspect_invite_command(
                    attachment.message_uid,
                    folder,
                    selector,
                    max_bytes,
                ),
                arguments: inspect_invite_arguments(
                    attachment.message_uid,
                    folder,
                    selector,
                    max_bytes,
                ),
            },
            retry_calendar_step: RetryCalendarStepRecovery {
                tool: "yacli.calendar.create".to_string(),
                command: calendar_create_command(create_request, true),
                arguments: json!({
                    "calendar": create_request.calendar,
                    "summary": create_request.summary,
                    "start": create_request.start,
                    "end": create_request.end,
                    "description": create_request.description,
                    "location": create_request.location,
                    "dry_run": true,
                }),
            },
        },
    }
}

pub fn inspect_invite_command(
    uid: u64,
    folder: &str,
    selector: &MailAttachmentSelector,
    max_bytes: u64,
) -> String {
    let mut command = format!(
        "yacli mail invite inspect {} --folder {}",
        uid,
        shell_quote(folder)
    );
    match selector {
        MailAttachmentSelector::Index(index) => {
            command.push_str(&format!(" --index {}", index));
        }
        MailAttachmentSelector::Filename(name) => {
            command.push_str(&format!(" --name {}", shell_quote(name)));
        }
    }
    if max_bytes != 2 * 1024 * 1024 {
        command.push_str(&format!(" --max-bytes {}", max_bytes));
    }
    command
}

pub fn inspect_invite_arguments(
    uid: u64,
    folder: &str,
    selector: &MailAttachmentSelector,
    max_bytes: u64,
) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("uid".to_string(), json!(uid));
    object.insert("folder".to_string(), json!(folder));
    match selector {
        MailAttachmentSelector::Index(index) => {
            object.insert("index".to_string(), json!(index));
        }
        MailAttachmentSelector::Filename(name) => {
            object.insert("name".to_string(), json!(name));
        }
    }
    object.insert("max_bytes".to_string(), json!(max_bytes));
    Value::Object(object)
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '.' | ':' | '@'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{inspect_invite_arguments, invite_create_event_command, partial_failure};
    use crate::calendar::{CalendarCreateRequest, CalendarInvite};
    use crate::error::YacliError;
    use crate::mail::{InspectedMailInvite, MailAttachmentSelector};

    #[test]
    fn partial_failure_preserves_retry_and_inspect_recovery() {
        let attachment = InspectedMailInvite {
            message_uid: 1353,
            attachment_index: 2,
            filename: Some("invite.ics".to_string()),
            mime_type: "text/calendar".to_string(),
            content_id: None,
            inline: false,
            invites: vec![CalendarInvite {
                uid: Some("evt-1".to_string()),
                summary: Some("Ревью".to_string()),
                start: Some("2026-03-16T10:00:00Z".to_string()),
                end: Some("2026-03-16T10:30:00Z".to_string()),
                description: Some("Описание".to_string()),
                location: Some("Переговорка".to_string()),
                status: None,
                all_day: false,
            }],
        };
        let selected_invite = attachment.invites[0].clone();
        let create_request = CalendarCreateRequest {
            calendar: "team".to_string(),
            summary: "Ревью".to_string(),
            start: "2026-03-16T10:00:00Z".to_string(),
            end: "2026-03-16T10:30:00Z".to_string(),
            description: Some("Описание".to_string()),
            location: Some("Переговорка".to_string()),
        };

        let partial = partial_failure(
            &attachment,
            &selected_invite,
            &create_request,
            "Inbox",
            &MailAttachmentSelector::Filename("invite.ics".to_string()),
            2 * 1024 * 1024,
            YacliError::Network("calendar offline".to_string()),
        );

        assert_eq!(partial.failed_stage, "calendar_create");
        assert_eq!(partial.error.code, "NETWORK_ERROR");
        assert_eq!(
            partial.recovery.inspect_invite.tool,
            "yacli.mail.invite.inspect"
        );
        assert_eq!(
            partial.recovery.retry_calendar_step.tool,
            "yacli.calendar.create"
        );
        assert!(
            partial
                .recovery
                .retry_calendar_step
                .command
                .contains("calendar create")
        );
        assert!(
            partial
                .recovery
                .retry_calendar_step
                .command
                .contains("--dry-run")
        );
    }

    #[test]
    fn inspect_invite_arguments_match_selector_shape() {
        let arguments =
            inspect_invite_arguments(42, "Inbox", &MailAttachmentSelector::Index(1), 1234);
        assert_eq!(
            arguments,
            json!({
                "uid": 42,
                "folder": "Inbox",
                "index": 1,
                "max_bytes": 1234
            })
        );
    }

    #[test]
    fn invite_create_event_command_preserves_selector() {
        let command = invite_create_event_command(
            42,
            "Inbox",
            &MailAttachmentSelector::Filename("invite.ics".to_string()),
            "team",
            2,
        );
        assert!(command.contains("mail invite create-event 42"));
        assert!(command.contains("--name invite.ics"));
        assert!(command.contains("--calendar team"));
        assert!(command.contains("--event-index 2"));
    }
}
