use chrono::{Days, Utc};
use serde_json::{Value, json};

use super::skills;
use crate::account_store::AccountStore;
use crate::error::{Result, YacliError};

struct PromptArgument {
    name: &'static str,
    description: &'static str,
    required: bool,
}

struct PromptDefinition {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    arguments: &'static [PromptArgument],
}

const SHARED_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "goal",
        description: "What you want to achieve with yacli.",
        required: false,
    },
    PromptArgument {
        name: "account",
        description: "Optional yacli account alias to prefer.",
        required: false,
    },
];

const MAIL_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "request",
        description: "What mail task to perform.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional yacli account alias.",
        required: false,
    },
    PromptArgument {
        name: "folder",
        description: "Mailbox folder, defaults to INBOX when omitted.",
        required: false,
    },
];

const CALENDAR_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "request",
        description: "What calendar task to perform.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional yacli account alias.",
        required: false,
    },
    PromptArgument {
        name: "calendar",
        description: "Calendar name, defaults to default.",
        required: false,
    },
    PromptArgument {
        name: "from",
        description: "Optional RFC3339 or YYYY-MM-DD start boundary.",
        required: false,
    },
    PromptArgument {
        name: "to",
        description: "Optional RFC3339 or YYYY-MM-DD end boundary.",
        required: false,
    },
];

const DISK_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "request",
        description: "What disk task to perform.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional yacli account alias.",
        required: false,
    },
    PromptArgument {
        name: "path",
        description: "Optional disk path such as disk:/Documents.",
        required: false,
    },
];

const DAILY_BRIEFING_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "account",
        description: "Optional yacli account alias.",
        required: false,
    },
    PromptArgument {
        name: "from",
        description: "Optional start date for the schedule window.",
        required: false,
    },
    PromptArgument {
        name: "to",
        description: "Optional end date for the schedule window.",
        required: false,
    },
    PromptArgument {
        name: "mail_limit",
        description: "How many recent inbox messages to inspect.",
        required: false,
    },
];

const FIND_AND_READ_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "query",
        description: "Search phrase to locate the email.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional yacli account alias.",
        required: false,
    },
    PromptArgument {
        name: "folder",
        description: "Mailbox folder, defaults to INBOX when omitted.",
        required: false,
    },
    PromptArgument {
        name: "limit",
        description: "How many matching messages to inspect.",
        required: false,
    },
];

const REPLY_WITH_CONTEXT_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "uid",
        description: "The mail UID to read and respond to.",
        required: true,
    },
    PromptArgument {
        name: "account",
        description: "Optional yacli account alias.",
        required: false,
    },
    PromptArgument {
        name: "folder",
        description: "Mailbox folder, defaults to INBOX when omitted.",
        required: false,
    },
    PromptArgument {
        name: "from",
        description: "Optional schedule start boundary to inspect.",
        required: false,
    },
    PromptArgument {
        name: "to",
        description: "Optional schedule end boundary to inspect.",
        required: false,
    },
];

const PROMPTS: &[PromptDefinition] = &[
    PromptDefinition {
        name: "shared",
        title: "yacli Shared Guide",
        description: "Resolve account, auth, and core yacli MCP context before working with a Yandex service.",
        arguments: SHARED_ARGUMENTS,
    },
    PromptDefinition {
        name: "mail",
        title: "yacli Mail Workflow",
        description: "Use yacli MCP mail tools to inspect folders, search mail, and read messages.",
        arguments: MAIL_ARGUMENTS,
    },
    PromptDefinition {
        name: "calendar",
        title: "yacli Calendar Workflow",
        description: "Use yacli MCP calendar tools to inspect calendars and upcoming events.",
        arguments: CALENDAR_ARGUMENTS,
    },
    PromptDefinition {
        name: "disk",
        title: "yacli Disk Workflow",
        description: "Use yacli MCP disk tools to inspect quota and list disk resources.",
        arguments: DISK_ARGUMENTS,
    },
    PromptDefinition {
        name: "daily-briefing",
        title: "yacli Daily Briefing",
        description: "Combine inbox and calendar context into one morning briefing.",
        arguments: DAILY_BRIEFING_ARGUMENTS,
    },
    PromptDefinition {
        name: "find-and-read",
        title: "yacli Find And Read",
        description: "Search for an email and read the most relevant result.",
        arguments: FIND_AND_READ_ARGUMENTS,
    },
    PromptDefinition {
        name: "reply-with-context",
        title: "yacli Reply With Context",
        description: "Read an email, inspect the schedule, and draft a context-aware reply.",
        arguments: REPLY_WITH_CONTEXT_ARGUMENTS,
    },
];

pub fn prompt_names() -> Vec<&'static str> {
    PROMPTS.iter().map(|prompt| prompt.name).collect()
}

pub fn prompt_definitions() -> Vec<Value> {
    PROMPTS
        .iter()
        .map(|prompt| {
            json!({
                "name": prompt.name,
                "title": prompt.title,
                "description": prompt.description,
                "arguments": prompt.arguments.iter().map(argument_json).collect::<Vec<_>>(),
            })
        })
        .collect()
}

pub fn get_prompt(params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| YacliError::Validation("prompts/get requires `name`".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    let prompt = PROMPTS
        .iter()
        .find(|prompt| prompt.name == name)
        .ok_or_else(|| YacliError::Validation(format!("unknown MCP prompt: {name}")))?;

    Ok(json!({
        "description": prompt.description,
        "messages": prompt_messages(name, &arguments)?,
    }))
}

pub fn complete(params: Value) -> Result<Value> {
    let reference = params
        .get("ref")
        .ok_or_else(|| YacliError::Validation("completion/complete requires `ref`".to_string()))?;
    let argument = params.get("argument").ok_or_else(|| {
        YacliError::Validation("completion/complete requires `argument`".to_string())
    })?;
    let argument_name = argument
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            YacliError::Validation("completion/complete requires `argument.name`".to_string())
        })?;
    let argument_value = argument
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let context_arguments = params
        .get("context")
        .and_then(|context| context.get("arguments"));

    let suggestions = match reference.get("type").and_then(Value::as_str) {
        Some("ref/prompt") => complete_prompt_reference(
            reference
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    YacliError::Validation(
                        "prompt completion requires `ref.name` for ref/prompt".to_string(),
                    )
                })?,
            argument_name,
            argument_value,
            context_arguments,
        )?,
        Some("ref/resource") => complete_resource_reference(
            reference
                .get("uri")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    YacliError::Validation(
                        "resource completion requires `ref.uri` for ref/resource".to_string(),
                    )
                })?,
            argument_name,
            argument_value,
        )?,
        Some(other) => {
            return Err(YacliError::Validation(format!(
                "unsupported completion reference type: {other}"
            )));
        }
        None => {
            return Err(YacliError::Validation(
                "completion/complete requires `ref.type`".to_string(),
            ));
        }
    };

    let total = suggestions.len();
    let values = suggestions.into_iter().take(100).collect::<Vec<_>>();
    Ok(json!({
        "completion": {
            "values": values,
            "total": total,
            "hasMore": total > 100
        }
    }))
}

fn argument_json(argument: &PromptArgument) -> Value {
    json!({
        "name": argument.name,
        "description": argument.description,
        "required": argument.required,
    })
}

fn prompt_messages(name: &str, arguments: &Value) -> Result<Vec<Value>> {
    let text = match name {
        "shared" => render_shared_prompt(arguments),
        "mail" => render_mail_prompt(arguments)?,
        "calendar" => render_calendar_prompt(arguments)?,
        "disk" => render_disk_prompt(arguments)?,
        "daily-briefing" => render_daily_briefing_prompt(arguments),
        "find-and-read" => render_find_and_read_prompt(arguments)?,
        "reply-with-context" => render_reply_with_context_prompt(arguments)?,
        _ => {
            return Err(YacliError::Validation(format!(
                "unknown MCP prompt: {name}"
            )));
        }
    };

    Ok(vec![json!({
        "role": "user",
        "content": {
            "type": "text",
            "text": enrich_with_canonical_skill(name, &text),
        }
    })])
}

fn complete_prompt_reference(
    name: &str,
    argument_name: &str,
    current_value: &str,
    context_arguments: Option<&Value>,
) -> Result<Vec<String>> {
    let suggestions = match (name, argument_name) {
        (_, "account") => configured_accounts()?,
        ("mail", "folder") | ("find-and-read", "folder") | ("reply-with-context", "folder") => {
            ["INBOX", "Sent", "Drafts", "Archive", "Trash", "Spam"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        ("calendar", "calendar") => vec!["default".to_string()],
        ("calendar", "from")
        | ("calendar", "to")
        | ("daily-briefing", "from")
        | ("daily-briefing", "to")
        | ("reply-with-context", "from")
        | ("reply-with-context", "to") => date_suggestions(),
        ("daily-briefing", "mail_limit") | ("find-and-read", "limit") => {
            vec!["5", "10", "20", "50"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        ("disk", "path") => disk_path_suggestions(),
        ("reply-with-context", "uid") => context_arguments
            .and_then(|arguments| arguments.get("uid"))
            .and_then(Value::as_str)
            .map(|uid| vec![uid.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    Ok(rank_and_filter(suggestions, current_value))
}

fn complete_resource_reference(
    uri: &str,
    argument_name: &str,
    current_value: &str,
) -> Result<Vec<String>> {
    let suggestions = match argument_name {
        "account"
            if uri == "resource://yacli/account/{account}"
                || uri == "resource://yacli/auth/{account}"
                || uri.starts_with("ui://yacli/dashboard") =>
        {
            configured_accounts()?
        }
        "skill" if uri == "resource://yacli/skill/{skill}" => skills::skill_names()
            .into_iter()
            .map(str::to_string)
            .collect(),
        "section" if uri.starts_with("ui://yacli/dashboard") => ["tools", "resources", "auth"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "resource" if uri.starts_with("ui://yacli/dashboard") => {
            ["account", "auth", "skills", "skill"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        "skill" if uri.starts_with("ui://yacli/dashboard") => skills::skill_names()
            .into_iter()
            .map(str::to_string)
            .collect(),
        "tool" if uri.starts_with("ui://yacli/dashboard") => vec![
            "yacli.app.snapshot".to_string(),
            "yacli.account.list".to_string(),
            "yacli.account.current".to_string(),
            "yacli.auth.status".to_string(),
            "yacli.update.check".to_string(),
        ],
        "prompt" if uri.starts_with("ui://yacli/dashboard") => {
            prompt_names().into_iter().map(str::to_string).collect()
        }
        _ => Vec::new(),
    };

    Ok(rank_and_filter(suggestions, current_value))
}

fn configured_accounts() -> Result<Vec<String>> {
    let store = AccountStore::load()?;
    let mut accounts = store.summaries().into_keys().collect::<Vec<_>>();
    accounts.sort();
    Ok(accounts)
}

fn date_suggestions() -> Vec<String> {
    let today = Utc::now().date_naive();
    let tomorrow = today.checked_add_days(Days::new(1)).unwrap_or(today);
    let next_week = today.checked_add_days(Days::new(7)).unwrap_or(today);
    vec![
        today.format("%Y-%m-%d").to_string(),
        tomorrow.format("%Y-%m-%d").to_string(),
        next_week.format("%Y-%m-%d").to_string(),
    ]
}

fn disk_path_suggestions() -> Vec<String> {
    vec![
        "disk:/".to_string(),
        "disk:/Documents".to_string(),
        "disk:/Downloads".to_string(),
        "disk:/Photos".to_string(),
    ]
}

fn rank_and_filter(candidates: Vec<String>, current_value: &str) -> Vec<String> {
    let needle = current_value.trim().to_ascii_lowercase();
    let mut prefix = Vec::new();
    let mut contains = Vec::new();
    let mut rest = Vec::new();

    for candidate in dedupe_preserve_order(candidates) {
        let haystack = candidate.to_ascii_lowercase();
        if needle.is_empty() {
            rest.push(candidate);
        } else if haystack.starts_with(&needle) {
            prefix.push(candidate);
        } else if haystack.contains(&needle) {
            contains.push(candidate);
        }
    }

    prefix.extend(contains);
    if needle.is_empty() {
        rest.sort();
        rest
    } else {
        prefix
    }
}

fn dedupe_preserve_order(candidates: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn enrich_with_canonical_skill(prompt_name: &str, body: &str) -> String {
    let Some(skill_name) = skills::prompt_skill_name(prompt_name) else {
        return body.to_string();
    };
    format!(
        "{body}\n\nCanonical embedded skill resource: `resource://yacli/skill/{skill_name}`.\nRead it with `resources/read` if you need the exact SKILL.md workflow text mirrored from Claude Code."
    )
}

fn render_shared_prompt(arguments: &Value) -> String {
    let goal = optional_string(arguments, "goal").unwrap_or(
        "Resolve the right yacli account, inspect auth posture, and choose the right service tools before acting.",
    );
    let account = optional_string(arguments, "account");

    format!(
        "You are working with the yacli MCP server.\n\
Goal: {goal}\n\
Preferred account: {}\n\
\n\
Use this workflow:\n\
1. Resolve the account with `yacli.account.current` or `yacli.account.list`.\n\
2. Check auth posture with `yacli.auth.status`.\n\
3. If you need structured account context, read `resource://yacli/account/{{account}}` and `resource://yacli/auth/{{account}}`.\n\
4. Only then move into mail, calendar, or disk tools.\n\
\n\
If the requested account is missing or not authenticated, explain the gap clearly before continuing.",
        account.unwrap_or("current")
    )
}

fn render_mail_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");

    Ok(format!(
        "Help with Yandex Mail through the yacli MCP server.\n\
Request: {request}\n\
Account: {account}\n\
Folder: {folder}\n\
\n\
Use this workflow:\n\
1. Confirm account and auth with `yacli.account.current` / `yacli.auth.status` if needed.\n\
2. Use `yacli.mail.folders` if the correct folder is unclear.\n\
3. Use `yacli.mail.list` for recent context or `yacli.mail.search` when the user gives keywords.\n\
4. Use `yacli.mail.read` for the exact UID that matters.\n\
5. If the task is a write action, use `yacli.mail.send`, `yacli.mail.reply`, or `yacli.mail.forward` with the smallest valid payload.\n\
6. Summarize findings or the send outcome clearly, including sender, date, subject, and the relevant body details."
    ))
}

fn render_calendar_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let calendar = optional_string(arguments, "calendar").unwrap_or("default");
    let from = optional_string(arguments, "from").unwrap_or("auto");
    let to = optional_string(arguments, "to").unwrap_or("auto");

    Ok(format!(
        "Help with Yandex Calendar through the yacli MCP server.\n\
Request: {request}\n\
Account: {account}\n\
Calendar: {calendar}\n\
Window: from={from}, to={to}\n\
\n\
Use this workflow:\n\
1. Confirm account and auth posture if needed.\n\
2. Use `yacli.calendar.calendars` when the target calendar is unclear.\n\
3. Use `yacli.calendar.events` with the narrowest useful time window.\n\
4. If the task is a write action, use `yacli.calendar.create` or `yacli.calendar.delete` with an explicit calendar and exact timestamps or UID.\n\
5. Return a concise schedule summary or the write outcome with times, titles, locations, and conflicts if visible.\n\
\n\
Calendar updates beyond create/delete are still not exposed through MCP. If the user asks to modify an existing event in place, explain that gap clearly."
    ))
}

fn render_disk_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let path = optional_string(arguments, "path").unwrap_or("disk:/");

    Ok(format!(
        "Help with Yandex Disk through the yacli MCP server.\n\
Request: {request}\n\
Account: {account}\n\
Path: {path}\n\
\n\
Use this workflow:\n\
1. Confirm account and auth posture if needed.\n\
2. Use `yacli.disk.info` for quota context.\n\
3. Use `yacli.disk.list` for the requested path.\n\
4. If the task is a write action, use `yacli.disk.mkdir` or `yacli.disk.upload` with an explicit `disk:/...` path and a real local source path for uploads.\n\
5. Summarize the relevant files, directories, storage state, or upload outcome.\n\
\n\
More advanced file mutations beyond mkdir/upload are still not exposed through MCP. If the user asks for move, delete, or rename, explain that gap clearly."
    ))
}

fn render_daily_briefing_prompt(arguments: &Value) -> String {
    let account = optional_string(arguments, "account").unwrap_or("current");
    let from = optional_string(arguments, "from").unwrap_or("today");
    let to = optional_string(arguments, "to").unwrap_or("tomorrow");
    let mail_limit = optional_string(arguments, "mail_limit").unwrap_or("10");

    format!(
        "Prepare a yacli daily briefing.\n\
Account: {account}\n\
Mail limit: {mail_limit}\n\
Calendar window: from={from}, to={to}\n\
\n\
Use this workflow:\n\
1. Resolve account context if needed.\n\
2. Use `yacli.mail.list` on INBOX with the requested limit.\n\
3. Use `yacli.calendar.events` for the requested window.\n\
4. Produce a briefing with: unread or recent message highlights, urgent-looking subjects, upcoming meetings, and obvious conflicts or deadlines.\n\
\n\
Keep the final answer short and operational."
    )
}

fn render_find_and_read_prompt(arguments: &Value) -> Result<String> {
    let query = required_string(arguments, "query")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");
    let limit = optional_string(arguments, "limit").unwrap_or("5");

    Ok(format!(
        "Find and read a specific email through the yacli MCP server.\n\
Query: {query}\n\
Account: {account}\n\
Folder: {folder}\n\
Limit: {limit}\n\
\n\
Use this workflow:\n\
1. Run `yacli.mail.search` with the given query and limit.\n\
2. Pick the most relevant UID based on sender, subject, and date.\n\
3. Run `yacli.mail.read` for that UID.\n\
4. Return the important contents of the message and explicitly mention which UID you selected."
    ))
}

fn render_reply_with_context_prompt(arguments: &Value) -> Result<String> {
    let uid = required_string(arguments, "uid")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");
    let from = optional_string(arguments, "from").unwrap_or("auto");
    let to = optional_string(arguments, "to").unwrap_or("auto");

    Ok(format!(
        "Draft a schedule-aware reply through the yacli MCP server.\n\
Mail UID: {uid}\n\
Account: {account}\n\
Folder: {folder}\n\
Calendar window: from={from}, to={to}\n\
\n\
Use this workflow:\n\
1. Read the original message with `yacli.mail.read`.\n\
2. Infer the relevant scheduling window from the message or use the provided window.\n\
3. Inspect availability with `yacli.calendar.events`.\n\
4. Send the actual reply with `yacli.mail.reply`, or if the user only asked for a draft, provide the draft text explicitly.\n\
\n\
Be explicit whether the final output is a sent reply or only a proposed draft."
    ))
}

fn required_string<'a>(arguments: &'a Value, key: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| YacliError::Validation(format!("prompt argument `{key}` is required")))
}

fn optional_string<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_definitions_expose_all_embedded_prompts() {
        let prompts = prompt_definitions();
        assert_eq!(prompts.len(), 7);
        assert!(prompts.iter().any(|prompt| prompt["name"] == "shared"));
        assert!(prompts.iter().any(|prompt| prompt["name"] == "mail"));
        assert!(prompts.iter().any(|prompt| prompt["name"] == "calendar"));
        assert!(prompts.iter().any(|prompt| prompt["name"] == "disk"));
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt["name"] == "daily-briefing")
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt["name"] == "find-and-read")
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt["name"] == "reply-with-context")
        );
    }

    #[test]
    fn get_prompt_requires_prompt_name() {
        let err = get_prompt(json!({})).expect_err("missing name should fail");
        assert!(err.to_string().contains("requires `name`"));
    }

    #[test]
    fn get_prompt_renders_daily_briefing_prompt() {
        let prompt = get_prompt(json!({
            "name": "daily-briefing",
            "arguments": {
                "account": "work",
                "mail_limit": "15"
            }
        }))
        .expect("prompt");

        let text = prompt["messages"][0]["content"]["text"]
            .as_str()
            .expect("prompt text");
        assert!(text.contains("Account: work"));
        assert!(text.contains("Mail limit: 15"));
        assert!(text.contains("yacli.mail.list"));
        assert!(text.contains("yacli.calendar.events"));
    }

    #[test]
    fn get_prompt_validates_required_arguments() {
        let err = get_prompt(json!({
            "name": "find-and-read",
            "arguments": {}
        }))
        .expect_err("missing query should fail");
        assert!(
            err.to_string()
                .contains("prompt argument `query` is required")
        );
    }

    #[test]
    fn reply_with_context_prompt_mentions_draft_boundary() {
        let prompt = get_prompt(json!({
            "name": "reply-with-context",
            "arguments": {
                "uid": "42"
            }
        }))
        .expect("prompt");

        let text = prompt["messages"][0]["content"]["text"]
            .as_str()
            .expect("prompt text");
        assert!(text.contains("Send the actual reply with `yacli.mail.reply`"));
        assert!(text.contains("sent reply or only a proposed draft"));
    }

    #[test]
    fn completion_for_mail_folder_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/prompt",
                "name": "mail"
            },
            "argument": {
                "name": "folder",
                "value": "in"
            }
        }))
        .expect("completion");

        assert_eq!(result["completion"]["values"][0], "INBOX");
        assert_eq!(result["completion"]["hasMore"], false);
    }

    #[test]
    fn completion_for_dashboard_resource_section_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/resource",
                "uri": "ui://yacli/dashboard{?account,section,resource,tool}"
            },
            "argument": {
                "name": "section",
                "value": "re"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "resources");
    }

    #[test]
    fn completion_for_skill_resource_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/resource",
                "uri": "resource://yacli/skill/{skill}"
            },
            "argument": {
                "name": "skill",
                "value": "yacli-ma"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "yacli-mail");
    }

    #[test]
    fn completion_for_dashboard_skill_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/resource",
                "uri": "ui://yacli/dashboard{?account,section,resource,tool,skill}"
            },
            "argument": {
                "name": "skill",
                "value": "yacli-ca"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "yacli-calendar");
    }

    #[test]
    fn prompt_messages_include_canonical_skill_resource() {
        let prompt = get_prompt(json!({
            "name": "mail",
            "arguments": {
                "request": "reply to the latest invoice"
            }
        }))
        .expect("prompt");
        let text = prompt["messages"][0]["content"]["text"]
            .as_str()
            .expect("prompt text");
        assert!(text.contains("resource://yacli/skill/yacli-mail"));
    }
}
