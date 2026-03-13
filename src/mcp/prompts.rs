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

const INVITE_TO_CALENDAR_ARGUMENTS: &[PromptArgument] = &[
    PromptArgument {
        name: "query",
        description: "Поисковая фраза, чтобы найти письмо с приглашением.",
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
        name: "calendar",
        description: "Календарь назначения, по умолчанию default.",
        required: false,
    },
];

const PROMPTS: &[PromptDefinition] = &[
    PromptDefinition {
        name: "shared",
        title: "Общая основа yacli",
        description: "Разобраться с аккаунтом, авторизацией и базовым MCP-контекстом yacli перед работой с Почтой, Календарём или Диском.",
        arguments: SHARED_ARGUMENTS,
    },
    PromptDefinition {
        name: "mail",
        title: "Почтовый workflow yacli",
        description: "Работа с Яндекс Почтой через MCP: папки, поиск, чтение, отправка, ответ и пересылка писем.",
        arguments: MAIL_ARGUMENTS,
    },
    PromptDefinition {
        name: "calendar",
        title: "Календарный workflow yacli",
        description: "Работа с Яндекс Календарём через MCP: календари, ближайшие события, создание и удаление встреч.",
        arguments: CALENDAR_ARGUMENTS,
    },
    PromptDefinition {
        name: "disk",
        title: "Дисковый workflow yacli",
        description: "Работа с Яндекс Диском через MCP: квота, список файлов, создание папок и загрузка файлов.",
        arguments: DISK_ARGUMENTS,
    },
    PromptDefinition {
        name: "daily-briefing",
        title: "Сводка по почте и календарю",
        description: "Собрать краткую сводку: важные письма, ближайшие встречи, дедлайны и действия, которые требуют внимания.",
        arguments: DAILY_BRIEFING_ARGUMENTS,
    },
    PromptDefinition {
        name: "find-and-read",
        title: "Найти и прочитать письмо",
        description: "Найти письмо по запросу, выбрать лучший результат и прочитать содержимое.",
        arguments: FIND_AND_READ_ARGUMENTS,
    },
    PromptDefinition {
        name: "reply-with-context",
        title: "Ответить с учётом контекста",
        description: "Прочитать письмо, сверить расписание и подготовить или отправить ответ с учётом календарного контекста.",
        arguments: REPLY_WITH_CONTEXT_ARGUMENTS,
    },
    PromptDefinition {
        name: "invite-to-calendar",
        title: "Создать событие из приглашения в письме",
        description: "Найти письмо с приглашением, разобрать .ics или text/calendar вложение и импортировать нужный VEVENT в календарь.",
        arguments: INVITE_TO_CALENDAR_ARGUMENTS,
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
        "invite-to-calendar" => render_invite_to_calendar_prompt(arguments)?,
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
        ("mail", "folder")
        | ("find-and-read", "folder")
        | ("reply-with-context", "folder")
        | ("invite-to-calendar", "folder") => {
            ["INBOX", "Sent", "Drafts", "Archive", "Trash", "Spam"]
                .into_iter()
                .map(str::to_string)
                .collect()
        }
        ("calendar", "calendar") | ("invite-to-calendar", "calendar") => {
            vec!["default".to_string()]
        }
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
        "Определи нужный аккаунт yacli, проверь авторизацию и выбери правильный сервисный surface перед действием.",
    );
    let account = optional_string(arguments, "account");

    format!(
        "Ты работаешь с MCP-сервером yacli.\n\
Цель: {goal}\n\
Предпочтительный аккаунт: {}\n\
\n\
Используй такой порядок работы:\n\
1. Определи аккаунт через `yacli.account.current` или `yacli.account.list`.\n\
2. Проверь состояние авторизации через `yacli.auth.status`.\n\
3. Если нужен структурированный контекст по аккаунту, прочитай `resource://yacli/account/{{account}}` и `resource://yacli/auth/{{account}}`.\n\
4. И только потом переходи к mail, calendar или disk tools.\n\
\n\
Если нужного аккаунта нет или сервис не авторизован, явно проговори этот gap до продолжения.",
        account.unwrap_or("текущий")
    )
}

fn render_mail_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");

    Ok(format!(
        "Помоги с Яндекс Почтой через MCP-сервер yacli.\n\
Задача: {request}\n\
Аккаунт: {account}\n\
Папка: {folder}\n\
\n\
Используй такой workflow:\n\
1. При необходимости подтверди аккаунт и авторизацию через `yacli.account.current` / `yacli.auth.status`.\n\
2. Если непонятно, в какой папке искать письмо, вызови `yacli.mail.folders`.\n\
3. Для быстрого контекста используй `yacli.mail.list`, а если у пользователя есть ключевые слова — `yacli.mail.search`.\n\
4. Для точного письма используй `yacli.mail.read` по нужному UID.\n\
5. Если задача write-oriented, используй `yacli.mail.send`, `yacli.mail.reply` или `yacli.mail.forward` с минимально достаточным payload.\n\
6. Если нужно сохранить вложение или превратить `.ics` в событие, используй `yacli.mail.attachment.export` и `yacli.mail.invite.create_event`.\n\
7. В финальном ответе кратко и чётко зафиксируй отправителя, дату, тему, ключевые детали письма и результат write-действия."
    ))
}

fn render_calendar_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let calendar = optional_string(arguments, "calendar").unwrap_or("default");
    let from = optional_string(arguments, "from").unwrap_or("auto");
    let to = optional_string(arguments, "to").unwrap_or("auto");

    Ok(format!(
        "Помоги с Яндекс Календарём через MCP-сервер yacli.\n\
Задача: {request}\n\
Аккаунт: {account}\n\
Календарь: {calendar}\n\
Окно: from={from}, to={to}\n\
\n\
Используй такой workflow:\n\
1. При необходимости подтверди аккаунт и авторизацию.\n\
2. Если непонятно, в каком календаре работать, вызови `yacli.calendar.calendars`.\n\
3. Используй `yacli.calendar.events` с как можно более узким временным окном.\n\
4. Если задача write-oriented, используй `yacli.calendar.create` или `yacli.calendar.delete` с явным календарём и точными timestamps или UID.\n\
5. Верни короткую сводку по расписанию или результат write-действия: время, название, место и конфликты, если они видны.\n\
\n\
Изменение существующего события beyond create/delete через MCP пока не поддержано. Если пользователь просит редактирование на месте, явно объясни этот gap."
    ))
}

fn render_disk_prompt(arguments: &Value) -> Result<String> {
    let request = required_string(arguments, "request")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let path = optional_string(arguments, "path").unwrap_or("disk:/");

    Ok(format!(
        "Помоги с Яндекс Диском через MCP-сервер yacli.\n\
Задача: {request}\n\
Аккаунт: {account}\n\
Путь: {path}\n\
\n\
Используй такой workflow:\n\
1. При необходимости подтверди аккаунт и авторизацию.\n\
2. Для понимания квоты используй `yacli.disk.info`.\n\
3. Для просмотра содержимого используй `yacli.disk.list` по нужному пути.\n\
4. Если задача write-oriented, используй `yacli.disk.mkdir` или `yacli.disk.upload` с явным путём `disk:/...` и реальным локальным source path для upload.\n\
5. Верни краткую сводку по файлам, папкам, квоте или результату загрузки.\n\
\n\
Более сложные file mutations beyond mkdir/upload через MCP пока не поддержаны. Если пользователь просит move, delete или rename, явно объясни этот gap."
    ))
}

fn render_daily_briefing_prompt(arguments: &Value) -> String {
    let account = optional_string(arguments, "account").unwrap_or("current");
    let from = optional_string(arguments, "from").unwrap_or("today");
    let to = optional_string(arguments, "to").unwrap_or("tomorrow");
    let mail_limit = optional_string(arguments, "mail_limit").unwrap_or("10");

    format!(
        "Подготовь сводку по почте и календарю через yacli.\n\
Аккаунт: {account}\n\
Лимит писем: {mail_limit}\n\
Окно календаря: from={from}, to={to}\n\
\n\
Используй такой workflow:\n\
1. При необходимости уточни аккаунт.\n\
2. Вызови `yacli.mail.list` по INBOX с заданным лимитом.\n\
3. Вызови `yacli.calendar.events` для нужного окна.\n\
4. Собери сводку: важные или свежие письма, срочные темы, ближайшие встречи, дедлайны и очевидные конфликты.\n\
\n\
Финальный ответ держи коротким, операционным и ориентированным на действия."
    )
}

fn render_find_and_read_prompt(arguments: &Value) -> Result<String> {
    let query = required_string(arguments, "query")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");
    let limit = optional_string(arguments, "limit").unwrap_or("5");

    Ok(format!(
        "Найди и прочитай нужное письмо через MCP-сервер yacli.\n\
Запрос: {query}\n\
Аккаунт: {account}\n\
Папка: {folder}\n\
Лимит: {limit}\n\
\n\
Используй такой workflow:\n\
1. Вызови `yacli.mail.search` с заданным запросом и лимитом.\n\
2. Выбери самый релевантный UID по отправителю, теме и дате.\n\
3. Вызови `yacli.mail.read` для этого UID.\n\
4. Верни ключевое содержимое письма и явно укажи, какой UID ты выбрал."
    ))
}

fn render_reply_with_context_prompt(arguments: &Value) -> Result<String> {
    let uid = required_string(arguments, "uid")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");
    let from = optional_string(arguments, "from").unwrap_or("auto");
    let to = optional_string(arguments, "to").unwrap_or("auto");

    Ok(format!(
        "Подготовь или отправь ответ на письмо с учётом расписания через MCP-сервер yacli.\n\
UID письма: {uid}\n\
Аккаунт: {account}\n\
Папка: {folder}\n\
Окно календаря: from={from}, to={to}\n\
\n\
Используй такой workflow:\n\
1. Прочитай исходное письмо через `yacli.mail.read`.\n\
2. Определи релевантное календарное окно из письма или используй явно переданное окно.\n\
3. Проверь занятость через `yacli.calendar.events`.\n\
4. Если нужен реальный ответ, используй `yacli.mail.reply`; если нужен только черновик, явно выдай текст черновика.\n\
\n\
Явно укажи, отправлен ли ответ реально или это только предложенный draft."
    ))
}

fn render_invite_to_calendar_prompt(arguments: &Value) -> Result<String> {
    let query = required_string(arguments, "query")?;
    let account = optional_string(arguments, "account").unwrap_or("current");
    let folder = optional_string(arguments, "folder").unwrap_or("INBOX");
    let calendar = optional_string(arguments, "calendar").unwrap_or("default");

    Ok(format!(
        "Помоги создать событие в Яндекс Календаре из приглашения в письме через MCP-сервер yacli.\n\
Поисковый запрос: {query}\n\
Аккаунт: {account}\n\
Папка: {folder}\n\
Календарь назначения: {calendar}\n\
\n\
Используй такой workflow:\n\
1. Найди письмо через `yacli.mail.search` по запросу и выбери лучший UID.\n\
2. Прочитай письмо через `yacli.mail.read`, если нужно уточнить вложения и контекст.\n\
3. Разбери календарное вложение через `yacli.mail.invite.inspect`.\n\
4. Если в одном вложении несколько VEVENT, выбери нужный `event_index` и явно объясни выбор.\n\
5. Создай событие через `yacli.mail.invite.create_event` с явным календарём `{calendar}`.\n\
6. В финальном ответе кратко зафиксируй UID письма, выбранное приглашение, календарь и созданное событие.\n\
\n\
Если у VEVENT нет `SUMMARY`, `DTSTART` или `DTEND`, не пытайся импортировать его молча: явно объясни, что данные приглашения неполные."
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
        assert_eq!(prompts.len(), 8);
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
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt["name"] == "invite-to-calendar")
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
        assert!(text.contains("Аккаунт: work"));
        assert!(text.contains("Лимит писем: 15"));
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
        assert!(text.contains("используй `yacli.mail.reply`"));
        assert!(text.contains("это только предложенный draft"));
    }

    #[test]
    fn invite_to_calendar_prompt_mentions_mail_and_calendar_bridge() {
        let prompt = get_prompt(json!({
            "name": "invite-to-calendar",
            "arguments": {
                "query": "приглашение demo",
                "calendar": "team"
            }
        }))
        .expect("prompt");

        let text = prompt["messages"][0]["content"]["text"]
            .as_str()
            .expect("prompt text");
        assert!(text.contains("Поисковый запрос: приглашение demo"));
        assert!(text.contains("Календарь назначения: team"));
        assert!(text.contains("`yacli.mail.search`"));
        assert!(text.contains("`yacli.mail.invite.create_event`"));
        assert!(text.contains("resource://yacli/skill/yacli-invite-to-calendar"));
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
    fn completion_for_invite_to_calendar_calendar_filters_by_prefix() {
        let result = complete(json!({
            "ref": {
                "type": "ref/prompt",
                "name": "invite-to-calendar"
            },
            "argument": {
                "name": "calendar",
                "value": "de"
            }
        }))
        .expect("completion");

        let values = result["completion"]["values"].as_array().expect("values");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "default");
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
