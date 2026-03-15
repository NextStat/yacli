use serde::{Deserialize, Serialize};

use crate::activity_store::ActivityEntry;
use crate::calendar::{CalendarCollection, CalendarEvent, delete_calendar_event};
use crate::disk::{
    DiskResource, PrivateDiskUnpublishRequest, UnpublishedDiskResource, unpublish_private_resource,
};
use crate::error::{Result, YacliError};
use crate::runtime_context::{resolve_calendar_private_context, resolve_disk_private_context};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityUndoAction {
    CalendarDelete { calendar: String, uid: String },
    DiskUnpublish { path: String },
}

impl ActivityUndoAction {
    pub fn command_line(&self) -> String {
        match self {
            Self::CalendarDelete { calendar, uid } => {
                if calendar == "default" {
                    format!("yacli calendar delete {}", shell_quote(uid))
                } else {
                    format!(
                        "yacli calendar delete --calendar {} {}",
                        shell_quote(calendar),
                        shell_quote(uid)
                    )
                }
            }
            Self::DiskUnpublish { path } => {
                format!("yacli disk unpublish {}", shell_quote(path))
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityUndoResult {
    CalendarDelete {
        calendar: CalendarCollection,
        deleted_event: CalendarEvent,
    },
    DiskUnpublish {
        result: UnpublishedDiskResource,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct ActivityUndoApplied {
    pub original_activity_id: String,
    pub original_operation: String,
    pub account: String,
    pub summary: String,
    pub replay_command: String,
    pub result: ActivityUndoResult,
}

pub fn calendar_create_undo(
    calendar: &CalendarCollection,
    event: &CalendarEvent,
) -> Option<ActivityUndoAction> {
    Some(ActivityUndoAction::CalendarDelete {
        calendar: calendar.id.clone(),
        uid: event.uid.clone()?,
    })
}

pub fn disk_publish_undo(resource: &DiskResource) -> ActivityUndoAction {
    ActivityUndoAction::DiskUnpublish {
        path: resource.path.clone(),
    }
}

pub fn apply_activity_undo(entry: &ActivityEntry) -> Result<ActivityUndoApplied> {
    let undo = entry.undo.clone().ok_or_else(|| {
        YacliError::UnsupportedOperation(format!(
            "activity undo: запись `{}` не поддерживает откат",
            entry.id
        ))
    })?;

    let replay_command = undo.command_line();

    match undo {
        ActivityUndoAction::CalendarDelete { calendar, uid } => {
            let (_, app_password, context) =
                resolve_calendar_private_context(Some(&entry.account))?;
            let (calendar, deleted_event) = delete_calendar_event(
                &context.caldav_base_url,
                &context.email,
                &app_password,
                &calendar,
                &uid,
            )?;
            Ok(ActivityUndoApplied {
                original_activity_id: entry.id.clone(),
                original_operation: entry.operation.clone(),
                account: entry.account.clone(),
                summary: format!(
                    "Откат действия {}: удалено событие из календаря {}: {}",
                    entry.id,
                    calendar.name,
                    deleted_event.summary.as_deref().unwrap_or("-")
                ),
                replay_command,
                result: ActivityUndoResult::CalendarDelete {
                    calendar,
                    deleted_event,
                },
            })
        }
        ActivityUndoAction::DiskUnpublish { path } => {
            let (_, base_url, access_token) = resolve_disk_private_context(Some(&entry.account))?;
            let result = unpublish_private_resource(
                &base_url,
                &access_token,
                &PrivateDiskUnpublishRequest { path },
            )?;
            Ok(ActivityUndoApplied {
                original_activity_id: entry.id.clone(),
                original_operation: entry.operation.clone(),
                account: entry.account.clone(),
                summary: format!(
                    "Откат действия {}: отозвана публичная ссылка {}",
                    entry.id,
                    result
                        .revoked_public_url
                        .as_deref()
                        .unwrap_or(&result.resource.path)
                ),
                replay_command,
                result: ActivityUndoResult::DiskUnpublish { result },
            })
        }
    }
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
