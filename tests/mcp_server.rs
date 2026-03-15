use assert_cmd::Command;
use base64::Engine;
use mockito::{Matcher, Server};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command as StdCommand, Stdio};
use tempfile::tempdir;

const APP_RESOURCE_URI: &str = "ui://yacli/dashboard";
const APP_RESOURCE_URI_TEMPLATE: &str =
    "ui://yacli/dashboard{?account,section,resource,tool,skill,prompt,workflow,activity,goal}";
const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

fn yacli() -> Command {
    let mut command = Command::cargo_bin("yacli").expect("binary exists");
    command.env("YACLI_SECRET_BACKEND", "file");
    command
}

fn current_release_update_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "yacli-aarch64-apple-darwin.tar.gz",
        ("macos", "x86_64") => "yacli-x86_64-apple-darwin.tar.gz",
        ("linux", "aarch64") => "yacli-aarch64-unknown-linux-gnu.tar.gz",
        ("linux", "x86_64") => "yacli-x86_64-unknown-linux-gnu.tar.gz",
        ("windows", "x86_64") => "yacli-x86_64-pc-windows-msvc.zip",
        (os, arch) => panic!("unsupported published auto-update target {arch}-{os}"),
    }
}

fn mcp_request(id: u64, method: &str, params: Value) -> String {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("request json");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn json_line_request(id: u64, method: &str, params: Value) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("request json")
        + "\n"
}

fn mcp_notification(method: &str, params: Value) -> String {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .expect("notification json");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn mcp_response(id: u64, result: Value) -> String {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
    .expect("response json");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn json_line_notification(method: &str, params: Value) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .expect("notification json")
        + "\n"
}

fn initialize_request(ui_enabled: bool) -> String {
    initialize_request_with_capabilities(ui_enabled, None)
}

fn initialize_request_with_roots(ui_enabled: bool, list_changed: bool) -> String {
    initialize_request_with_capabilities(ui_enabled, Some(list_changed))
}

fn initialize_request_with_capabilities(
    ui_enabled: bool,
    roots_list_changed: Option<bool>,
) -> String {
    let mut capabilities = if ui_enabled {
        json!({
            "extensions": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                }
            }
        })
    } else {
        json!({})
    };
    if let Some(list_changed) = roots_list_changed {
        capabilities["roots"] = json!({
            "listChanged": list_changed
        });
    }

    mcp_request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": capabilities,
            "clientInfo": { "name": "test", "version": "0.1.0" }
        }),
    )
}

fn json_line_initialize_request(ui_enabled: bool) -> String {
    let capabilities = if ui_enabled {
        json!({
            "extensions": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                }
            }
        })
    } else {
        json!({})
    };

    json_line_request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": capabilities,
            "clientInfo": { "name": "claude-code", "version": "2.1.75" }
        }),
    )
}

fn parse_responses(stdout: &[u8]) -> Vec<Value> {
    let raw = String::from_utf8(stdout.to_vec()).expect("utf8 output");
    let mut remaining = raw.as_str();
    let mut responses = Vec::new();

    while !remaining.is_empty() {
        let (headers, rest) = remaining.split_once("\r\n\r\n").expect("header separator");
        let length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .expect("content-length")
            .parse::<usize>()
            .expect("length");
        let body = &rest[..length];
        responses.push(serde_json::from_str(body).expect("valid json-rpc response"));
        remaining = &rest[length..];
    }

    responses
}

fn parse_json_line_responses(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8(stdout.to_vec())
        .expect("utf8 output")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid json-rpc response"))
        .collect()
}

fn read_response(reader: &mut dyn BufRead) -> Value {
    let mut content_length = None::<usize>;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).expect("header line");
        assert!(bytes > 0, "response stream closed unexpectedly");
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line.trim().strip_prefix("Content-Length: ") {
            content_length = Some(value.parse::<usize>().expect("content length"));
        }
    }

    let length = content_length.expect("content length header");
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).expect("response payload");
    serde_json::from_slice(&payload).expect("json-rpc payload")
}

fn write_accounts_file(config_dir: &std::path::Path, content: &str) {
    fs::create_dir_all(config_dir).expect("config dir");
    fs::write(config_dir.join("accounts.toml"), content).expect("accounts file");
}

fn write_credentials_file(config_dir: &std::path::Path, content: &str) {
    fs::create_dir_all(config_dir).expect("config dir");
    fs::write(config_dir.join("credentials.toml"), content).expect("credentials file");
}

fn write_mock_mail_account(config_dir: &std::path::Path) {
    write_accounts_file(
        config_dir,
        r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
credential_ref = "store:mail"

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );
}

fn write_mock_calendar_account(config_dir: &std::path::Path, caldav_base_url: &str) {
    write_accounts_file(
        config_dir,
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "{caldav_base_url}"
credential_ref = "store:calendar"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
        ),
    );
}

fn write_mock_disk_account(config_dir: &std::path::Path, disk_base_url: &str) {
    write_accounts_file(
        config_dir,
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{disk_base_url}"
credential_ref = "store:disk"
"#,
        ),
    );
}

fn basic_auth_header(account: &str, app_password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{account}:{app_password}"))
    )
}

#[test]
fn mcp_stdio_initialize_returns_capabilities() {
    let output = yacli()
        .args(["mcp"])
        .write_stdin(initialize_request(true))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let response = parse_responses(&output).remove(0);
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(response["result"]["serverInfo"]["name"], "yacli");
    assert_eq!(response["result"]["capabilities"]["completions"], json!({}));
    assert_eq!(
        response["result"]["capabilities"]["prompts"]["listChanged"],
        false
    );
    assert_eq!(
        response["result"]["capabilities"]["experimental"]["io.modelcontextprotocol/ui"]["resourceTemplates"],
        true
    );
    assert_eq!(
        response["result"]["capabilities"]["experimental"]["io.modelcontextprotocol/ui"]["mimeTypes"]
            [0],
        APP_RESOURCE_MIME_TYPE
    );
    assert_eq!(
        response["result"]["capabilities"]["resources"]["subscribe"],
        true
    );
}

#[test]
fn mcp_stdio_accepts_claude_code_json_line_messages() {
    let input = [
        json_line_initialize_request(true),
        json_line_notification("notifications/initialized", json!({})),
        json_line_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let raw_output = String::from_utf8(output.clone()).expect("utf8 output");
    assert!(!raw_output.contains("Content-Length:"));

    let responses = parse_json_line_responses(&output);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "yacli");

    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let snapshot_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.app.snapshot")
        .expect("snapshot tool");
    assert_eq!(
        snapshot_tool["_meta"]["ui"]["resourceUri"],
        APP_RESOURCE_URI
    );

    let prompts_input = [
        json_line_initialize_request(true),
        json_line_notification("notifications/initialized", json!({})),
        json_line_request(2, "prompts/list", json!({})),
    ]
    .concat();

    let prompts_output = yacli()
        .args(["mcp"])
        .write_stdin(prompts_input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prompts_responses = parse_json_line_responses(&prompts_output);
    let prompts = prompts_responses[1]["result"]["prompts"]
        .as_array()
        .expect("prompts array");
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "daily-briefing")
    );
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "attachment-to-disk")
    );
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "send-file-by-mail")
    );
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "send-link-by-mail")
    );
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "publish-file-link")
    );
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "revoke-public-link")
    );
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt["name"] == "invite-to-calendar")
    );
}

#[test]
fn mcp_stdio_rejects_operational_requests_before_initialize() {
    let output = yacli()
        .args(["mcp"])
        .write_stdin(mcp_request(2, "tools/list", json!({})))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let response = parse_responses(&output).remove(0);
    assert_eq!(response["error"]["code"], -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("message")
            .contains("initialize")
    );
}

#[test]
fn mcp_stdio_lists_and_renders_embedded_prompts() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "prompts/list", json!({})),
        mcp_request(
            3,
            "prompts/get",
            json!({
                "name": "find-and-read",
                "arguments": {
                    "query": "invoice",
                    "account": "work"
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let prompts = responses[1]["result"]["prompts"]
        .as_array()
        .expect("prompts array");
    assert_eq!(prompts.len(), 13);
    let mail_prompt = prompts
        .iter()
        .find(|prompt| prompt["name"] == "mail")
        .expect("mail prompt");
    assert_eq!(mail_prompt["title"], "Почтовый workflow yacli");

    let rendered = responses[2]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .expect("prompt text");
    assert!(rendered.contains("Запрос: invoice"));
    assert!(rendered.contains("Аккаунт: work"));
    assert!(rendered.contains("yacli.mail.search"));
    assert!(rendered.contains("yacli.mail.read"));

    let invite_prompt = prompts
        .iter()
        .find(|prompt| prompt["name"] == "invite-to-calendar")
        .expect("invite prompt");
    assert_eq!(
        invite_prompt["title"],
        "Создать событие из приглашения в письме"
    );
    let send_file_prompt = prompts
        .iter()
        .find(|prompt| prompt["name"] == "send-file-by-mail")
        .expect("send-file prompt");
    assert_eq!(send_file_prompt["title"], "Отправить файл с диска по почте");
}

#[test]
fn mcp_stdio_completes_prompt_and_resource_arguments() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "completion/complete",
            json!({
                "ref": {
                    "type": "ref/prompt",
                    "name": "mail"
                },
                "argument": {
                    "name": "folder",
                    "value": "in"
                }
            }),
        ),
        mcp_request(
            3,
            "completion/complete",
            json!({
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://yacli/account/{account}"
                },
                "argument": {
                    "name": "account",
                    "value": "mo"
                }
            }),
        ),
        mcp_request(
            4,
            "completion/complete",
            json!({
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://yacli/skill/{skill}"
                },
                "argument": {
                    "name": "skill",
                    "value": "yacli-ma"
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses[1]["result"]["completion"]["values"][0], "INBOX");
    assert_eq!(responses[1]["result"]["completion"]["hasMore"], false);
    assert_eq!(responses[2]["result"]["completion"]["values"][0], "mock");
    assert_eq!(
        responses[3]["result"]["completion"]["values"][0],
        "yacli-mail"
    );
}

#[test]
fn mcp_stdio_requests_client_roots_and_refreshes_after_list_changed() {
    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("yacli"))
        .arg("mcp")
        .env("YACLI_SECRET_BACKEND", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn yacli mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    stdin
        .write_all(initialize_request_with_roots(true, true).as_bytes())
        .expect("write initialize");
    let initialize = read_response(&mut reader);
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");

    stdin
        .write_all(mcp_notification("notifications/initialized", json!({})).as_bytes())
        .expect("write initialized");
    stdin
        .write_all(mcp_request(2, "tools/list", json!({})).as_bytes())
        .expect("write tools/list");
    let tools_list = read_response(&mut reader);
    let tools = tools_list["result"]["tools"].as_array().expect("tools");
    let roots_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.roots.list")
        .expect("roots tool");
    assert_eq!(roots_tool["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI);

    stdin
        .write_all(
            mcp_request(
                3,
                "tools/call",
                json!({
                    "name": "yacli.roots.list",
                    "arguments": {}
                }),
            )
            .as_bytes(),
        )
        .expect("write roots tool call");
    let outbound_roots_request = read_response(&mut reader);
    assert_eq!(outbound_roots_request["method"], "roots/list");
    let request_id = outbound_roots_request["id"].as_u64().expect("request id");

    stdin
        .write_all(
            mcp_response(
                request_id,
                json!({
                    "roots": [
                        {
                            "uri": "file:///tmp/project-a",
                            "name": "project-a"
                        }
                    ]
                }),
            )
            .as_bytes(),
        )
        .expect("write roots response");
    let roots_tool_result = read_response(&mut reader);
    assert_eq!(
        roots_tool_result["result"]["structuredContent"]["roots"][0]["uri"],
        "file:///tmp/project-a"
    );
    assert_eq!(
        roots_tool_result["result"]["structuredContent"]["roots"][0]["name"],
        "project-a"
    );

    stdin
        .write_all(mcp_notification("notifications/roots/list_changed", json!({})).as_bytes())
        .expect("write roots changed");
    stdin
        .write_all(
            mcp_request(
                4,
                "tools/call",
                json!({
                    "name": "yacli.roots.list",
                    "arguments": {}
                }),
            )
            .as_bytes(),
        )
        .expect("write second roots tool call");
    let refreshed_outbound_roots_request = read_response(&mut reader);
    assert_eq!(refreshed_outbound_roots_request["method"], "roots/list");
    let refreshed_request_id = refreshed_outbound_roots_request["id"]
        .as_u64()
        .expect("request id");
    assert_ne!(refreshed_request_id, request_id);

    stdin
        .write_all(
            mcp_response(
                refreshed_request_id,
                json!({
                    "roots": [
                        {
                            "uri": "file:///tmp/project-b",
                            "name": "project-b"
                        }
                    ]
                }),
            )
            .as_bytes(),
        )
        .expect("write refreshed roots response");
    let refreshed_roots_result = read_response(&mut reader);
    assert_eq!(
        refreshed_roots_result["result"]["structuredContent"]["roots"][0]["uri"],
        "file:///tmp/project-b"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn mcp_stdio_does_not_advertise_roots_without_client_capability() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(tools.iter().all(|tool| tool["name"] != "yacli.roots.list"));
}

#[test]
fn mcp_stdio_exposes_mail_write_tools_and_validates_send_reply_forward() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "yacli.mail.send",
                "arguments": {
                    "account": "mock",
                    "to": "broken-recipient",
                    "subject": "Hello",
                    "text": "Body"
                }
            }),
        ),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "yacli.mail.reply",
                "arguments": {
                    "account": "mock",
                    "uid": 42
                }
            }),
        ),
        mcp_request(
            5,
            "tools/call",
            json!({
                "name": "yacli.mail.forward",
                "arguments": {
                    "account": "mock",
                    "uid": 42,
                    "to": "broken-recipient"
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(tools.iter().any(|tool| tool["name"] == "yacli.mail.send"));
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.mail.send_link")
    );
    assert!(tools.iter().any(|tool| tool["name"] == "yacli.mail.reply"));
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.mail.forward")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.mail.attachment.export")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.mail.invite.inspect")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.mail.invite.create_event")
    );

    assert_eq!(responses[2]["error"]["code"], -32602);
    assert!(
        responses[2]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("mail send recipient must contain `@`")
    );

    assert_eq!(responses[3]["error"]["code"], -32602);
    assert!(
        responses[3]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("mail reply requires text or --html")
    );

    assert_eq!(responses[4]["error"]["code"], -32602);
    assert!(
        responses[4]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("mail forward recipient must contain `@`")
    );

    let attachment_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.mail.attachment.export")
        .expect("attachment export tool");
    assert_eq!(
        attachment_tool["inputSchema"]["required"],
        json!(["uid", "output_path"])
    );
    let send_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.mail.send")
        .expect("send tool");
    assert_eq!(
        send_tool["inputSchema"]["properties"]["attachments"]["type"],
        "array"
    );
    assert_eq!(
        send_tool["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );
    let send_link_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.mail.send_link")
        .expect("send-link tool");
    assert_eq!(
        send_link_tool["inputSchema"]["required"],
        json!(["to", "subject", "source_path", "disk_path"])
    );
    assert_eq!(
        send_link_tool["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );
    let invite_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.mail.invite.inspect")
        .expect("invite inspect tool");
    assert_eq!(invite_tool["inputSchema"]["required"], json!(["uid"]));
    let invite_create_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.mail.invite.create_event")
        .expect("invite create tool");
    assert_eq!(
        invite_create_tool["inputSchema"]["required"],
        json!(["uid"])
    );
    let calendar_create_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.calendar.create")
        .expect("calendar create tool");
    assert_eq!(
        calendar_create_tool["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );
}

#[test]
fn mcp_stdio_mail_send_dry_run_reviews_message_without_network() {
    let temp = tempdir().expect("tempdir");
    let attachment_path = temp.path().join("invoice.txt");
    fs::write(&attachment_path, "invoice body").expect("attachment");
    write_mock_mail_account(temp.path());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.mail.send",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Hello",
                    "text": "Body",
                    "attachments": [attachment_path.display().to_string()],
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["dry_run"], true);
    assert_eq!(structured["review"]["attachment_count"], 1);
    assert_eq!(
        structured["review"]["attachments"][0]["filename"],
        "invoice.txt"
    );
    assert_eq!(structured["review"]["sent"]["body_kind"], "multipart_mixed");
}

#[test]
fn mcp_stdio_mail_send_link_dry_run_reviews_upload_publish_and_mail_without_network() {
    let temp = tempdir().expect("tempdir");
    let source_path = temp.path().join("archive.zip");
    fs::write(&source_path, "archive body").expect("source");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
credential_ref = "store:mail"

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
credential_ref = "store:disk"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.mail.send_link",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Материалы",
                    "text": "Отправляю ссылку",
                    "source_path": source_path.display().to_string(),
                    "disk_path": "disk:/docs/archive.zip",
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["dry_run"], true);
    assert_eq!(
        structured["review"]["upload"]["remote_path"],
        "disk:/docs/archive.zip"
    );
    assert_eq!(
        structured["review"]["link_placeholder"],
        "<публичная ссылка на файл>"
    );
    assert_eq!(
        structured["review"]["mail_review"]["sent"]["subject"],
        "Материалы"
    );
}

#[test]
fn mcp_stdio_mail_send_link_returns_partial_recovery_when_smtp_fails_after_publish() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();
    let source_path = temp.path().join("archive.zip");
    fs::write(&source_path, "archive body").expect("source");
    write_accounts_file(
        temp.path(),
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "127.0.0.1"
smtp_port = 9
credential_ref = "store:mail"

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{}"
credential_ref = "store:disk"
"#,
            server.url()
        ),
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _ticket = server
        .mock("GET", "/v1/disk/resources/upload")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/archive.zip".into()),
            Matcher::UrlEncoded("overwrite".into(), "false".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"href":"{}/upload-target/archive.zip","method":"PUT","templated":false}}"#,
            server.url()
        ))
        .create();
    let _upload = server
        .mock("PUT", "/upload-target/archive.zip")
        .match_body(Matcher::Exact("archive body".to_string()))
        .with_status(201)
        .create();
    let _publish = server
        .mock("PUT", "/v1/disk/resources/publish")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/archive.zip".into(),
        ))
        .with_status(200)
        .create();
    let _metadata = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/archive.zip".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "archive.zip",
  "path": "disk:/docs/archive.zip",
  "type": "file",
  "size": 12,
  "mime_type": "application/zip",
  "public_url": "https://disk.yandex.ru/i/archive-link",
  "public_key": "archive-link-key",
  "revision": 7
}"#,
        )
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.mail.send_link",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Материалы",
                    "text": "Отправляю ссылку",
                    "source_path": source_path.display().to_string(),
                    "disk_path": "disk:/docs/archive.zip"
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["status"], "partial");
    assert_eq!(
        structured["partial"]["recovery"]["share_public_link"]["public_url"],
        "https://disk.yandex.ru/i/archive-link"
    );
    assert_eq!(
        structured["partial"]["recovery"]["retry_mail_step"]["tool"],
        "yacli.mail.send_published_link"
    );

    let activity = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["activity", "list", "--limit", "10"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let activity_value: Value = serde_json::from_slice(&activity).expect("activity json");
    let items = activity_value["items"].as_array().expect("items");
    assert_eq!(items[0]["operation"], "mail.send_link.partial");
    assert_eq!(items[0]["source"], "mcp");
}

#[test]
fn mcp_stdio_mail_send_dry_run_recommends_send_link_for_oversized_attachment() {
    let temp = tempdir().expect("tempdir");
    let attachment_path = temp.path().join("archive.zip");
    fs::write(&attachment_path, vec![b'x'; 20 * 1024 * 1024]).expect("attachment");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
credential_ref = "store:mail"

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.mail.send",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Большой архив",
                    "text": "Материалы во вложении",
                    "attachments": [attachment_path.display().to_string()],
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(
        structured["review"]["delivery_posture"],
        "send_link_recommended"
    );
    assert_eq!(
        structured["review"]["remediation"]["workflow"],
        "send-link-by-mail"
    );
    assert_eq!(
        structured["review"]["remediation"]["suggested_disk_path"],
        "disk:/uploads/archive.zip"
    );
}

#[test]
fn mcp_stdio_mail_attachment_export_requires_selector_before_network() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(
            [
                initialize_request(true),
                mcp_notification("notifications/initialized", json!({})),
                mcp_request(
                    2,
                    "tools/call",
                    json!({
                        "name": "yacli.mail.attachment.export",
                        "arguments": {
                            "account": "mock",
                            "uid": 42,
                            "output_path": temp.path().join("invoice.pdf").display().to_string(),
                        }
                    }),
                ),
            ]
            .concat(),
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses[1]["error"]["code"], -32602);
    assert!(
        responses[1]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("yacli.mail.attachment.export requires `index` or `name`")
    );
}

#[test]
fn mcp_stdio_mail_invite_inspect_requires_selector_before_network() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(
            [
                initialize_request(true),
                mcp_notification("notifications/initialized", json!({})),
                mcp_request(
                    2,
                    "tools/call",
                    json!({
                        "name": "yacli.mail.invite.inspect",
                        "arguments": {
                            "account": "mock",
                            "uid": 42
                        }
                    }),
                ),
            ]
            .concat(),
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses[1]["error"]["code"], -32602);
    assert!(
        responses[1]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("yacli.mail.invite.inspect requires `index` or `name`")
    );
}

#[test]
fn mcp_stdio_mail_invite_create_event_requires_selector_before_network() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(
            [
                initialize_request(true),
                mcp_notification("notifications/initialized", json!({})),
                mcp_request(
                    2,
                    "tools/call",
                    json!({
                        "name": "yacli.mail.invite.create_event",
                        "arguments": {
                            "account": "mock",
                            "uid": 42
                        }
                    }),
                ),
            ]
            .concat(),
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses[1]["error"]["code"], -32602);
    assert!(
        responses[1]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("yacli.mail.invite.create_event requires `index` or `name`")
    );
}

#[test]
fn mcp_stdio_mail_send_rejects_directory_attachment_before_network() {
    let temp = tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(
            [
                initialize_request(true),
                mcp_notification("notifications/initialized", json!({})),
                mcp_request(
                    2,
                    "tools/call",
                    json!({
                        "name": "yacli.mail.send",
                        "arguments": {
                            "account": "mock",
                            "to": "person@example.com",
                            "subject": "Hello",
                            "text": "Body",
                            "attachments": [temp.path().display().to_string()]
                        }
                    }),
                ),
            ]
            .concat(),
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    assert_eq!(responses[1]["error"]["code"], -32601);
    assert!(
        responses[1]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("attachment path points to a directory")
    );
}

#[test]
fn mcp_stdio_exposes_calendar_write_tools_and_executes_create_delete() {
    let temp = tempdir().expect("tempdir");
    let mut caldav = Server::new();
    write_mock_calendar_account(temp.path(), &caldav.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let auth = basic_auth_header("me@yandex.ru", "calendar-secret");

    let _principal = caldav
        .mock("PROPFIND", "/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/me@yandex.ru/</d:href>
        </d:current-user-principal>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .expect(2)
        .create();

    let _home = caldav
        .mock("PROPFIND", "/principals/users/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/me@yandex.ru/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .expect(2)
        .create();

    let _collections = caldav
        .mock("PROPFIND", "/calendars/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Личный</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .expect(4)
        .create();

    let _put = caldav
        .mock(
            "PUT",
            Matcher::Regex("/calendars/me@yandex\\.ru/default/[^/]+\\.ics".to_string()),
        )
        .match_header("authorization", auth.as_str())
        .match_header("content-type", "text/calendar; charset=utf-8")
        .match_header("if-none-match", "*")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("SUMMARY:Синк команды".to_string()),
            Matcher::Regex("DTSTART:20260312T090000Z".to_string()),
            Matcher::Regex("DTEND:20260312T100000Z".to_string()),
        ]))
        .with_status(201)
        .with_header("etag", "\"new-evt\"")
        .create();

    let _create_report = caldav
        .mock("REPORT", "/calendars/me@yandex.ru/default/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .match_body(Matcher::Regex("prop-filter name=\"UID\"".to_string()))
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body_from_request(|request| {
            let body = request
                .utf8_lossy_body()
                .expect("request body available")
                .into_owned();
            let uid = body
                .split("<c:text-match collation=\"i;octet\">")
                .nth(1)
                .and_then(|value| value.split("</c:text-match>").next())
                .expect("uid filter present");
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/canonical-event.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"new-evt"</d:getetag>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:{uid}
SUMMARY:Синк команды
DTSTART:20260312T090000Z
DTEND:20260312T100000Z
END:VEVENT
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#
            )
            .into_bytes()
        })
        .create();

    let _delete_report = caldav
        .mock("REPORT", "/calendars/me@yandex.ru/default/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .match_body(Matcher::Regex(
            "text-match collation=\"i;octet\">event-1<".to_string(),
        ))
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/event-1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"evt-1"</d:getetag>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:event-1
SUMMARY:Удаляемое событие
DTSTART:20260312T090000Z
DTEND:20260312T100000Z
END:VEVENT
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _delete = caldav
        .mock("DELETE", "/calendars/me@yandex.ru/default/event-1.ics")
        .match_header("authorization", auth.as_str())
        .match_header("if-match", "\"evt-1\"")
        .with_status(204)
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "yacli.calendar.create",
                "arguments": {
                    "account": "mock",
                    "summary": "Синк команды",
                    "start": "2026-03-12T09:00:00Z",
                    "end": "2026-03-12T10:00:00Z"
                }
            }),
        ),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "yacli.calendar.delete",
                "arguments": {
                    "account": "mock",
                    "uid": "event-1"
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.calendar.create")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.calendar.delete")
    );

    assert_eq!(
        responses[2]["result"]["structuredContent"]["calendar"]["id"],
        "default"
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["event"]["summary"],
        "Синк команды"
    );
    assert_eq!(
        responses[3]["result"]["structuredContent"]["deleted_event"]["uid"],
        "event-1"
    );
    assert_eq!(
        responses[3]["result"]["structuredContent"]["deleted_event"]["summary"],
        "Удаляемое событие"
    );
}

#[test]
fn mcp_stdio_calendar_create_dry_run_reviews_event_without_network_write() {
    let temp = tempdir().expect("tempdir");
    let mut caldav = Server::new();

    write_mock_calendar_account(temp.path(), &caldav.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let auth = basic_auth_header("me@yandex.ru", "calendar-secret");

    let _principal = caldav
        .mock("PROPFIND", "/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/me@yandex.ru/</d:href>
        </d:current-user-principal>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _home = caldav
        .mock("PROPFIND", "/principals/users/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/me@yandex.ru/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _collections = caldav
        .mock("PROPFIND", "/calendars/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Личный</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.calendar.create",
                "arguments": {
                    "account": "mock",
                    "summary": "Синк команды",
                    "start": "2026-03-12T09:00:00Z",
                    "end": "2026-03-12T10:00:00Z",
                    "location": "Meet",
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["calendar"]["id"], "default");
    assert_eq!(structured["dry_run"], true);
    assert_eq!(structured["review"]["summary"], "Синк команды");
    assert_eq!(structured["review"]["start"], "2026-03-12T09:00:00Z");
    assert_eq!(structured["review"]["end"], "2026-03-12T10:00:00Z");
    assert_eq!(structured["review"]["status"], "CONFIRMED");
}

#[test]
fn mcp_stdio_exposes_disk_write_tools_and_executes_mkdir_upload() {
    let temp = tempdir().expect("tempdir");
    let mut disk = Server::new();
    write_mock_disk_account(temp.path(), &disk.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let source_path = temp.path().join("note.txt");
    fs::write(&source_path, b"hello disk").expect("source file");

    let _mkdir = disk
        .mock("PUT", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/new-folder".into(),
        ))
        .with_status(201)
        .create();

    let _mkdir_metadata = disk
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/new-folder".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "new-folder",
  "path": "disk:/docs/new-folder",
  "type": "dir",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:00+00:00",
  "revision": 18,
  "_embedded": {
    "limit": 100,
    "offset": 0,
    "total": 0,
    "items": []
  }
}"#,
        )
        .create();

    let _ticket = disk
        .mock("GET", "/v1/disk/resources/upload")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/note.txt".into()),
            Matcher::UrlEncoded("overwrite".into(), "false".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
  "href": "{}/upload-target/note.txt",
  "method": "PUT",
  "templated": false
}}"#,
            disk.url()
        ))
        .create();

    let _upload = disk
        .mock("PUT", "/upload-target/note.txt")
        .match_body(Matcher::Exact("hello disk".to_string()))
        .with_status(201)
        .create();

    let _upload_metadata = disk
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/note.txt".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "note.txt",
  "path": "disk:/docs/note.txt",
  "type": "file",
  "size": 10,
  "mime_type": "text/plain",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}"#,
        )
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "yacli.disk.mkdir",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/new-folder"
                }
            }),
        ),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "yacli.disk.upload",
                "arguments": {
                    "account": "mock",
                    "source": source_path.to_str().expect("utf8 path"),
                    "path": "disk:/docs/note.txt"
                }
            }),
        ),
        mcp_request(
            5,
            "resources/read",
            json!({
                "uri": "resource://yacli/activity"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(tools.iter().any(|tool| tool["name"] == "yacli.disk.mkdir"));
    assert!(tools.iter().any(|tool| tool["name"] == "yacli.disk.upload"));
    let disk_upload_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.disk.upload")
        .expect("disk upload tool");
    assert_eq!(
        disk_upload_tool["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );
    let disk_publish_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.disk.publish")
        .expect("disk publish tool");
    assert_eq!(
        disk_publish_tool["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );
    let disk_unpublish_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.disk.unpublish")
        .expect("disk unpublish tool");
    assert_eq!(
        disk_unpublish_tool["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );
    let disk_upload_link_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.disk.upload_link")
        .expect("disk upload-link tool");
    assert_eq!(
        disk_upload_link_tool["inputSchema"]["properties"]["dry_run"]["type"],
        "boolean"
    );

    assert_eq!(
        responses[2]["result"]["structuredContent"]["resource"]["resource_type"],
        "dir"
    );
    assert_eq!(
        responses[3]["result"]["structuredContent"]["resource"]["resource_type"],
        "file"
    );
    assert_eq!(
        responses[3]["result"]["structuredContent"]["upload"]["bytes_written"],
        10
    );

    let activity_catalog_contents = responses[4]["result"]["contents"]
        .as_array()
        .expect("activity catalog contents");
    let activity_catalog_payload: Value = serde_json::from_str(
        activity_catalog_contents[0]["text"]
            .as_str()
            .expect("activity catalog text"),
    )
    .expect("activity catalog json");
    let activity_items = activity_catalog_payload["items"]
        .as_array()
        .expect("activity items");
    assert_eq!(activity_items.len(), 2);
    assert_eq!(activity_items[0]["operation"], "disk.upload");
    assert_eq!(activity_items[0]["source"], "mcp");
    assert_eq!(activity_items[1]["operation"], "disk.mkdir");
    assert_eq!(activity_items[1]["source"], "mcp");

    let activity_output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["activity", "list", "--limit", "5"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let activity_value: Value = serde_json::from_slice(&activity_output).expect("activity json");
    let items = activity_value["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["operation"], "disk.upload");
    assert_eq!(items[0]["source"], "mcp");
    assert_eq!(items[1]["operation"], "disk.mkdir");
    assert_eq!(items[1]["source"], "mcp");

    let activity_id = items[0]["id"].as_str().expect("activity id");
    let detail_input = [
        initialize_request(false),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": format!("resource://yacli/activity/{activity_id}")
            }),
        ),
    ]
    .concat();

    let detail_output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(detail_input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let detail_responses = parse_responses(&detail_output);
    let activity_detail_contents = detail_responses[1]["result"]["contents"]
        .as_array()
        .expect("activity detail contents");
    let activity_detail_payload: Value = serde_json::from_str(
        activity_detail_contents[0]["text"]
            .as_str()
            .expect("activity detail text"),
    )
    .expect("activity detail json");
    assert_eq!(activity_detail_payload["id"], activity_id);
    assert_eq!(activity_detail_payload["operation"], "disk.upload");
    assert_eq!(activity_detail_payload["source"], "mcp");
    assert!(
        activity_detail_payload["replay_command"]
            .as_str()
            .expect("replay command")
            .contains("--dry-run")
    );
}

#[test]
fn mcp_stdio_exposes_disk_download_tool_and_downloads_private_file() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();
    let output_path = temp.path().join("downloads").join("guide.pdf");
    let body = b"%PDF-1.4\nprivate pdf\n";

    write_accounts_file(
        temp.path(),
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{}"
credential_ref = "store:disk"
"#,
            server.url()
        ),
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    let _metadata = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/guide.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
  "name": "guide.pdf",
  "path": "disk:/docs/guide.pdf",
  "type": "file",
  "size": {},
  "mime_type": "application/pdf",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}}"#,
            body.len()
        ))
        .create();

    let _ticket = server
        .mock("GET", "/v1/disk/resources/download")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/guide.pdf".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
  "href": "{}/download/private-guide.pdf",
  "method": "GET"
}}"#,
            server.url()
        ))
        .create();

    let _download = server
        .mock("GET", "/download/private-guide.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(body.as_slice())
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "yacli.disk.download",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/guide.pdf",
                    "output_path": output_path.to_str().expect("utf8 path")
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_DISK_TOKEN", "disk-token")
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.disk.download")
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["resource"]["resource_type"],
        "file"
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["download"]["bytes_written"],
        body.len()
    );
    assert_eq!(
        fs::read(&output_path).expect("downloaded file"),
        body.as_slice()
    );
}

#[test]
fn mcp_stdio_exposes_disk_publish_tool_and_returns_public_link() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_mock_disk_account(temp.path(), &server.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _publish = server
        .mock("PUT", "/v1/disk/resources/publish")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/report.pdf".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("{}")
        .create();

    let _metadata = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/report.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "report.pdf",
  "path": "disk:/docs/report.pdf",
  "type": "file",
  "size": 42,
  "mime_type": "application/pdf",
  "public_url": "https://disk.yandex.ru/i/public-report",
  "public_key": "public-key-report",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}"#,
        )
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "yacli.disk.publish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf"
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.disk.publish")
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["resource"]["public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_stdio_activity_undo_revokes_public_link_for_reversible_publish() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_mock_disk_account(temp.path(), &server.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _publish = server
        .mock("PUT", "/v1/disk/resources/publish")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/report.pdf".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("{}")
        .create();

    let _metadata_after_publish = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/report.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "report.pdf",
  "path": "disk:/docs/report.pdf",
  "type": "file",
  "size": 42,
  "mime_type": "application/pdf",
  "public_url": "https://disk.yandex.ru/i/public-report",
  "public_key": "public-key-report",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}"#,
        )
        .expect(2)
        .create();

    let _unpublish = server
        .mock("PUT", "/v1/disk/resources/unpublish")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/report.pdf".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("{}")
        .create();

    let _metadata_after_unpublish = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/report.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "report.pdf",
  "path": "disk:/docs/report.pdf",
  "type": "file",
  "size": 42,
  "mime_type": "application/pdf",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:03+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 20
}"#,
        )
        .create();

    let publish_input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "yacli.disk.publish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf"
                }
            }),
        ),
        mcp_request(
            4,
            "resources/read",
            json!({
                "uri": "resource://yacli/activity"
            }),
        ),
    ]
    .concat();

    let publish_output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(publish_input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let publish_responses = parse_responses(&publish_output);
    let tools = publish_responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.activity.undo")
    );
    let activity_contents = publish_responses[3]["result"]["contents"]
        .as_array()
        .expect("activity contents");
    let activity_payload: Value = serde_json::from_str(
        activity_contents[0]["text"]
            .as_str()
            .expect("activity text"),
    )
    .expect("activity payload");
    let activity_id = activity_payload["items"][0]["id"]
        .as_str()
        .expect("activity id");

    let undo_input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.activity.undo",
                "arguments": {
                    "id": activity_id
                }
            }),
        ),
    ]
    .concat();

    let undo_output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(undo_input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let undo_responses = parse_responses(&undo_output);
    assert_eq!(
        undo_responses[1]["result"]["structuredContent"]["undo"]["result"]["kind"],
        "disk_unpublish"
    );
    assert_eq!(
        undo_responses[1]["result"]["structuredContent"]["undo"]["result"]["result"]["revoked_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_stdio_exposes_disk_unpublish_tool_and_revokes_public_link() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_mock_disk_account(temp.path(), &server.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _metadata_before = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/report.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "report.pdf",
  "path": "disk:/docs/report.pdf",
  "type": "file",
  "size": 42,
  "mime_type": "application/pdf",
  "public_url": "https://disk.yandex.ru/i/public-report",
  "public_key": "public-key-report",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}"#,
        )
        .expect(1)
        .create();

    let _unpublish = server
        .mock("PUT", "/v1/disk/resources/unpublish")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/report.pdf".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("{}")
        .create();

    let _metadata_after = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/report.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "report.pdf",
  "path": "disk:/docs/report.pdf",
  "type": "file",
  "size": 42,
  "mime_type": "application/pdf",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 20
}"#,
        )
        .expect(1)
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(
            3,
            "tools/call",
            json!({
                "name": "yacli.disk.unpublish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf"
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.disk.unpublish")
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["result"]["was_public"],
        true
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["result"]["revoked_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_stdio_disk_publish_dry_run_reviews_public_link_without_network_write() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_mock_disk_account(temp.path(), &server.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _metadata = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/report.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "report.pdf",
  "path": "disk:/docs/report.pdf",
  "type": "file",
  "size": 42,
  "mime_type": "application/pdf",
  "public_url": "https://disk.yandex.ru/i/public-report",
  "public_key": "public-key-report",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}"#,
        )
        .expect(1)
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.disk.publish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf",
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["dry_run"], true);
    assert_eq!(structured["review"]["already_public"], true);
    assert_eq!(
        structured["review"]["current_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_stdio_disk_unpublish_dry_run_reviews_public_link_without_network_write() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_mock_disk_account(temp.path(), &server.url());
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _metadata = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/report.pdf".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "report.pdf",
  "path": "disk:/docs/report.pdf",
  "type": "file",
  "size": 42,
  "mime_type": "application/pdf",
  "public_url": "https://disk.yandex.ru/i/public-report",
  "public_key": "public-key-report",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}"#,
        )
        .expect(1)
        .create();

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.disk.unpublish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf",
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["dry_run"], true);
    assert_eq!(structured["review"]["is_public"], true);
    assert_eq!(
        structured["review"]["current_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_stdio_disk_upload_dry_run_reviews_file_without_network_write() {
    let temp = tempdir().expect("tempdir");
    let source_path = temp.path().join("note.txt");
    fs::write(&source_path, b"hello disk").expect("source file");

    write_mock_disk_account(temp.path(), "https://cloud-api.yandex.net");
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.disk.upload",
                "arguments": {
                    "account": "mock",
                    "source": source_path.to_str().expect("utf8 path"),
                    "path": "disk:/docs/note.txt",
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["dry_run"], true);
    assert_eq!(structured["path"], "disk:/docs/note.txt");
    assert_eq!(structured["upload"]["remote_path"], "disk:/docs/note.txt");
    assert_eq!(structured["upload"]["bytes_written"], 10);
}

#[test]
fn mcp_stdio_disk_upload_link_dry_run_reviews_upload_and_publish_without_network_write() {
    let temp = tempdir().expect("tempdir");
    let source_path = temp.path().join("note.txt");
    fs::write(&source_path, b"hello disk").expect("source file");

    write_mock_disk_account(temp.path(), "https://cloud-api.yandex.net");
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.disk.upload_link",
                "arguments": {
                    "account": "mock",
                    "source": source_path.to_str().expect("utf8 path"),
                    "path": "disk:/docs/note.txt",
                    "dry_run": true
                }
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["dry_run"], true);
    assert_eq!(
        structured["review"]["upload"]["remote_path"],
        "disk:/docs/note.txt"
    );
    assert_eq!(structured["review"]["publish_path"], "disk:/docs/note.txt");
}

#[test]
fn mcp_stdio_apps_capable_clients_receive_ui_metadata_and_resources() {
    let input = [
        initialize_request(true),
        mcp_notification("notifications/initialized", json!({})),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "resources/list", json!({})),
        mcp_request(6, "resources/templates/list", json!({})),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "yacli.app.snapshot",
                "arguments": {}
            }),
        ),
        mcp_request(
            5,
            "resources/read",
            json!({
                "uri": APP_RESOURCE_URI
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let snapshot_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.app.snapshot")
        .expect("snapshot tool");
    assert_eq!(
        snapshot_tool["_meta"]["ui"]["resourceUri"],
        APP_RESOURCE_URI
    );
    assert_eq!(snapshot_tool["_meta"]["ui"]["visibility"][0], "app");
    assert!(snapshot_tool["_meta"]["ui"]["visibility"].get(1).is_none());

    let account_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.account.current")
        .expect("account tool");
    assert_eq!(account_tool["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI);
    assert_eq!(account_tool["_meta"]["ui"]["visibility"][0], "model");
    assert_eq!(account_tool["_meta"]["ui"]["visibility"][1], "app");

    let update_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.update.check")
        .expect("update tool");
    assert_eq!(update_tool["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI);
    assert_eq!(update_tool["_meta"]["ui"]["visibility"][0], "model");
    assert_eq!(update_tool["_meta"]["ui"]["visibility"][1], "app");

    let resources = responses[2]["result"]["resources"]
        .as_array()
        .expect("resources array");
    let dashboard = resources
        .iter()
        .find(|resource| resource["uri"] == APP_RESOURCE_URI)
        .expect("dashboard resource");
    assert_eq!(dashboard["mimeType"], APP_RESOURCE_MIME_TYPE);
    assert_eq!(dashboard["_meta"]["ui"]["prefersBorder"], true);
    assert_eq!(dashboard["_meta"]["ui"]["csp"]["connectDomains"], json!([]));

    let templates = responses[3]["result"]["resourceTemplates"]
        .as_array()
        .expect("resource templates array");
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == APP_RESOURCE_URI_TEMPLATE)
    );

    assert_eq!(
        responses[4]["result"]["structuredContent"]["appResourceUri"],
        APP_RESOURCE_URI
    );
    assert_eq!(
        responses[4]["result"]["_meta"]["ui"]["resourceUri"],
        APP_RESOURCE_URI
    );
    assert_eq!(
        responses[4]["result"]["_meta"]["ui"]["visibility"][0],
        "app"
    );

    let contents = responses[5]["result"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents[0]["mimeType"], APP_RESOURCE_MIME_TYPE);
    assert_eq!(contents[0]["_meta"]["ui"]["prefersBorder"], true);
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("ui/initialize"));
    assert!(html.contains("ui/notifications/initialized"));
    assert!(html.contains("ui/notifications/tool-input"));
    assert!(html.contains("ui/notifications/tool-result"));
    assert!(html.contains("tools/call"));
    assert!(html.contains("yacli.app.snapshot"));
    assert!(html.contains("Refresh snapshot"));
    assert!(html.contains("tool-name-input"));
    assert!(html.contains("tool-args-input"));
    assert!(html.contains("tool-schema-json"));
    assert!(html.contains("tool-review-json"));
    assert!(html.contains("Preview action"));
    assert!(html.contains("Apply reviewed action"));
    assert!(html.contains("Open suggested flow"));
    assert!(html.contains("Run selected tool"));
    assert!(html.contains("ui/open-link"));
    assert!(html.contains("ui/request-display-mode"));
    assert!(html.contains("ui/update-model-context"));
    assert!(html.contains("ui/message"));
    assert!(html.contains("Host profile"));
    assert!(html.contains("host-profile-summary"));
    assert!(html.contains("host-capability-grid"));
    assert!(html.contains("host-recommendations"));
    assert!(html.contains("apps-rich"));
    assert!(html.contains("hybrid"));
    assert!(html.contains("text-first"));
    assert!(html.contains("Open auth issuer"));
    assert!(html.contains("Open resource metadata"));
    assert!(html.contains("Share auth context"));
    assert!(html.contains("authChallengeFromError"));
    assert!(html.contains("resources/subscribe"));
    assert!(html.contains("resources/unsubscribe"));
    assert!(html.contains("notifications/resources/updated"));
    assert!(html.contains("resources/templates/list"));
    assert!(html.contains("resources/read"));
    assert!(html.contains("Read account resource"));
    assert!(html.contains("Read auth resource"));
    assert!(html.contains("Read skills catalog"));
    assert!(html.contains("Read selected skill"));
    assert!(html.contains("skill-input"));
    assert!(html.contains("Workflow Hub"));
    assert!(html.contains("Goal Router"));
    assert!(html.contains("goal-input"));
    assert!(html.contains("Route goal"));
    assert!(html.contains("Open recommended workflow"));
    assert!(html.contains("Open recommended prompt"));
    assert!(html.contains("Open goal onboarding"));
    assert!(html.contains("Open goal doctor"));
    assert!(html.contains("Preview routed action"));
    assert!(html.contains("Apply routed action"));
    assert!(html.contains("Share goal remediation"));
    assert!(html.contains("Share goal replay"));
    assert!(html.contains("Share goal route"));
    assert!(html.contains("yacli.goal.route"));
    assert!(html.contains("workflow-name-input"));
    assert!(html.contains("List workflows"));
    assert!(html.contains("Open selected workflow"));
    assert!(html.contains("Open workflow prompt"));
    assert!(html.contains("Open workflow skill"));
    assert!(html.contains("Open workflow runner"));
    assert!(html.contains("Preview workflow action"));
    assert!(html.contains("Apply workflow action"));
    assert!(html.contains("Share workflow replay"));
    assert!(html.contains("resource://yacli/workflows"));
    assert!(html.contains("resource://yacli/workflow/"));
    assert!(html.contains("renderWorkflowCatalog"));
    assert!(html.contains("renderWorkflow"));
    assert!(html.contains("workflowCatalogItems"));
    assert!(html.contains("workflowDetailPayload"));
    assert!(html.contains("workflowDryRunToolName"));
    assert!(html.contains("workflowPrimaryToolName"));
    assert!(html.contains("workflowPrimaryToolArguments"));
    assert!(html.contains("workflowPrimaryOperation"));
    assert!(html.contains("workflowSupportsReview"));
    assert!(html.contains("goalPrimaryOperation"));
    assert!(html.contains("goalSupportsReview"));
    assert!(html.contains("workflowAutofillArguments"));
    assert!(html.contains("pathBasename"));
    assert!(html.contains("inferredDiskPathFromSourcePath"));
    assert!(html.contains("setWorkflowRunnerState"));
    assert!(html.contains("reviewRemediation"));
    assert!(html.contains("remediationPrefillSendLinkArgs"));
    assert!(html.contains("openToolRemediation"));
    assert!(html.contains("renderGoalRoute"));
    assert!(html.contains("bestGoalRecommendation"));
    assert!(html.contains("fallbackGoalTextHints"));
    assert!(html.contains("goalRouteToolArguments"));
    assert!(html.contains("goalDryRunToolName"));
    assert!(html.contains("latestGoalActivity"));
    assert!(html.contains("routeGoal"));
    assert!(html.contains("openGoalWorkflow"));
    assert!(html.contains("openGoalPrompt"));
    assert!(html.contains("openGoalOnboarding"));
    assert!(html.contains("openGoalDoctor"));
    assert!(html.contains("previewGoalAction"));
    assert!(html.contains("applyGoalAction"));
    assert!(html.contains("shareGoalRemediation"));
    assert!(html.contains("shareGoalReplay"));
    assert!(html.contains("shareGoalRoute"));
    assert!(html.contains("latestWorkflowActivity"));
    assert!(html.contains("previewWorkflowAction"));
    assert!(html.contains("applyWorkflowAction"));
    assert!(html.contains("shareWorkflowReplay"));
    assert!(html.contains("Activity Log"));
    assert!(html.contains("activity-id-input"));
    assert!(html.contains("List activity"));
    assert!(html.contains("Open selected action"));
    assert!(html.contains("Undo selected action"));
    assert!(html.contains("Share replay"));
    assert!(html.contains("resource://yacli/activity"));
    assert!(html.contains("resource://yacli/activity/"));
    assert!(html.contains("renderActivityCatalog"));
    assert!(html.contains("renderActivity"));
    assert!(html.contains("activityCatalogItems"));
    assert!(html.contains("activityDetailPayload"));
    assert!(html.contains("activitySupportsUndo"));
    assert!(html.contains("latestUndoableActivity"));
    assert!(html.contains("undoActivityById"));
    assert!(html.contains("undoSelectedActivity"));
    assert!(html.contains("undoLatestHomeActivity"));
    assert!(html.contains("Searchable browser"));
    assert!(html.contains("browser-query-input"));
    assert!(html.contains("browser-filter-select"));
    assert!(html.contains("browser-results"));
    assert!(html.contains("Refresh browser"));
    assert!(html.contains("notifications/prompts/list_changed"));
    assert!(html.contains("notifications/tools/list_changed"));
    assert!(html.contains("tools/list"));
    assert!(html.contains("Prompt browser"));
    assert!(html.contains("List prompts"));
    assert!(html.contains("Render selected prompt"));
    assert!(html.contains("prompt-name-input"));
    assert!(html.contains("prompt-args-input"));
    assert!(html.contains("prompt-list-json"));
    assert!(html.contains("prompt-json"));
    assert!(html.contains("Share snapshot"));
    assert!(html.contains("Share current view"));
    assert!(html.contains("Current yacli dashboard view"));
    assert!(html.contains("buildCurrentViewUri"));
    assert!(html.contains("APP_VIEW_STATE_STORAGE_KEY"));
    assert!(html.contains("APP_RUNTIME_STATE_STORAGE_KEY"));
    assert!(html.contains("window.localStorage"));
    assert!(html.contains("toolSupportsDryRun"));
    assert!(html.contains("previewSelectedTool"));
    assert!(html.contains("applyReviewedTool"));
    assert!(html.contains("reviewedToolName"));
    assert!(html.contains("reviewPayload"));
    assert!(html.contains("Unified Home"));
    assert!(html.contains("panel-home"));
    assert!(html.contains("action-home-refresh"));
    assert!(html.contains("action-home-refresh-onboarding"));
    assert!(html.contains("action-home-share-onboarding"));
    assert!(html.contains("action-home-refresh-doctor"));
    assert!(html.contains("action-home-share-doctor"));
    assert!(html.contains("action-home-apply-safe-fixes"));
    assert!(html.contains("action-home-refresh-next-actions"));
    assert!(html.contains("action-home-share-next-actions"));
    assert!(html.contains("action-home-refresh-suggestions"));
    assert!(html.contains("action-home-share-suggestions"));
    assert!(html.contains("action-home-open-suggestion"));
    assert!(html.contains("action-home-apply-suggestion"));
    assert!(html.contains("action-home-undo-latest"));
    assert!(html.contains("action-home-open-workflows"));
    assert!(html.contains("action-home-open-activity"));
    assert!(html.contains("action-home-open-account"));
    assert!(html.contains("renderHomeSummary"));
    assert!(html.contains("renderOnboarding"));
    assert!(html.contains("renderDoctor"));
    assert!(html.contains("renderSuggestions"));
    assert!(html.contains("topSuggestion"));
    assert!(html.contains("openTopSuggestion"));
    assert!(html.contains("applyTopSuggestion"));
    assert!(html.contains("workflowExecutionPayload"));
    assert!(html.contains("refreshOnboardingResource"));
    assert!(html.contains("shareOnboarding"));
    assert!(html.contains("resource://yacli/suggestions"));
    assert!(html.contains("resource://yacli/onboarding"));
    assert!(html.contains("refreshDoctorResource"));
    assert!(html.contains("shareDoctor"));
    assert!(html.contains("applySafeDoctorFixes"));
    assert!(html.contains("resource://yacli/doctor"));
    assert!(html.contains("resource://yacli/next-actions"));
    assert!(html.contains("refreshHomeResource"));
    assert!(html.contains("refreshNextActionsResource"));
    assert!(html.contains("resource://yacli/home"));
    assert!(html.contains("homePayload"));
    assert!(html.contains("nextActionsPayload"));
    assert!(html.contains("refreshHome"));
    assert!(html.contains("openHomeWorkflowHub"));
    assert!(html.contains("openHomeActivityLog"));
    assert!(html.contains("openHomeAccountResource"));
    assert!(html.contains("mergeBootstrapState"));
    assert!(html.contains("savePersistedViewState"));
    assert!(html.contains("savePersistedRuntimeState"));
    assert!(html.contains("current-view-uri"));
    assert!(html.contains("Open MCP Apps docs"));
    assert!(html.contains("List accounts"));
    assert!(html.contains("Check updates"));
}

#[test]
fn mcp_stdio_reads_account_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://yacli/dashboard?account=work&section=auth&resource=auth&tool=yacli.auth.status"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"resourceUri\":\"ui://yacli/dashboard?account=work&section=auth&resource=auth&tool=yacli.auth.status\""));
    assert!(html.contains("\"defaultAccount\":\"work\""));
    assert!(html.contains("\"preferredSection\":\"auth\""));
    assert!(html.contains("\"preferredResource\":\"auth\""));
    assert!(html.contains("\"preferredTool\":\"yacli.auth.status\""));
    assert!(html.contains("dashboard opened for account"));
    assert!(html.contains("\"section restored\""));
    assert!(html.contains("state.bootstrap.preferredSection"));
    assert!(html.contains("restoreBootstrapFocus"));
    assert!(html.contains("loadPersistedViewState"));
    assert!(html.contains("restorePersistedRuntimeState"));
    assert!(html.contains("\"runtime restored\""));
    assert!(html.contains("\"View: \" + uri"));
    assert!(html.contains("panel-auth"));
}

#[test]
fn mcp_stdio_reads_skill_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://yacli/dashboard?section=resources&resource=skill&skill=yacli-mail"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredResource\":\"skill\""));
    assert!(html.contains("\"preferredSkill\":\"yacli-mail\""));
    assert!(html.contains("resource://yacli/skill/"));
    assert!(html.contains("readSkillResource"));
}

#[test]
fn mcp_stdio_reads_prompt_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://yacli/dashboard?section=prompts&prompt=mail"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredSection\":\"prompts\""));
    assert!(html.contains("\"preferredPrompt\":\"mail\""));
    assert!(html.contains("renderSelectedPrompt"));
    assert!(html.contains("prompts/get"));
}

#[test]
fn mcp_stdio_reads_workflow_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://yacli/dashboard?section=workflows&workflow=invite-to-calendar"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredSection\":\"workflows\""));
    assert!(html.contains("\"preferredWorkflow\":\"invite-to-calendar\""));
    assert!(html.contains("workflow-name-input"));
    assert!(html.contains("readWorkflowResource"));
    assert!(html.contains("openWorkflowPrompt"));
    assert!(html.contains("openWorkflowSkill"));
    assert!(html.contains("openWorkflowRunner"));
    assert!(html.contains("panel-workflows"));
}

#[test]
fn mcp_stdio_reads_goal_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://yacli/dashboard?section=goal&goal=%D0%BD%D0%B0%D0%B9%D0%B4%D0%B8%20%D0%BF%D1%80%D0%B8%D0%B3%D0%BB%D0%B0%D1%88%D0%B5%D0%BD%D0%B8%D0%B5"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredSection\":\"goal\""));
    assert!(html.contains("\"preferredGoal\":\"найди приглашение\""));
    assert!(html.contains("goal-input"));
    assert!(html.contains("routeGoal"));
    assert!(html.contains("previewGoalAction"));
}

#[test]
fn mcp_stdio_reads_activity_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://yacli/dashboard?section=activity&activity=act_20260314T104600Z_demo1234"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredSection\":\"activity\""));
    assert!(html.contains("\"preferredActivity\":\"act_20260314T104600Z_demo1234\""));
    assert!(html.contains("activity-id-input"));
    assert!(html.contains("readActivityResource"));
    assert!(html.contains("action-undo-activity"));
    assert!(html.contains("undoSelectedActivity"));
    assert!(html.contains("shareActivityReplay"));
    assert!(html.contains("panel-activity"));
}

#[test]
fn mcp_stdio_reads_home_aware_dashboard_resource() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "resources/read",
            json!({
                "uri": "ui://yacli/dashboard?section=home&goal=%D0%BE%D1%82%D0%BF%D1%80%D0%B0%D0%B2%D1%8C%20%D1%84%D0%B0%D0%B9%D0%BB"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let contents = responses[1]["result"]["contents"]
        .as_array()
        .expect("contents array");
    let html = contents[0]["text"].as_str().expect("html");
    assert!(html.contains("\"preferredSection\":\"home\""));
    assert!(html.contains("\"preferredGoal\":\"отправь файл\""));
    assert!(html.contains("panel-home"));
    assert!(html.contains("home-summary-json"));
    assert!(html.contains("refreshHome"));
}

#[test]
fn mcp_stdio_goal_route_tool_matches_invite_workflow_for_russian_goal() {
    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.goal.route",
                "arguments": {
                    "goal": "найди приглашение в письме и добавь встречу в календарь team"
                }
            }),
        ),
    ]
    .join("");

    let output = yacli()
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let response = &responses[1]["result"]["structuredContent"];
    assert_eq!(response["status"], "matched");
    assert_eq!(
        response["best_match"]["workflow"]["id"],
        "invite-to-calendar"
    );
    assert_eq!(response["hints"]["calendar"], "team");
    assert_eq!(
        response["best_match"]["route"]["tool_arguments"]["calendar"],
        "team"
    );
    assert!(response["remediation"].is_object());
}

#[test]
fn mcp_stdio_doctor_apply_safe_tool_installs_detected_claude_desktop() {
    let config_dir = tempdir().expect("config tempdir");
    let home_dir = tempdir().expect("home tempdir");
    let expected_config = if cfg!(target_os = "macos") {
        home_dir
            .path()
            .join("Library/Application Support/Claude/claude_desktop_config.json")
    } else if cfg!(target_os = "windows") {
        home_dir
            .path()
            .join("AppData/Roaming/Claude/claude_desktop_config.json")
    } else {
        home_dir
            .path()
            .join(".config/Claude/claude_desktop_config.json")
    };
    fs::create_dir_all(expected_config.parent().expect("parent")).expect("create parent");

    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.doctor.apply_safe",
                "arguments": {}
            }),
        ),
        mcp_request(
            3,
            "resources/read",
            json!({
                "uri": "resource://yacli/activity"
            }),
        ),
    ]
    .join("");

    let output = yacli()
        .env("YACLI_CONFIG_DIR", config_dir.path())
        .env("HOME", home_dir.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let response = &responses[1]["result"]["structuredContent"];
    assert_eq!(response["status"], "partial");
    assert!(
        response["doctor_after"]["mcp_clients"]
            .as_array()
            .expect("mcp clients")
            .iter()
            .any(|item| item["client"] == "claude-desktop" && item["status"] == "installed")
    );
    let activity_contents = responses[2]["result"]["contents"]
        .as_array()
        .expect("activity contents");
    let activity_payload: Value = serde_json::from_str(
        activity_contents[0]["text"]
            .as_str()
            .expect("activity text"),
    )
    .expect("activity json");
    assert_eq!(
        activity_payload["items"][0]["operation"],
        "doctor.apply_safe"
    );
    assert_eq!(activity_payload["items"][0]["source"], "mcp");
    assert_eq!(
        activity_payload["items"][0]["replay_command"],
        "yacli doctor --apply-safe"
    );
    assert!(expected_config.exists());
}

#[test]
fn mcp_stdio_update_check_tool_reads_release_mirror() {
    let mut server = Server::new();
    let base_url = format!("{}/releases/download/v9.9.9", server.url());
    let asset = current_release_update_target();
    let _checksums = server
        .mock("GET", "/releases/download/v9.9.9/SHA256SUMS")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body(format!("deadbeef  {asset}\n"))
        .create();

    let input = [
        initialize_request(false),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.update.check",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_UPDATE_BASE_URL", &base_url)
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let structured = &responses[1]["result"]["structuredContent"];
    assert_eq!(structured["operation"], "update.check");
    assert_eq!(structured["status"], "update_available");
    assert_eq!(structured["targetVersion"], "9.9.9");
    assert_eq!(structured["requestedVersion"], "latest");
    assert_eq!(structured["baseUrl"], base_url);
}

#[test]
fn mcp_stdio_lists_resource_templates_and_reads_templated_resources() {
    let temp = tempdir().expect("tempdir");
    let home = tempdir().expect("home tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
credential_ref = "env:YACLI_MAIL_TOKEN"

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );

    let input = [
        initialize_request(false),
        mcp_request(2, "resources/templates/list", json!({})),
        mcp_request(
            3,
            "resources/read",
            json!({
                "uri": "resource://yacli/account/personal"
            }),
        ),
        mcp_request(
            4,
            "resources/read",
            json!({
                "uri": "resource://yacli/auth/personal"
            }),
        ),
        mcp_request(
            5,
            "resources/read",
            json!({
                "uri": "resource://yacli/skills"
            }),
        ),
        mcp_request(
            6,
            "resources/read",
            json!({
                "uri": "resource://yacli/skill/yacli-mail"
            }),
        ),
        mcp_request(
            7,
            "resources/read",
            json!({
                "uri": "resource://yacli/workflows"
            }),
        ),
        mcp_request(
            8,
            "resources/read",
            json!({
                "uri": "resource://yacli/workflow/invite-to-calendar"
            }),
        ),
        mcp_request(
            9,
            "resources/read",
            json!({
                "uri": "resource://yacli/activity"
            }),
        ),
        mcp_request(
            10,
            "resources/read",
            json!({
                "uri": "resource://yacli/onboarding"
            }),
        ),
        mcp_request(
            11,
            "resources/read",
            json!({
                "uri": "resource://yacli/doctor"
            }),
        ),
        mcp_request(
            12,
            "resources/read",
            json!({
                "uri": "resource://yacli/home"
            }),
        ),
        mcp_request(
            13,
            "resources/read",
            json!({
                "uri": "resource://yacli/home/personal"
            }),
        ),
        mcp_request(
            14,
            "resources/read",
            json!({
                "uri": "resource://yacli/next-actions"
            }),
        ),
        mcp_request(
            15,
            "resources/read",
            json!({
                "uri": "resource://yacli/home/personal?goal=%D0%BE%D1%82%D0%BF%D1%80%D0%B0%D0%B2%D1%8C%20%D1%84%D0%B0%D0%B9%D0%BB%20%D0%BF%D0%BE%20%D0%BF%D0%BE%D1%87%D1%82%D0%B5"
            }),
        ),
        mcp_request(
            16,
            "resources/read",
            json!({
                "uri": "resource://yacli/next-actions?goal=%D0%BD%D0%B0%D0%B9%D0%B4%D0%B8%20%D0%BF%D1%80%D0%B8%D0%B3%D0%BB%D0%B0%D1%88%D0%B5%D0%BD%D0%B8%D0%B5"
            }),
        ),
        mcp_request(
            17,
            "resources/read",
            json!({
                "uri": "resource://yacli/suggestions"
            }),
        ),
        mcp_request(
            18,
            "resources/read",
            json!({
                "uri": "resource://yacli/onboarding?goal=%D0%BD%D0%B0%D0%B9%D0%B4%D0%B8%20%D0%BF%D1%80%D0%B8%D0%B3%D0%BB%D0%B0%D1%88%D0%B5%D0%BD%D0%B8%D0%B5"
            }),
        ),
        mcp_request(
            19,
            "resources/read",
            json!({
                "uri": "resource://yacli/doctor?goal=%D0%BD%D0%B0%D0%B9%D0%B4%D0%B8%20%D0%BF%D1%80%D0%B8%D0%B3%D0%BB%D0%B0%D1%88%D0%B5%D0%BD%D0%B8%D0%B5"
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("HOME", home.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let templates = responses[1]["result"]["resourceTemplates"]
        .as_array()
        .expect("resource templates array");
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/account/{account}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/auth/{account}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/skill/{skill}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/workflow/{workflow}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/activity/{activity}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/home/{account}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/home{?goal}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/home/{account}{?goal}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/next-actions{?goal}")
    );
    assert!(templates.iter().any(
        |template| template["uriTemplate"] == "resource://yacli/next-actions/{account}{?goal}"
    ));
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/suggestions{?goal}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"]
                == "resource://yacli/suggestions/{account}{?goal}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/onboarding{?goal}")
    );
    assert!(
        templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/doctor{?goal}")
    );
    assert!(
        !templates
            .iter()
            .any(|template| template["uriTemplate"] == APP_RESOURCE_URI_TEMPLATE)
    );

    let account_contents = responses[2]["result"]["contents"]
        .as_array()
        .expect("account contents");
    assert_eq!(account_contents[0]["mimeType"], "application/json");
    let account_payload: Value =
        serde_json::from_str(account_contents[0]["text"].as_str().expect("account text"))
            .expect("account json");
    assert_eq!(account_payload["account"], "personal");
    assert_eq!(account_payload["current"], true);
    assert_eq!(
        account_payload["services"]["mail"]["credentialRef"],
        "env:YACLI_MAIL_TOKEN"
    );

    let auth_contents = responses[3]["result"]["contents"]
        .as_array()
        .expect("auth contents");
    assert_eq!(auth_contents[0]["mimeType"], "application/json");
    let auth_payload: Value =
        serde_json::from_str(auth_contents[0]["text"].as_str().expect("auth text"))
            .expect("auth json");
    assert_eq!(auth_payload["account"], "personal");
    assert_eq!(
        auth_payload["services"]["mail"]["credential_state"],
        "env_missing"
    );

    let skills_catalog_contents = responses[4]["result"]["contents"]
        .as_array()
        .expect("skills catalog contents");
    let skills_catalog_payload: Value = serde_json::from_str(
        skills_catalog_contents[0]["text"]
            .as_str()
            .expect("skills catalog text"),
    )
    .expect("skills catalog json");
    assert_eq!(skills_catalog_payload["count"], 13);
    assert!(
        skills_catalog_payload["items"]
            .as_array()
            .expect("skill items")
            .iter()
            .any(|item| item["name"] == "yacli-mail")
    );

    let skill_contents = responses[5]["result"]["contents"]
        .as_array()
        .expect("skill contents");
    assert_eq!(skill_contents[0]["mimeType"], "text/markdown");
    let skill_text = skill_contents[0]["text"].as_str().expect("skill text");
    assert!(skill_text.contains("# yacli mail"));
    assert!(skill_text.contains("yacli mail send"));

    let workflow_catalog_contents = responses[6]["result"]["contents"]
        .as_array()
        .expect("workflow catalog contents");
    let workflow_catalog_payload: Value = serde_json::from_str(
        workflow_catalog_contents[0]["text"]
            .as_str()
            .expect("workflow catalog text"),
    )
    .expect("workflow catalog json");
    assert!(
        workflow_catalog_payload["workflows"]
            .as_array()
            .expect("workflow items")
            .iter()
            .any(|item| item["id"] == "daily-briefing")
    );

    let workflow_contents = responses[7]["result"]["contents"]
        .as_array()
        .expect("workflow detail contents");
    let workflow_payload: Value = serde_json::from_str(
        workflow_contents[0]["text"]
            .as_str()
            .expect("workflow detail text"),
    )
    .expect("workflow detail json");
    assert_eq!(workflow_payload["id"], "invite-to-calendar");
    assert_eq!(workflow_payload["prompt_name"], "invite-to-calendar");
    assert_eq!(workflow_payload["skill_name"], "yacli-invite-to-calendar");
    assert_eq!(
        workflow_payload["primary_tool"],
        "yacli.mail.invite.create_event"
    );
    assert_eq!(
        workflow_payload["primary_operation"],
        "mail.invite.create_event"
    );
    assert_eq!(workflow_payload["supports_review"], false);
    assert_eq!(
        workflow_payload["primary_tool_arguments"]["folder"],
        "INBOX"
    );
    assert_eq!(
        workflow_payload["primary_tool_arguments"]["calendar"],
        "team"
    );

    let activity_catalog_contents = responses[8]["result"]["contents"]
        .as_array()
        .expect("activity catalog contents");
    let activity_catalog_payload: Value = serde_json::from_str(
        activity_catalog_contents[0]["text"]
            .as_str()
            .expect("activity catalog text"),
    )
    .expect("activity catalog json");
    assert_eq!(activity_catalog_payload["count"], 0);
    assert_eq!(
        activity_catalog_payload["items"]
            .as_array()
            .expect("activity items")
            .len(),
        0
    );

    let onboarding_contents = responses[9]["result"]["contents"]
        .as_array()
        .expect("onboarding contents");
    let onboarding_payload: Value = serde_json::from_str(
        onboarding_contents[0]["text"]
            .as_str()
            .expect("onboarding text"),
    )
    .expect("onboarding json");
    assert_eq!(onboarding_payload["current_account"], "personal");
    assert!(
        onboarding_payload["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .any(|item| item["id"] == "account")
    );
    assert!(
        onboarding_payload["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .any(|item| item["id"] == "mcp_install")
    );

    let doctor_contents = responses[10]["result"]["contents"]
        .as_array()
        .expect("doctor contents");
    let doctor_payload: Value =
        serde_json::from_str(doctor_contents[0]["text"].as_str().expect("doctor text"))
            .expect("doctor json");
    assert_eq!(doctor_payload["current_account"], "personal");
    assert!(
        doctor_payload["checks"]
            .as_array()
            .expect("doctor checks")
            .iter()
            .any(|item| item["id"] == "secret_backend")
    );
    assert!(
        doctor_payload["checks"]
            .as_array()
            .expect("doctor checks")
            .iter()
            .any(|item| item["id"] == "mcp_install")
    );

    let home_contents = responses[11]["result"]["contents"]
        .as_array()
        .expect("home contents");
    let home_payload: Value =
        serde_json::from_str(home_contents[0]["text"].as_str().expect("home text"))
            .expect("home json");
    assert_eq!(home_payload["current_account"], "personal");
    assert_eq!(home_payload["workflow_count"], 8);
    assert_eq!(home_payload["onboarding"]["current_account"], "personal");
    assert_eq!(home_payload["doctor"]["current_account"], "personal");
    assert_eq!(home_payload["suggestions"]["count"], 0);

    let templated_home_contents = responses[12]["result"]["contents"]
        .as_array()
        .expect("templated home contents");
    let templated_home_payload: Value = serde_json::from_str(
        templated_home_contents[0]["text"]
            .as_str()
            .expect("templated home text"),
    )
    .expect("templated home json");
    assert_eq!(templated_home_payload["current_account"], "personal");
    assert_eq!(
        templated_home_payload["doctor"]["current_account"],
        "personal"
    );

    let next_actions_contents = responses[13]["result"]["contents"]
        .as_array()
        .expect("next actions contents");
    let next_actions_payload: Value = serde_json::from_str(
        next_actions_contents[0]["text"]
            .as_str()
            .expect("next actions text"),
    )
    .expect("next actions json");
    assert!(
        next_actions_payload["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .any(|item| item["id"] == "mcp_install")
    );

    let goal_home_contents = responses[14]["result"]["contents"]
        .as_array()
        .expect("goal home contents");
    let goal_home_payload: Value = serde_json::from_str(
        goal_home_contents[0]["text"]
            .as_str()
            .expect("goal home text"),
    )
    .expect("goal home json");
    assert_eq!(goal_home_payload["goal"], "отправь файл по почте");
    assert_eq!(
        goal_home_payload["goal_route"]["best_match"]["workflow"]["id"],
        "send-file-by-mail"
    );

    let goal_next_actions_contents = responses[15]["result"]["contents"]
        .as_array()
        .expect("goal next actions contents");
    let goal_next_actions_payload: Value = serde_json::from_str(
        goal_next_actions_contents[0]["text"]
            .as_str()
            .expect("goal next actions text"),
    )
    .expect("goal next actions json");
    assert_eq!(goal_next_actions_payload["goal"], "найди приглашение");
    assert_eq!(
        goal_next_actions_payload["goal_route"]["best_match"]["workflow"]["id"],
        "invite-to-calendar"
    );
    assert_eq!(goal_next_actions_payload["actions"][0]["source"], "goal");

    let suggestions_contents = responses[16]["result"]["contents"]
        .as_array()
        .expect("suggestions contents");
    let suggestions_payload: Value = serde_json::from_str(
        suggestions_contents[0]["text"]
            .as_str()
            .expect("suggestions text"),
    )
    .expect("suggestions json");
    assert_eq!(suggestions_payload["status"], "idle");
    assert_eq!(suggestions_payload["count"], 0);

    let workflow_response = responses
        .iter()
        .find(|response| response["id"] == 8)
        .expect("workflow response");
    let workflow_contents = workflow_response["result"]["contents"]
        .as_array()
        .expect("workflow contents");
    let workflow_payload: Value = serde_json::from_str(
        workflow_contents[0]["text"]
            .as_str()
            .expect("workflow text"),
    )
    .expect("workflow json");
    assert_eq!(workflow_payload["id"], "invite-to-calendar");
    assert_eq!(workflow_payload["execution"]["state"], "needs_input");
    assert_eq!(
        workflow_payload["execution"]["next_action"],
        "connect_services"
    );
    assert_eq!(
        workflow_payload["execution"]["available_actions"][0],
        "connect_services"
    );

    let goal_onboarding_contents = responses[17]["result"]["contents"]
        .as_array()
        .expect("goal onboarding contents");
    let goal_onboarding_payload: Value = serde_json::from_str(
        goal_onboarding_contents[0]["text"]
            .as_str()
            .expect("goal onboarding text"),
    )
    .expect("goal onboarding json");
    assert_eq!(goal_onboarding_payload["goal"], "найди приглашение");
    assert_eq!(
        goal_onboarding_payload["goal_route"]["best_match"]["workflow"]["id"],
        "invite-to-calendar"
    );

    let goal_doctor_contents = responses[18]["result"]["contents"]
        .as_array()
        .expect("goal doctor contents");
    let goal_doctor_payload: Value = serde_json::from_str(
        goal_doctor_contents[0]["text"]
            .as_str()
            .expect("goal doctor text"),
    )
    .expect("goal doctor json");
    assert_eq!(goal_doctor_payload["goal"], "найди приглашение");
    assert_eq!(
        goal_doctor_payload["goal_route"]["remediation"]["status"],
        "needs_setup"
    );
    assert!(
        goal_doctor_payload["focus_checks"]
            .as_array()
            .expect("goal doctor focus checks")
            .iter()
            .any(|item| item["id"] == "mail" || item["id"] == "calendar")
    );
}

#[test]
fn mcp_stdio_app_snapshot_exposes_auth_discovery_when_http_auth_is_configured() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );

    let input = [
        initialize_request(true),
        mcp_request(
            2,
            "tools/call",
            json!({
                "name": "yacli.app.snapshot",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token")
        .env("YACLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test")
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let discovery = &responses[1]["result"]["structuredContent"]["authDiscovery"];
    assert_eq!(discovery["enabled"], true);
    assert_eq!(
        discovery["authorizationServers"][0],
        "https://auth.example.test"
    );
    assert_eq!(
        discovery["resourceMetadataUrl"],
        "http://127.0.0.1:8787/.well-known/oauth-protected-resource/mcp"
    );
}

#[test]
fn mcp_stdio_text_only_clients_receive_graceful_degradation() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );

    let input = [
        initialize_request(false),
        mcp_request(2, "tools/list", json!({})),
        mcp_request(3, "resources/list", json!({})),
        mcp_request(
            4,
            "tools/call",
            json!({
                "name": "yacli.account.list",
                "arguments": {}
            }),
        ),
    ]
    .concat();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mcp"])
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = parse_responses(&output);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let account_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.account.current")
        .expect("account tool");
    assert!(account_tool.get("_meta").is_none());
    assert!(
        !tools
            .iter()
            .any(|tool| tool["name"] == "yacli.app.snapshot")
    );

    let resources = responses[2]["result"]["resources"]
        .as_array()
        .expect("resources array");
    assert!(
        !resources
            .iter()
            .any(|resource| resource["uri"] == APP_RESOURCE_URI)
    );

    assert_eq!(
        responses[3]["result"]["structuredContent"]["items"][0]["name"],
        "personal"
    );
    assert!(responses[3]["result"].get("_meta").is_none());
}

#[test]
fn mcp_stdio_subscriptions_emit_resource_updated_notifications() {
    let temp = tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("yacli"))
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_SECRET_BACKEND", "file")
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn yacli mcp");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    stdin
        .write_all(initialize_request(false).as_bytes())
        .expect("initialize write");
    let initialize = read_response(&mut reader);
    assert_eq!(
        initialize["result"]["capabilities"]["resources"]["subscribe"],
        true
    );

    stdin
        .write_all(
            mcp_request(
                2,
                "resources/subscribe",
                json!({
                    "uri": "resource://yacli/account/personal"
                }),
            )
            .as_bytes(),
        )
        .expect("subscribe write");
    let subscribe = read_response(&mut reader);
    assert_eq!(subscribe["result"], json!({}));

    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "updated@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );

    let notification = read_response(&mut reader);
    assert_eq!(notification["method"], "notifications/resources/updated");
    assert_eq!(
        notification["params"]["uri"],
        "resource://yacli/account/personal"
    );

    let _ = child.kill();
    let _ = child.wait();
}
