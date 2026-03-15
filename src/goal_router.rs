use serde_json::{Map, Value, json};

use crate::doctor::doctor_payload;
use crate::error::Result;
use crate::onboarding::onboarding_resource_payload;
use crate::workflows::{self, WorkflowDefinition};

struct WorkflowAliasSet {
    id: &'static str,
    aliases: &'static [&'static str],
}

const WORKFLOW_ALIASES: &[WorkflowAliasSet] = &[
    WorkflowAliasSet {
        id: "daily-briefing",
        aliases: &[
            "сводк",
            "утрен",
            "бриф",
            "brief",
            "inbox",
            "дайджест",
            "письм",
            "встреч",
        ],
    },
    WorkflowAliasSet {
        id: "reply-with-context",
        aliases: &[
            "ответ",
            "reply",
            "расписан",
            "календар",
            "окно",
            "подтверд",
            "confirm",
        ],
    },
    WorkflowAliasSet {
        id: "attachment-to-disk",
        aliases: &[
            "вложен",
            "attachment",
            "скачай",
            "сохрани",
            "выгрузи",
            "export",
            "pdf",
            "invoice",
        ],
    },
    WorkflowAliasSet {
        id: "send-file-by-mail",
        aliases: &[
            "отправ",
            "прикреп",
            "attach",
            "attachment",
            "влож",
            "file",
            "файл",
            "почт",
        ],
    },
    WorkflowAliasSet {
        id: "send-link-by-mail",
        aliases: &[
            "почт",
            "письм",
            "email",
            "mail",
            "отправ",
            "перешл",
            "больш",
            "ссылк",
            "url",
            "link",
        ],
    },
    WorkflowAliasSet {
        id: "publish-file-link",
        aliases: &[
            "загру",
            "upload",
            "дай",
            "получ",
            "опублик",
            "publish",
            "публич",
            "link",
            "share",
            "url",
        ],
    },
    WorkflowAliasSet {
        id: "revoke-public-link",
        aliases: &[
            "отзов",
            "убер",
            "сним",
            "скрой",
            "закр",
            "revoke",
            "unpublish",
            "disable",
            "диск",
            "файл",
            "public",
            "ссылк",
        ],
    },
    WorkflowAliasSet {
        id: "invite-to-calendar",
        aliases: &[
            "приглаш",
            "invite",
            "ics",
            "встреч",
            "событ",
            "календар",
            "добавь",
        ],
    },
];
const STRONG_MATCH_THRESHOLD: usize = 5;

#[derive(Clone, Debug)]
struct RankedWorkflow {
    definition: &'static WorkflowDefinition,
    score: usize,
    matched_terms: Vec<&'static str>,
}

#[derive(Clone, Debug)]
struct GoalHints {
    email: String,
    disk_path: String,
    local_path: String,
    quoted_text: String,
    calendar: String,
}

pub fn goal_route_payload(query: &str, account: Option<&str>) -> Result<Value> {
    let normalized_query = normalize_query(query);
    let hints = parse_goal_hints(query);
    let hints_payload = goal_hints_payload(&hints);
    let onboarding = onboarding_resource_payload(None)?;
    let doctor = doctor_payload(account, None)?;
    let mut recommendations = workflows::workflow_ids()
        .into_iter()
        .filter_map(workflows::workflow_definition)
        .map(|definition| rank_workflow(definition, &normalized_query))
        .filter(|ranked| ranked.score >= STRONG_MATCH_THRESHOLD)
        .collect::<Vec<_>>();

    recommendations.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.definition.id.cmp(right.definition.id))
    });

    let best_match = recommendations
        .first()
        .map(|ranked| recommendation_json(ranked, &hints, account));
    let recommendations = if recommendations.is_empty() {
        workflows::workflow_catalog()
            .into_iter()
            .take(3)
            .map(|workflow| {
                let id = workflow["id"].as_str().unwrap_or_default();
                json!({
                    "score": 0,
                    "matched_terms": [],
                    "workflow": workflow,
                    "route": route_json(id, None, None, &hints, account),
                    "reason": "Сильного совпадения не найдено. Откройте canonical workflow hub и выберите ближайший сценарий."
                })
            })
            .collect::<Vec<_>>()
    } else {
        recommendations
            .iter()
            .take(3)
            .map(|ranked| recommendation_json(ranked, &hints, account))
            .collect::<Vec<_>>()
    };

    let status = if best_match.is_some() {
        "matched"
    } else {
        "fallback"
    };
    let suggested_command = best_match
        .as_ref()
        .and_then(|item| item["route"]["workflow_command"].as_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| "yacli workflow list".to_string());
    let remediation = remediation_payload(best_match.as_ref(), &onboarding, &doctor);

    Ok(json!({
        "status": status,
        "query": query,
        "normalized_query": normalized_query,
        "account": account,
        "hints": hints_payload,
        "summary": if let Some(item) = best_match.as_ref() {
            match remediation["status"].as_str().unwrap_or("needs_setup") {
                "ready" => format!(
                    "Лучшая маршрутизация цели ведёт в workflow `{}` и продукт уже готов к выполнению.",
                    item["workflow"]["id"].as_str().unwrap_or_default()
                ),
                _ => format!(
                    "Лучшая маршрутизация цели ведёт в workflow `{}`, но сначала нужно добить readiness.",
                    item["workflow"]["id"].as_str().unwrap_or_default()
                ),
            }
        } else {
            "Маршрутизатор не нашёл сильного совпадения и рекомендует открыть workflow hub.".to_string()
        },
        "best_match": best_match.unwrap_or(Value::Null),
        "suggested_command": suggested_command,
        "recommendations": recommendations,
        "remediation": remediation,
    }))
}

fn rank_workflow(definition: &'static WorkflowDefinition, query: &str) -> RankedWorkflow {
    let mut score = 0;
    let mut matched_terms = Vec::new();

    for alias in aliases_for(definition.id) {
        if query.contains(alias) {
            score += 5;
            matched_terms.push(*alias);
        }
    }

    for signal in [
        definition.title,
        definition.summary,
        definition.connects,
        definition.request_example,
    ] {
        let normalized_signal = normalize_query(signal);
        for token in normalized_signal.split_whitespace() {
            if token.len() >= 4 && query.contains(token) {
                score += 2;
            }
        }
    }

    RankedWorkflow {
        definition,
        score,
        matched_terms,
    }
}

fn goal_hints_payload(hints: &GoalHints) -> Value {
    json!({
        "email": hints.email,
        "disk_path": hints.disk_path,
        "local_path": hints.local_path,
        "quoted_text": hints.quoted_text,
        "calendar": hints.calendar,
    })
}

fn parse_goal_hints(query: &str) -> GoalHints {
    GoalHints {
        email: extract_email_hint(query),
        disk_path: extract_prefixed_token(query, &["disk:"]),
        local_path: extract_local_path_hint(query),
        quoted_text: extract_quoted_hint(query),
        calendar: extract_calendar_hint(query),
    }
}

fn extract_email_hint(query: &str) -> String {
    query
        .split_whitespace()
        .map(strip_goal_token)
        .find(|token| {
            let Some((local, domain)) = token.split_once('@') else {
                return false;
            };
            !local.is_empty() && domain.contains('.')
        })
        .unwrap_or_default()
        .to_string()
}

fn extract_prefixed_token(query: &str, prefixes: &[&str]) -> String {
    query
        .split_whitespace()
        .map(strip_goal_token)
        .find(|token| prefixes.iter().any(|prefix| token.starts_with(prefix)))
        .unwrap_or_default()
        .to_string()
}

fn extract_local_path_hint(query: &str) -> String {
    query
        .split_whitespace()
        .map(strip_goal_token)
        .find(|token| {
            (token.starts_with("./")
                || token.starts_with("../")
                || token.starts_with("~/")
                || (token.starts_with('/') && !token.starts_with("disk:/")))
                && token.len() > 1
        })
        .unwrap_or_default()
        .to_string()
}

fn extract_quoted_hint(query: &str) -> String {
    for (open, close) in [('"', '"'), ('«', '»'), ('“', '”')] {
        if let Some(start) = query.find(open)
            && let Some(rest) = query.get(start + open.len_utf8()..)
            && let Some(end) = rest.find(close)
        {
            return rest[..end].trim().to_string();
        }
    }
    String::new()
}

fn extract_calendar_hint(query: &str) -> String {
    let tokens = query
        .split_whitespace()
        .map(strip_goal_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    for (index, token) in tokens.iter().enumerate() {
        let normalized = normalize_query(token);
        if !(normalized.starts_with("календар") || normalized == "calendar") {
            continue;
        }
        if let Some(next) = tokens.get(index + 1)
            && is_calendar_candidate(next)
        {
            return (*next).to_string();
        }
        if index > 0 {
            let previous = tokens[index - 1];
            if is_calendar_candidate(previous) {
                return previous.to_string();
            }
        }
    }

    String::new()
}

fn is_calendar_candidate(token: &str) -> bool {
    let normalized = normalize_query(token);
    !(normalized.is_empty()
        || normalized.starts_with("календар")
        || matches!(
            normalized.as_str(),
            "calendar" | "в" | "на" | "мой" | "моем" | "мою" | "мои"
        ))
}

fn strip_goal_token(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '"' | '\''
                    | '`'
                    | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '«'
                    | '»'
                    | '“'
                    | '”'
            )
    })
}

fn aliases_for(id: &str) -> &'static [&'static str] {
    WORKFLOW_ALIASES
        .iter()
        .find(|item| item.id == id)
        .map(|item| item.aliases)
        .unwrap_or(&[])
}

fn recommendation_json(ranked: &RankedWorkflow, hints: &GoalHints, account: Option<&str>) -> Value {
    let workflow = workflows::workflow_json(ranked.definition);
    json!({
        "score": ranked.score,
        "matched_terms": ranked.matched_terms,
        "workflow": workflow,
        "route": route_json(
            ranked.definition.id,
            Some(ranked.definition.prompt_name),
            workflow["mcp_tools"].as_array(),
            hints,
            account,
        ),
        "reason": format!(
            "Запрос лучше всего совпадает с workflow `{}` и сигналами `{}`.",
            ranked.definition.id,
            ranked.matched_terms.join(", ")
        )
    })
}

fn route_json(
    workflow_id: &str,
    prompt_name: Option<&str>,
    mcp_tools: Option<&Vec<Value>>,
    hints: &GoalHints,
    account: Option<&str>,
) -> Value {
    let best_tool = workflows::workflow_primary_tool(workflow_id)
        .or_else(|| {
            mcp_tools
                .and_then(|items| items.first())
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    let tool_arguments = route_tool_arguments(workflow_id, best_tool, hints, account);

    json!({
        "workflow_command": format!("yacli workflow show {workflow_id}"),
        "workflow_resource": format!("resource://yacli/workflow/{workflow_id}"),
        "dashboard_view": format!("ui://yacli/dashboard?section=workflows&workflow={workflow_id}"),
        "prompt_name": prompt_name.unwrap_or_default(),
        "best_tool": best_tool,
        "tool_arguments": tool_arguments,
    })
}

fn route_tool_arguments(
    workflow_id: &str,
    best_tool: &str,
    hints: &GoalHints,
    account: Option<&str>,
) -> Value {
    let local_filename = path_basename(&hints.local_path);
    let disk_filename = path_basename(&hints.disk_path);
    let inferred_filename = if !local_filename.is_empty() {
        local_filename.clone()
    } else if !disk_filename.is_empty() {
        disk_filename.clone()
    } else {
        "archive.zip".to_string()
    };
    let mut args = workflows::workflow_primary_tool_arguments(workflow_id)
        .as_object()
        .cloned()
        .unwrap_or_default();

    if let Some(account) = account
        && !account.is_empty()
    {
        args.insert("account".to_string(), Value::String(account.to_string()));
    }

    match (workflow_id, best_tool) {
        ("daily-briefing", "yacli.mail.list") => {}
        ("reply-with-context", "yacli.mail.reply") => {
            args.insert(
                "text".to_string(),
                Value::String(default_if_empty(
                    &hints.quoted_text,
                    "Подтверждаю, это окно подходит",
                )),
            );
        }
        ("attachment-to-disk", "yacli.mail.attachment.export") => {
            let attachment_name =
                if !hints.quoted_text.is_empty() && hints.quoted_text.contains('.') {
                    hints.quoted_text.clone()
                } else if !disk_filename.is_empty() {
                    disk_filename
                } else if !local_filename.is_empty() {
                    local_filename
                } else {
                    "invoice.pdf".to_string()
                };
            args.insert("name".to_string(), Value::String(attachment_name.clone()));
            args.insert(
                "output_path".to_string(),
                Value::String(format!("./{}", path_basename(&attachment_name))),
            );
        }
        ("send-file-by-mail", "yacli.mail.send") => {
            args.insert(
                "to".to_string(),
                Value::String(default_if_empty(&hints.email, "person@example.com")),
            );
            args.insert(
                "subject".to_string(),
                Value::String(default_if_empty(&hints.quoted_text, "Счёт")),
            );
            args.insert(
                "text".to_string(),
                Value::String("Во вложении файл".to_string()),
            );
            args.insert(
                "attachments".to_string(),
                Value::Array(vec![Value::String(default_if_empty(
                    &hints.local_path,
                    "./invoice.pdf",
                ))]),
            );
        }
        ("send-link-by-mail", "yacli.mail.send_link") => {
            let source_path =
                default_if_empty(&hints.local_path, &format!("./{inferred_filename}"));
            let disk_path = if !hints.disk_path.is_empty() {
                hints.disk_path.clone()
            } else {
                inferred_disk_path(&source_path, &inferred_filename)
            };
            args.insert(
                "to".to_string(),
                Value::String(default_if_empty(&hints.email, "person@example.com")),
            );
            args.insert(
                "subject".to_string(),
                Value::String(default_if_empty(&hints.quoted_text, "Материалы")),
            );
            args.insert("source_path".to_string(), Value::String(source_path));
            args.insert("disk_path".to_string(), Value::String(disk_path));
        }
        ("publish-file-link", "yacli.disk.upload_link") => {
            let source = default_if_empty(&hints.local_path, &format!("./{inferred_filename}"));
            let path = if !hints.disk_path.is_empty() {
                hints.disk_path.clone()
            } else {
                inferred_disk_path(&source, &inferred_filename)
            };
            args.insert("source".to_string(), Value::String(source));
            args.insert("path".to_string(), Value::String(path));
        }
        ("revoke-public-link", "yacli.disk.unpublish") => {
            args.insert(
                "path".to_string(),
                Value::String(default_if_empty(
                    &hints.disk_path,
                    "disk:/docs/archive/archive.zip",
                )),
            );
        }
        ("invite-to-calendar", "yacli.mail.invite.create_event") => {
            args.insert(
                "calendar".to_string(),
                Value::String(default_if_empty(&hints.calendar, "team")),
            );
        }
        _ => {}
    }

    Value::Object(args)
}

fn path_basename(path: &str) -> String {
    let normalized = path.trim().trim_start_matches("disk:");
    normalized
        .split(['/', '\\'])
        .rfind(|part| !part.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn inferred_disk_path(local_path: &str, fallback_name: &str) -> String {
    let basename = path_basename(local_path);
    let name = if basename.is_empty() {
        fallback_name.to_string()
    } else {
        basename
    };
    format!("disk:/uploads/{name}")
}

fn default_if_empty(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn remediation_payload(best_match: Option<&Value>, onboarding: &Value, doctor: &Value) -> Value {
    let current_account = doctor["current_account"].clone();
    let mcp_installed = doctor["mcp_clients"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|item| item["status"] == "installed");

    let Some(best_match) = best_match else {
        return json!({
            "status": "fallback",
            "current_account": current_account,
            "required_services": [],
            "service_states": {},
            "actions": next_fallback_actions(onboarding, doctor),
            "mcp_installed": mcp_installed,
        });
    };

    let workflow_id = best_match["workflow"]["id"].as_str().unwrap_or_default();
    let required_services = required_services_for_workflow(workflow_id);
    let service_states = required_services
        .iter()
        .map(|service| {
            (
                (*service).to_string(),
                Value::String(
                    doctor["services"][*service]["credential_state"]
                        .as_str()
                        .unwrap_or("not_configured")
                        .to_string(),
                ),
            )
        })
        .collect::<Map<String, Value>>();

    let mut actions = Vec::new();
    if doctor["current_account"].is_null()
        && let Some(action) = find_check_action("account", onboarding, doctor)
    {
        actions.push(action);
    }

    for service in &required_services {
        let state = doctor["services"][*service]["credential_state"]
            .as_str()
            .unwrap_or("not_configured");
        if !matches!(state, "store_present" | "env_present")
            && let Some(action) = find_check_action(service, onboarding, doctor)
            && !actions
                .iter()
                .any(|item: &Value| item["id"] == action["id"])
        {
            actions.push(action);
        }
    }

    json!({
        "status": if actions.is_empty() { "ready" } else { "needs_setup" },
        "current_account": current_account,
        "required_services": required_services,
        "service_states": service_states,
        "actions": actions,
        "mcp_installed": mcp_installed,
    })
}

fn next_fallback_actions(onboarding: &Value, doctor: &Value) -> Vec<Value> {
    ["account", "mail", "calendar", "disk", "workflow_hub"]
        .into_iter()
        .filter_map(|id| find_check_action(id, onboarding, doctor))
        .take(3)
        .collect()
}

fn find_check_action(id: &str, onboarding: &Value, doctor: &Value) -> Option<Value> {
    for payload in [onboarding, doctor] {
        let checks = payload["checks"].as_array()?;
        for check in checks {
            if check["id"] != id {
                continue;
            }
            let command = check["recommended_command"].as_str()?;
            return Some(json!({
                "id": check["id"].clone(),
                "title": check["title"].clone(),
                "status": check["status"].clone(),
                "reason": check["detail"].clone(),
                "command": command,
            }));
        }
    }
    None
}

fn required_services_for_workflow(workflow_id: &str) -> Vec<&'static str> {
    match workflow_id {
        "daily-briefing" => vec!["mail", "calendar"],
        "reply-with-context" => vec!["mail", "calendar"],
        "attachment-to-disk" => vec!["mail"],
        "send-file-by-mail" => vec!["mail"],
        "send-link-by-mail" => vec!["mail", "disk"],
        "publish-file-link" => vec!["disk"],
        "revoke-public-link" => vec!["disk"],
        "invite-to-calendar" => vec!["mail", "calendar"],
        _ => Vec::new(),
    }
}

fn normalize_query(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'а'..='я' | '0'..='9' => ch,
            'ё' => 'е',
            _ => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_goal_matches_russian_invite_flow() {
        let payload = goal_route_payload(
            "найди приглашение в письме и добавь встречу в календарь",
            None,
        )
        .expect("goal route");

        assert_eq!(payload["status"], "matched");
        assert_eq!(payload["account"], Value::Null);
        assert_eq!(
            payload["best_match"]["workflow"]["id"],
            "invite-to-calendar"
        );
        assert_eq!(
            payload["best_match"]["route"]["best_tool"],
            "yacli.mail.invite.create_event"
        );
        assert_eq!(
            payload["best_match"]["route"]["tool_arguments"]["calendar"],
            "team"
        );
    }

    #[test]
    fn route_goal_matches_russian_revoke_link_flow() {
        let payload = goal_route_payload("отзови публичную ссылку у файла на диске", None)
            .expect("goal route");

        assert_eq!(payload["status"], "matched");
        assert_eq!(
            payload["best_match"]["workflow"]["id"],
            "revoke-public-link"
        );
    }

    #[test]
    fn route_goal_matches_russian_publish_link_flow() {
        let payload = goal_route_payload("загрузи файл на диск и дай публичную ссылку", None)
            .expect("goal route");

        assert_eq!(payload["status"], "matched");
        assert_eq!(payload["best_match"]["workflow"]["id"], "publish-file-link");
    }

    #[test]
    fn route_goal_extracts_canonical_hints_for_autofill() {
        let payload = goal_route_payload(
            "отправь \"Материалы ревью\" на andrei@nextstat.io файл ./review.zip через disk:/docs/review.zip в календарь team",
            None,
        )
        .expect("goal route");

        assert_eq!(payload["hints"]["email"], "andrei@nextstat.io");
        assert_eq!(payload["hints"]["local_path"], "./review.zip");
        assert_eq!(payload["hints"]["disk_path"], "disk:/docs/review.zip");
        assert_eq!(payload["hints"]["quoted_text"], "Материалы ревью");
        assert_eq!(payload["hints"]["calendar"], "team");
        assert_eq!(payload["best_match"]["workflow"]["id"], "send-link-by-mail");
        assert_eq!(
            payload["best_match"]["route"]["tool_arguments"]["to"],
            "andrei@nextstat.io"
        );
        assert_eq!(
            payload["best_match"]["route"]["tool_arguments"]["source_path"],
            "./review.zip"
        );
        assert_eq!(
            payload["best_match"]["route"]["tool_arguments"]["disk_path"],
            "disk:/docs/review.zip"
        );
    }

    #[test]
    fn route_goal_falls_back_to_catalog_for_unknown_query() {
        let payload = goal_route_payload("сделай что-нибудь полезное", None).expect("goal route");

        assert_eq!(payload["status"], "fallback");
        assert_eq!(payload["suggested_command"], "yacli workflow list");
        assert_eq!(
            payload["recommendations"]
                .as_array()
                .expect("recommendations")
                .len(),
            3
        );
    }
}
