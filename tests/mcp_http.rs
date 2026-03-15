use assert_cmd::cargo::cargo_bin;
use base64::Engine;
use mockito::{Matcher, Server};
use reqwest::Method;
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

struct TestHttpServer {
    child: Child,
    addr: String,
}

impl TestHttpServer {
    fn spawn() -> Self {
        Self::spawn_with_args_envs(&[], &[])
    }

    fn spawn_with_envs(envs: &[(&str, &str)]) -> Self {
        Self::spawn_with_args_envs(&[], envs)
    }

    fn spawn_with_args_envs(args: &[&str], envs: &[(&str, &str)]) -> Self {
        let mut last_stderr = String::new();
        for _attempt in 0..10 {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
            let addr = listener.local_addr().expect("local addr").to_string();
            drop(listener);

            let mut command = Command::new(cargo_bin("yacli"));
            command
                .args(["mcp", "--transport", "http", "--listen", &addr])
                .env("YACLI_SECRET_BACKEND", "file")
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            for (key, value) in envs {
                command.env(key, value);
            }
            let mut child = command.spawn().expect("spawn yacli mcp http");

            for _ in 0..800 {
                if let Some(status) = child.try_wait().expect("poll child status") {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    if stderr.contains("Address already in use") {
                        break;
                    }
                    panic!("HTTP MCP server exited early with {status}: {stderr}");
                }
                if http_transport_ready(&addr) {
                    return Self { child, addr };
                }
                thread::sleep(Duration::from_millis(50));
            }

            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let _ = child.kill();
            let _ = child.wait();
            last_stderr = stderr.clone();
            if !stderr.contains("Address already in use") && !stderr.is_empty() {
                panic!("HTTP MCP server did not start at {addr}. stderr: {stderr}");
            }
        }

        panic!("HTTP MCP server did not become ready after retries. last stderr: {last_stderr}");
    }

    fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http_transport_ready(addr: &str) -> bool {
    let client = Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .expect("http readiness client");
    client
        .request(Method::OPTIONS, format!("http://{addr}/mcp"))
        .send()
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client")
}

fn post_json(
    client: &Client,
    url: &str,
    body: Value,
    session_id: Option<&str>,
    origin: Option<&str>,
) -> Response {
    let mut request = client.post(url).json(&body);
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    request.send().expect("http response")
}

fn post_json_sse(client: &Client, url: &str, body: Value, session_id: Option<&str>) -> Response {
    let mut request = client
        .post(url)
        .header("Accept", "text/event-stream")
        .json(&body);
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }
    request.send().expect("http sse response")
}

fn open_sse(client: &Client, url: &str, session_id: &str) -> Response {
    client
        .get(url)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", session_id)
        .send()
        .expect("sse response")
}

fn read_sse_json_message(reader: &mut BufReader<Response>) -> Value {
    let mut payload = String::new();

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).expect("sse line");
        assert!(bytes > 0, "sse stream closed unexpectedly");
        if line == "\n" || line == "\r\n" {
            if !payload.is_empty() {
                return serde_json::from_str(&payload).expect("sse json payload");
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data: ") {
            payload.push_str(data.trim_end());
        }
    }
}

fn write_accounts_file(config_dir: &std::path::Path, content: &str) {
    std::fs::create_dir_all(config_dir).expect("config dir");
    std::fs::write(config_dir.join("accounts.toml"), content).expect("accounts file");
}

fn write_credentials_file(config_dir: &std::path::Path, content: &str) {
    std::fs::create_dir_all(config_dir).expect("config dir");
    std::fs::write(config_dir.join("credentials.toml"), content).expect("credentials file");
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
fn mcp_http_initialize_returns_session_header_and_supports_follow_up_requests() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );

    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();
    let payload: Value = initialize.json().expect("initialize json");
    assert_eq!(payload["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(payload["result"]["capabilities"]["completions"], json!({}));
    assert_eq!(
        payload["result"]["capabilities"]["prompts"]["listChanged"],
        false
    );
    assert_eq!(
        payload["result"]["capabilities"]["resources"]["subscribe"],
        true
    );

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    assert!(tools_list.status().is_success());
    let payload: Value = tools_list.json().expect("tools/list json");
    let tools = payload["result"]["tools"].as_array().expect("tools array");
    let account_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.account.current")
        .expect("account tool");
    assert_eq!(
        account_tool["_meta"]["ui"]["resourceUri"],
        "ui://yacli/dashboard"
    );
    let update_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.update.check")
        .expect("update tool");
    assert_eq!(
        update_tool["_meta"]["ui"]["resourceUri"],
        "ui://yacli/dashboard"
    );

    let prompts_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "prompts/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    assert!(prompts_list.status().is_success());
    let prompt_payload: Value = prompts_list.json().expect("prompts/list json");
    let prompts = prompt_payload["result"]["prompts"]
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
fn mcp_http_renders_prompt_messages() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let prompt = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "prompts/get",
            "params": {
                "name": "reply-with-context",
                "arguments": {
                    "uid": "42",
                    "account": "work"
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(prompt.status().is_success());
    let payload: Value = prompt.json().expect("prompts/get json");
    let text = payload["result"]["messages"][0]["content"]["text"]
        .as_str()
        .expect("prompt text");
    assert!(text.contains("UID письма: 42"));
    assert!(text.contains("Аккаунт: work"));
    assert!(text.contains("используй `yacli.mail.reply`"));
}

#[test]
fn mcp_http_completes_prompt_and_resource_arguments() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_mock_mail_account(temp.path());

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let prompt_completion = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "completion/complete",
            "params": {
                "ref": {
                    "type": "ref/prompt",
                    "name": "mail"
                },
                "argument": {
                    "name": "folder",
                    "value": "in"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let prompt_payload: Value = prompt_completion.json().expect("completion json");
    assert_eq!(prompt_payload["result"]["completion"]["values"][0], "INBOX");

    let resource_completion = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "completion/complete",
            "params": {
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://yacli/account/{account}"
                },
                "argument": {
                    "name": "account",
                    "value": "mo"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let resource_payload: Value = resource_completion.json().expect("completion json");
    assert_eq!(
        resource_payload["result"]["completion"]["values"][0],
        "mock"
    );

    let skill_completion = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "completion/complete",
            "params": {
                "ref": {
                    "type": "ref/resource",
                    "uri": "resource://yacli/skill/{skill}"
                },
                "argument": {
                    "name": "skill",
                    "value": "yacli-ma"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let skill_payload: Value = skill_completion.json().expect("completion json");
    assert_eq!(
        skill_payload["result"]["completion"]["values"][0],
        "yacli-mail"
    );
}

#[test]
fn mcp_http_requests_client_roots_via_post_sse() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "roots": {
                        "listChanged": true
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    assert!(tools_list.status().is_success());
    let payload: Value = tools_list.json().expect("tools/list json");
    let tools = payload["result"]["tools"].as_array().expect("tools array");
    assert!(tools.iter().any(|tool| tool["name"] == "yacli.roots.list"));

    let sse = post_json_sse(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.roots.list",
                "arguments": {}
            }
        }),
        Some(&session_id),
    );
    assert!(sse.status().is_success());
    let mut reader = BufReader::new(sse);
    let roots_request = read_sse_json_message(&mut reader);
    assert_eq!(roots_request["method"], "roots/list");
    let roots_request_id = roots_request["id"].as_u64().expect("roots request id");

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": roots_request_id,
            "result": {
                "roots": [
                    {
                        "uri": "file:///tmp/http-project",
                        "name": "http-project"
                    }
                ]
            }
        }),
        Some(&session_id),
        None,
    );
    assert_eq!(response.status(), 202);

    let final_response = read_sse_json_message(&mut reader);
    assert_eq!(final_response["id"], 3);
    assert_eq!(
        final_response["result"]["structuredContent"]["roots"][0]["uri"],
        "file:///tmp/http-project"
    );
}

#[test]
fn mcp_http_roots_tool_requires_post_sse_accept_header() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "roots": {
                        "listChanged": true
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.roots.list",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    assert_eq!(response.status(), 406);
}

#[test]
fn mcp_http_exposes_mail_write_tools_and_validates_send_input() {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
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

    let send = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.send",
                "arguments": {
                    "account": "mock",
                    "to": "broken-recipient",
                    "subject": "Hello",
                    "text": "Body"
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(send.status().is_success());
    let payload: Value = send.json().expect("tools/call json");
    assert_eq!(payload["error"]["code"], -32602);
    assert!(
        payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("mail send recipient must contain `@`")
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
fn mcp_http_mail_send_dry_run_reviews_message_without_network() {
    let temp = tempfile::tempdir().expect("tempdir");
    let attachment_path = temp.path().join("invoice.txt");
    std::fs::write(&attachment_path, "invoice body").expect("attachment");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("config dir"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let send = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.send",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Hello",
                    "text": "Body",
                    "attachments": [attachment_path.display().to_string()],
                    "dry_run": true
                }
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(send.status().is_success());
    let payload: Value = send.json().expect("tools/call json");
    let structured = &payload["result"]["structuredContent"];
    assert_eq!(structured["dry_run"], true);
    assert_eq!(structured["review"]["attachment_count"], 1);
    assert_eq!(
        structured["review"]["attachments"][0]["filename"],
        "invoice.txt"
    );
    assert_eq!(structured["review"]["sent"]["body_kind"], "multipart_mixed");
}

#[test]
fn mcp_http_mail_send_link_dry_run_reviews_upload_publish_and_mail_without_network() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source_path = temp.path().join("archive.zip");
    std::fs::write(&source_path, "archive body").expect("source");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("config dir"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
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
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    let structured = &payload["result"]["structuredContent"];
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
fn mcp_http_mail_send_link_returns_partial_recovery_when_smtp_fails_after_publish() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut mock_server = Server::new();
    let source_path = temp.path().join("archive.zip");
    std::fs::write(&source_path, "archive body").expect("source");
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
            mock_server.url()
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

    let _ticket = mock_server
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
            mock_server.url()
        ))
        .create();
    let _upload = mock_server
        .mock("PUT", "/upload-target/archive.zip")
        .match_body(Matcher::Exact("archive body".to_string()))
        .with_status(201)
        .create();
    let _publish = mock_server
        .mock("PUT", "/v1/disk/resources/publish")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded(
            "path".into(),
            "disk:/docs/archive.zip".into(),
        ))
        .with_status(200)
        .create();
    let _metadata = mock_server
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("config dir"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.send_link",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Материалы",
                    "text": "Отправляю ссылку",
                    "source_path": source_path.display().to_string(),
                    "disk_path": "disk:/docs/archive.zip"
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    let structured = &payload["result"]["structuredContent"];
    assert_eq!(structured["status"], "partial");
    assert_eq!(
        structured["partial"]["recovery"]["share_public_link"]["public_url"],
        "https://disk.yandex.ru/i/archive-link"
    );
    assert_eq!(
        structured["partial"]["recovery"]["retry_mail_step"]["tool"],
        "yacli.mail.send_published_link"
    );

    let activity_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/activity"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(activity_resource.status().is_success());
    let activity_payload: Value = activity_resource.json().expect("activity resource");
    let activity_contents = activity_payload["result"]["contents"]
        .as_array()
        .expect("activity contents");
    let activity_json: Value = serde_json::from_str(
        activity_contents[0]["text"]
            .as_str()
            .expect("activity text"),
    )
    .expect("activity payload");
    assert_eq!(
        activity_json["items"][0]["operation"],
        "mail.send_link.partial"
    );
    assert_eq!(activity_json["items"][0]["source"], "mcp");
}

#[test]
fn mcp_http_mail_send_dry_run_recommends_send_link_for_oversized_attachment() {
    let temp = tempfile::tempdir().expect("tempdir");
    let attachment_path = temp.path().join("archive.zip");
    std::fs::write(&attachment_path, vec![b'x'; 20 * 1024 * 1024]).expect("attachment");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("config dir"),
    )]);
    let client = client();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let send = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.send",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Большой архив",
                    "text": "Материалы во вложении",
                    "attachments": [attachment_path.display().to_string()],
                    "dry_run": true
                }
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(send.status().is_success());
    let payload: Value = send.json().expect("tools/call json");
    let structured = &payload["result"]["structuredContent"];
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
fn mcp_http_mail_attachment_export_requires_selector_before_network() {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.attachment.export",
                "arguments": {
                    "account": "mock",
                    "uid": 42,
                    "output_path": temp.path().join("invoice.pdf").display().to_string()
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    assert_eq!(payload["error"]["code"], -32602);
    assert!(
        payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("yacli.mail.attachment.export requires `index` or `name`")
    );
}

#[test]
fn mcp_http_mail_invite_inspect_requires_selector_before_network() {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.invite.inspect",
                "arguments": {
                    "account": "mock",
                    "uid": 42
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    assert_eq!(payload["error"]["code"], -32602);
    assert!(
        payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("yacli.mail.invite.inspect requires `index` or `name`")
    );
}

#[test]
fn mcp_http_mail_invite_create_event_requires_selector_before_network() {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.invite.create_event",
                "arguments": {
                    "account": "mock",
                    "uid": 42
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    assert_eq!(payload["error"]["code"], -32602);
    assert!(
        payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("yacli.mail.invite.create_event requires `index` or `name`")
    );
}

#[test]
fn mcp_http_mail_send_rejects_directory_attachment_before_network() {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.mail.send",
                "arguments": {
                    "account": "mock",
                    "to": "person@example.com",
                    "subject": "Hello",
                    "text": "Body",
                    "attachments": [temp.path().display().to_string()]
                }
            }
        }),
        Some(&session_id),
        None,
    );

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    assert_eq!(payload["error"]["code"], -32601);
    assert!(
        payload["error"]["message"]
            .as_str()
            .expect("message")
            .contains("attachment path points to a directory")
    );
}

#[test]
fn mcp_http_exposes_calendar_write_tools_and_executes_create_delete() {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
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

    let create = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.calendar.create",
                "arguments": {
                    "account": "mock",
                    "summary": "Синк команды",
                    "start": "2026-03-12T09:00:00Z",
                    "end": "2026-03-12T10:00:00Z"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let create_payload: Value = create.json().expect("create json");
    assert_eq!(
        create_payload["result"]["structuredContent"]["calendar"]["id"],
        "default"
    );
    assert_eq!(
        create_payload["result"]["structuredContent"]["event"]["summary"],
        "Синк команды"
    );

    let delete = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "yacli.calendar.delete",
                "arguments": {
                    "account": "mock",
                    "uid": "event-1"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let delete_payload: Value = delete.json().expect("delete json");
    assert_eq!(
        delete_payload["result"]["structuredContent"]["deleted_event"]["uid"],
        "event-1"
    );
    assert_eq!(
        delete_payload["result"]["structuredContent"]["deleted_event"]["summary"],
        "Удаляемое событие"
    );
}

#[test]
fn mcp_http_calendar_create_dry_run_reviews_event_without_network_write() {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let create = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.calendar.create",
                "arguments": {
                    "account": "mock",
                    "summary": "Синк команды",
                    "start": "2026-03-12T09:00:00Z",
                    "end": "2026-03-12T10:00:00Z",
                    "location": "Meet",
                    "dry_run": true
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let create_payload: Value = create.json().expect("create json");
    assert_eq!(
        create_payload["result"]["structuredContent"]["calendar"]["id"],
        "default"
    );
    assert_eq!(
        create_payload["result"]["structuredContent"]["dry_run"],
        true
    );
    assert_eq!(
        create_payload["result"]["structuredContent"]["review"]["summary"],
        "Синк команды"
    );
    assert_eq!(
        create_payload["result"]["structuredContent"]["review"]["status"],
        "CONFIRMED"
    );
}

#[test]
fn mcp_http_exposes_disk_write_tools_and_executes_mkdir_upload() {
    let temp = tempfile::tempdir().expect("tempdir");
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
    std::fs::write(&source_path, b"hello disk").expect("source file");

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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
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

    let mkdir = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.mkdir",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/new-folder"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let mkdir_payload: Value = mkdir.json().expect("mkdir json");
    assert_eq!(
        mkdir_payload["result"]["structuredContent"]["resource"]["resource_type"],
        "dir"
    );

    let upload = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.upload",
                "arguments": {
                    "account": "mock",
                    "source": source_path.to_str().expect("utf8 path"),
                    "path": "disk:/docs/note.txt"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let upload_payload: Value = upload.json().expect("upload json");
    assert_eq!(
        upload_payload["result"]["structuredContent"]["resource"]["resource_type"],
        "file"
    );
    assert_eq!(
        upload_payload["result"]["structuredContent"]["upload"]["bytes_written"],
        10
    );

    let activity_catalog = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/activity"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(activity_catalog.status().is_success());
    let activity_catalog_payload: Value = activity_catalog.json().expect("activity catalog json");
    let activity_catalog_contents = activity_catalog_payload["result"]["contents"]
        .as_array()
        .expect("activity catalog contents");
    let activity_catalog_json: Value = serde_json::from_str(
        activity_catalog_contents[0]["text"]
            .as_str()
            .expect("activity catalog text"),
    )
    .expect("activity catalog payload");
    let activity_items = activity_catalog_json["items"]
        .as_array()
        .expect("activity items");
    assert_eq!(activity_items.len(), 2);
    assert_eq!(activity_items[0]["operation"], "disk.upload");
    assert_eq!(activity_items[0]["source"], "mcp");
    assert_eq!(activity_items[1]["operation"], "disk.mkdir");
    assert_eq!(activity_items[1]["source"], "mcp");

    let activity_id = activity_items[0]["id"].as_str().expect("activity id");
    let activity_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "resources/read",
            "params": {
                "uri": format!("resource://yacli/activity/{activity_id}")
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(activity_resource.status().is_success());
    let activity_payload: Value = activity_resource.json().expect("activity detail json");
    let activity_contents = activity_payload["result"]["contents"]
        .as_array()
        .expect("activity detail contents");
    let activity_json: Value = serde_json::from_str(
        activity_contents[0]["text"]
            .as_str()
            .expect("activity detail text"),
    )
    .expect("activity detail payload");
    assert_eq!(activity_json["id"], activity_id);
    assert_eq!(activity_json["operation"], "disk.upload");
    assert_eq!(activity_json["source"], "mcp");
    assert!(
        activity_json["replay_command"]
            .as_str()
            .expect("replay command")
            .contains("--dry-run")
    );
}

#[test]
fn mcp_http_exposes_disk_download_tool_and_downloads_private_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut provider = Server::new();
    let body = b"%PDF-1.4\nprivate pdf\n";
    let output_path = temp.path().join("downloads").join("guide.pdf");

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
            provider.url()
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

    let _metadata = provider
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

    let _ticket = provider
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
            provider.url()
        ))
        .create();

    let _download = provider
        .mock("GET", "/download/private-guide.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(body.as_slice())
        .create();

    let config_dir = temp.path().to_str().expect("utf8 temp path").to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = reqwest::blocking::Client::new();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.disk.download")
    );

    let download = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.download",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/guide.pdf",
                    "output_path": output_path.to_str().expect("utf8 path")
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let download_payload: Value = download.json().expect("download json");
    assert_eq!(
        download_payload["result"]["structuredContent"]["resource"]["resource_type"],
        "file"
    );
    assert_eq!(
        download_payload["result"]["structuredContent"]["download"]["bytes_written"],
        body.len()
    );
    assert_eq!(
        std::fs::read(&output_path).expect("downloaded file"),
        body.as_slice()
    );
}

#[test]
fn mcp_http_exposes_disk_publish_tool_and_returns_public_link() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut provider = Server::new();

    write_mock_disk_account(temp.path(), &provider.url());
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

    let _publish = provider
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

    let _metadata = provider
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

    let config_dir = temp.path().to_str().expect("utf8 temp path").to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = reqwest::blocking::Client::new();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.disk.publish")
    );

    let publish = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.publish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let publish_payload: Value = publish.json().expect("publish json");
    assert_eq!(
        publish_payload["result"]["structuredContent"]["resource"]["public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_http_activity_undo_revokes_public_link_for_reversible_publish() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut provider = Server::new();

    write_mock_disk_account(temp.path(), &provider.url());
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

    let _publish = provider
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

    let _metadata_after_publish = provider
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

    let _unpublish = provider
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

    let _metadata_after_unpublish = provider
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

    let config_dir = temp.path().to_str().expect("utf8 temp path").to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = reqwest::blocking::Client::new();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session id header")
        .to_str()
        .expect("session id")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.activity.undo")
    );

    let publish = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.publish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(publish.status().is_success());

    let activity_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/activity"
            }
        }),
        Some(&session_id),
        None,
    );
    let activity_payload: Value = activity_resource.json().expect("activity resource");
    let activity_contents = activity_payload["result"]["contents"]
        .as_array()
        .expect("activity contents");
    let activity_json: Value = serde_json::from_str(
        activity_contents[0]["text"]
            .as_str()
            .expect("activity text"),
    )
    .expect("activity payload");
    let activity_id = activity_json["items"][0]["id"]
        .as_str()
        .expect("activity id");

    let undo = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "yacli.activity.undo",
                "arguments": {
                    "id": activity_id
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let undo_payload: Value = undo.json().expect("undo payload");
    assert_eq!(
        undo_payload["result"]["structuredContent"]["undo"]["result"]["kind"],
        "disk_unpublish"
    );
    assert_eq!(
        undo_payload["result"]["structuredContent"]["undo"]["result"]["result"]["revoked_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_http_exposes_disk_unpublish_tool_and_revokes_public_link() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut provider = Server::new();

    write_mock_disk_account(temp.path(), &provider.url());
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

    let _metadata_before = provider
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

    let _unpublish = provider
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

    let _metadata_after = provider
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

    let config_dir = temp.path().to_str().expect("utf8 temp path").to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = reqwest::blocking::Client::new();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    let tools_payload: Value = tools_list.json().expect("tools/list json");
    let tools = tools_payload["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "yacli.disk.unpublish")
    );

    let unpublish = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.unpublish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let unpublish_payload: Value = unpublish.json().expect("unpublish json");
    assert_eq!(
        unpublish_payload["result"]["structuredContent"]["result"]["was_public"],
        true
    );
    assert_eq!(
        unpublish_payload["result"]["structuredContent"]["result"]["revoked_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_http_disk_publish_dry_run_reviews_public_link_without_network_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut provider = Server::new();

    write_mock_disk_account(temp.path(), &provider.url());
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

    let _metadata = provider
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

    let config_dir = temp.path().to_str().expect("utf8 temp path").to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = reqwest::blocking::Client::new();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let publish = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.publish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf",
                    "dry_run": true
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let payload: Value = publish.json().expect("publish json");
    assert_eq!(payload["result"]["structuredContent"]["dry_run"], true);
    assert_eq!(
        payload["result"]["structuredContent"]["review"]["current_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_http_disk_unpublish_dry_run_reviews_public_link_without_network_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut provider = Server::new();

    write_mock_disk_account(temp.path(), &provider.url());
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

    let _metadata = provider
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

    let config_dir = temp.path().to_str().expect("utf8 temp path").to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = reqwest::blocking::Client::new();
    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let unpublish = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.unpublish",
                "arguments": {
                    "account": "mock",
                    "path": "disk:/docs/report.pdf",
                    "dry_run": true
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let payload: Value = unpublish.json().expect("unpublish json");
    assert_eq!(payload["result"]["structuredContent"]["dry_run"], true);
    assert_eq!(
        payload["result"]["structuredContent"]["review"]["current_public_url"],
        "https://disk.yandex.ru/i/public-report"
    );
}

#[test]
fn mcp_http_disk_upload_dry_run_reviews_file_without_network_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source_path = temp.path().join("note.txt");
    std::fs::write(&source_path, b"hello disk").expect("source file");

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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let upload = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.upload",
                "arguments": {
                    "account": "mock",
                    "source": source_path.to_str().expect("utf8 path"),
                    "path": "disk:/docs/note.txt",
                    "dry_run": true
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let payload: Value = upload.json().expect("upload json");
    assert_eq!(payload["result"]["structuredContent"]["dry_run"], true);
    assert_eq!(
        payload["result"]["structuredContent"]["upload"]["remote_path"],
        "disk:/docs/note.txt"
    );
    assert_eq!(
        payload["result"]["structuredContent"]["upload"]["bytes_written"],
        10
    );
}

#[test]
fn mcp_http_disk_upload_link_dry_run_reviews_upload_and_publish_without_network_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source_path = temp.path().join("note.txt");
    std::fs::write(&source_path, b"hello disk").expect("source file");

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

    let server = TestHttpServer::spawn_with_envs(&[(
        "YACLI_CONFIG_DIR",
        temp.path().to_str().expect("utf8 path"),
    )]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let upload = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.disk.upload_link",
                "arguments": {
                    "account": "mock",
                    "source": source_path.to_str().expect("utf8 path"),
                    "path": "disk:/docs/note.txt",
                    "dry_run": true
                }
            }
        }),
        Some(&session_id),
        None,
    );
    let payload: Value = upload.json().expect("upload-link json");
    assert_eq!(payload["result"]["structuredContent"]["dry_run"], true);
    assert_eq!(
        payload["result"]["structuredContent"]["review"]["upload"]["remote_path"],
        "disk:/docs/note.txt"
    );
    assert_eq!(
        payload["result"]["structuredContent"]["review"]["publish_path"],
        "disk:/docs/note.txt"
    );
}

#[test]
fn mcp_http_rejects_follow_up_requests_without_session_header() {
    let server = TestHttpServer::spawn();
    let client = client();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        None,
        None,
    );

    assert_eq!(response.status().as_u16(), 400);
    let payload: Value = response.json().expect("error json");
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .expect("error text")
            .contains("Mcp-Session-Id")
    );
}

#[test]
fn mcp_http_rejects_non_local_origins() {
    let server = TestHttpServer::spawn();
    let client = client();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        Some("https://example.com"),
    );

    assert_eq!(response.status().as_u16(), 403);
}

#[test]
fn mcp_http_protected_tools_require_bearer_token_when_configured() {
    let temp = tempfile::tempdir().expect("tempdir");
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
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[
        ("YACLI_CONFIG_DIR", &config_dir),
        ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
        ("YACLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
    ]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let unauthorized = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    let expected_metadata_url = format!(
        "http://{}/.well-known/oauth-protected-resource/mcp",
        server.addr
    );
    assert_eq!(unauthorized.status().as_u16(), 401);
    assert_eq!(
        unauthorized
            .headers()
            .get("www-authenticate")
            .expect("auth challenge")
            .to_str()
            .expect("header text"),
        format!(
            "Bearer error=\"invalid_token\", resource_metadata=\"{}\", scope=\"yacli.auth.read\"",
            expected_metadata_url
        )
    );
    let unauthorized_payload: Value = unauthorized.json().expect("unauthorized json");
    assert_eq!(unauthorized_payload["error"], "invalid_token");
    assert_eq!(
        unauthorized_payload["resource_metadata"],
        expected_metadata_url
    );
    assert_eq!(unauthorized_payload["scope"], "yacli.auth.read");

    let authorized = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .header("Authorization", "Bearer secret-token")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }))
        .send()
        .expect("authorized response");

    assert!(authorized.status().is_success());
    let payload: Value = authorized.json().expect("authorized json");
    assert_eq!(
        payload["result"]["structuredContent"]["account"],
        "personal"
    );
}

#[test]
fn mcp_http_local_bearer_auth_works_without_auth_issuer() {
    let temp = tempfile::tempdir().expect("tempdir");
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
"#,
    );
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[
        ("YACLI_CONFIG_DIR", &config_dir),
        ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
    ]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let unauthorized = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    assert_eq!(unauthorized.status().as_u16(), 401);
    assert_eq!(
        unauthorized
            .headers()
            .get("www-authenticate")
            .expect("auth challenge")
            .to_str()
            .expect("header text"),
        "Bearer error=\"invalid_token\", scope=\"yacli.auth.read\""
    );
    let unauthorized_payload: Value = unauthorized.json().expect("unauthorized json");
    assert_eq!(unauthorized_payload["error"], "invalid_token");
    assert!(unauthorized_payload.get("resource_metadata").is_none());
    assert_eq!(unauthorized_payload["scope"], "yacli.auth.read");

    let metadata = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");
    assert_eq!(metadata.status().as_u16(), 404);

    let authorized = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .header("Authorization", "Bearer secret-token")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }))
        .send()
        .expect("authorized response");

    assert!(authorized.status().is_success());
}

#[test]
fn mcp_http_exposes_protected_resource_metadata_when_auth_is_enabled() {
    let server = TestHttpServer::spawn_with_envs(&[
        ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
        ("YACLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
    ]);
    let client = client();

    let response = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("metadata json");
    assert_eq!(payload["resource"], format!("http://{}/mcp", server.addr));
    assert_eq!(
        payload["authorization_servers"][0],
        "https://auth.example.test"
    );
    assert_eq!(payload["scopes_supported"][0], "yacli.auth.read");
    assert_eq!(payload["bearer_methods_supported"][0], "header");
}

#[test]
fn mcp_http_auth_discovery_uses_public_url_when_provided() {
    let server = TestHttpServer::spawn_with_args_envs(
        &["--public-url", "https://mcp.example.test/mcp"],
        &[
            ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
            ("YACLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
        ],
    );
    let client = client();

    let response = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("metadata json");
    assert_eq!(payload["resource"], "https://mcp.example.test/mcp");
}

#[test]
fn mcp_http_accepts_batch_requests_after_initialize() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let batch = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .json(&json!([
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "ping",
                "params": {}
            },
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/list",
                "params": {}
            }
        ]))
        .send()
        .expect("batch response");

    assert!(batch.status().is_success());
    let payload: Value = batch.json().expect("batch json");
    let responses = payload.as_array().expect("batch array");
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 2);
    assert_eq!(responses[1]["id"], 3);
}

#[test]
fn mcp_http_lists_resource_templates_and_reads_templated_resources() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home tempdir");
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
    let config_dir = temp.path().display().to_string();
    let home_dir = home.path().display().to_string();
    let server =
        TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir), ("HOME", &home_dir)]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let templates = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/templates/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    assert!(templates.status().is_success());
    let templates_payload: Value = templates.json().expect("templates json");
    let resource_templates = templates_payload["result"]["resourceTemplates"]
        .as_array()
        .expect("templates array");
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/account/{account}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/skill/{skill}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/workflow/{workflow}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/activity/{activity}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/home/{account}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/home{?goal}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/home/{account}{?goal}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/next-actions{?goal}")
    );
    assert!(resource_templates.iter().any(
        |template| template["uriTemplate"] == "resource://yacli/next-actions/{account}{?goal}"
    ));
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/suggestions{?goal}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"]
                == "resource://yacli/suggestions/{account}{?goal}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/onboarding{?goal}")
    );
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/doctor{?goal}")
    );

    let account_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/account/personal"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(account_resource.status().is_success());
    let payload: Value = account_resource.json().expect("resource json");
    let contents = payload["result"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents[0]["mimeType"], "application/json");
    let account_payload: Value =
        serde_json::from_str(contents[0]["text"].as_str().expect("resource text"))
            .expect("account payload");
    assert_eq!(account_payload["account"], "personal");
    assert_eq!(account_payload["services"]["calendar"]["enabled"], true);

    let skills_catalog = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/skills"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(skills_catalog.status().is_success());
    let skills_payload: Value = skills_catalog.json().expect("skills json");
    let skills_contents = skills_payload["result"]["contents"]
        .as_array()
        .expect("skills contents");
    let skills_catalog_payload: Value = serde_json::from_str(
        skills_contents[0]["text"]
            .as_str()
            .expect("skills catalog text"),
    )
    .expect("skills catalog payload");
    assert_eq!(skills_catalog_payload["count"], 13);

    let skill_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/skill/yacli-mail"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(skill_resource.status().is_success());
    let skill_payload: Value = skill_resource.json().expect("skill json");
    let skill_contents = skill_payload["result"]["contents"]
        .as_array()
        .expect("skill contents");
    assert_eq!(skill_contents[0]["mimeType"], "text/markdown");
    let skill_text = skill_contents[0]["text"].as_str().expect("skill text");
    assert!(skill_text.contains("# yacli mail"));
    assert!(skill_text.contains("yacli mail send"));

    let workflow_catalog = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/workflows"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(workflow_catalog.status().is_success());
    let workflow_catalog_payload: Value = workflow_catalog.json().expect("workflow catalog json");
    let workflow_catalog_contents = workflow_catalog_payload["result"]["contents"]
        .as_array()
        .expect("workflow catalog contents");
    let workflow_catalog_json: Value = serde_json::from_str(
        workflow_catalog_contents[0]["text"]
            .as_str()
            .expect("workflow catalog text"),
    )
    .expect("workflow catalog payload");
    assert!(
        workflow_catalog_json["workflows"]
            .as_array()
            .expect("workflow items")
            .iter()
            .any(|item| item["id"] == "send-file-by-mail")
    );

    let workflow_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/workflow/attachment-to-disk"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(workflow_resource.status().is_success());
    let workflow_payload: Value = workflow_resource.json().expect("workflow detail json");
    let workflow_contents = workflow_payload["result"]["contents"]
        .as_array()
        .expect("workflow contents");
    let workflow_json: Value = serde_json::from_str(
        workflow_contents[0]["text"]
            .as_str()
            .expect("workflow text"),
    )
    .expect("workflow detail payload");
    assert_eq!(workflow_json["id"], "attachment-to-disk");
    assert_eq!(workflow_json["prompt_name"], "attachment-to-disk");
    assert_eq!(workflow_json["execution"]["state"], "needs_input");
    assert_eq!(
        workflow_json["execution"]["next_action"],
        "connect_services"
    );
    assert_eq!(
        workflow_json["execution"]["available_actions"][0],
        "connect_services"
    );

    let activity_catalog = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/activity"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(activity_catalog.status().is_success());
    let activity_catalog_payload: Value = activity_catalog.json().expect("activity catalog json");
    let activity_catalog_contents = activity_catalog_payload["result"]["contents"]
        .as_array()
        .expect("activity catalog contents");
    let activity_catalog_json: Value = serde_json::from_str(
        activity_catalog_contents[0]["text"]
            .as_str()
            .expect("activity catalog text"),
    )
    .expect("activity catalog payload");
    assert_eq!(activity_catalog_json["count"], 0);

    let onboarding_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/onboarding"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(onboarding_resource.status().is_success());
    let onboarding_resource_payload: Value = onboarding_resource
        .json()
        .expect("onboarding resource json");
    let onboarding_contents = onboarding_resource_payload["result"]["contents"]
        .as_array()
        .expect("onboarding contents");
    let onboarding_json: Value = serde_json::from_str(
        onboarding_contents[0]["text"]
            .as_str()
            .expect("onboarding text"),
    )
    .expect("onboarding payload");
    assert_eq!(onboarding_json["current_account"], "personal");
    assert!(
        onboarding_json["checks"]
            .as_array()
            .expect("onboarding checks")
            .iter()
            .any(|item| item["id"] == "workflow_hub")
    );
    assert!(
        onboarding_json["checks"]
            .as_array()
            .expect("onboarding checks")
            .iter()
            .any(|item| item["id"] == "mcp_install")
    );

    let doctor_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/doctor"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(doctor_resource.status().is_success());
    let doctor_resource_payload: Value = doctor_resource.json().expect("doctor resource json");
    let doctor_contents = doctor_resource_payload["result"]["contents"]
        .as_array()
        .expect("doctor contents");
    let doctor_json: Value =
        serde_json::from_str(doctor_contents[0]["text"].as_str().expect("doctor text"))
            .expect("doctor payload");
    assert_eq!(doctor_json["current_account"], "personal");
    assert!(
        doctor_json["checks"]
            .as_array()
            .expect("doctor checks")
            .iter()
            .any(|item| item["id"] == "secret_backend")
    );
    assert!(
        doctor_json["checks"]
            .as_array()
            .expect("doctor checks")
            .iter()
            .any(|item| item["id"] == "mcp_install")
    );

    let home_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/home"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(home_resource.status().is_success());
    let home_resource_payload: Value = home_resource.json().expect("home resource json");
    let home_contents = home_resource_payload["result"]["contents"]
        .as_array()
        .expect("home contents");
    let home_json: Value =
        serde_json::from_str(home_contents[0]["text"].as_str().expect("home text"))
            .expect("home payload");
    assert_eq!(home_json["current_account"], "personal");
    assert_eq!(home_json["workflow_count"], 8);
    assert_eq!(home_json["onboarding"]["current_account"], "personal");
    assert_eq!(home_json["doctor"]["current_account"], "personal");
    assert_eq!(home_json["suggestions"]["count"], 0);

    let templated_home_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/home/personal"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(templated_home_resource.status().is_success());
    let templated_home_payload: Value = templated_home_resource
        .json()
        .expect("templated home resource json");
    let templated_home_contents = templated_home_payload["result"]["contents"]
        .as_array()
        .expect("templated home contents");
    let templated_home_json: Value = serde_json::from_str(
        templated_home_contents[0]["text"]
            .as_str()
            .expect("templated home text"),
    )
    .expect("templated home payload");
    assert_eq!(templated_home_json["current_account"], "personal");
    assert_eq!(templated_home_json["doctor"]["current_account"], "personal");

    let next_actions_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 13,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/next-actions"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(next_actions_resource.status().is_success());
    let next_actions_resource_payload: Value = next_actions_resource
        .json()
        .expect("next actions resource json");
    let next_actions_contents = next_actions_resource_payload["result"]["contents"]
        .as_array()
        .expect("next actions contents");
    let next_actions_json: Value = serde_json::from_str(
        next_actions_contents[0]["text"]
            .as_str()
            .expect("next actions text"),
    )
    .expect("next actions payload");
    assert!(
        next_actions_json["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .any(|item| item["id"] == "mcp_install")
    );

    let goal_home_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 14,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/home/personal?goal=%D0%BE%D1%82%D0%BF%D1%80%D0%B0%D0%B2%D1%8C%20%D1%84%D0%B0%D0%B9%D0%BB%20%D0%BF%D0%BE%20%D0%BF%D0%BE%D1%87%D1%82%D0%B5"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(goal_home_resource.status().is_success());
    let goal_home_resource_payload: Value =
        goal_home_resource.json().expect("goal home resource json");
    let goal_home_contents = goal_home_resource_payload["result"]["contents"]
        .as_array()
        .expect("goal home contents");
    let goal_home_json: Value = serde_json::from_str(
        goal_home_contents[0]["text"]
            .as_str()
            .expect("goal home text"),
    )
    .expect("goal home payload");
    assert_eq!(goal_home_json["goal"], "отправь файл по почте");
    assert_eq!(
        goal_home_json["goal_route"]["best_match"]["workflow"]["id"],
        "send-file-by-mail"
    );

    let goal_next_actions_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 15,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/next-actions?goal=%D0%BD%D0%B0%D0%B9%D0%B4%D0%B8%20%D0%BF%D1%80%D0%B8%D0%B3%D0%BB%D0%B0%D1%88%D0%B5%D0%BD%D0%B8%D0%B5"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(goal_next_actions_resource.status().is_success());
    let goal_next_actions_resource_payload: Value = goal_next_actions_resource
        .json()
        .expect("goal next actions resource json");
    let goal_next_actions_contents = goal_next_actions_resource_payload["result"]["contents"]
        .as_array()
        .expect("goal next actions contents");
    let goal_next_actions_json: Value = serde_json::from_str(
        goal_next_actions_contents[0]["text"]
            .as_str()
            .expect("goal next actions text"),
    )
    .expect("goal next actions payload");
    assert_eq!(goal_next_actions_json["goal"], "найди приглашение");
    assert_eq!(
        goal_next_actions_json["goal_route"]["best_match"]["workflow"]["id"],
        "invite-to-calendar"
    );
    assert_eq!(goal_next_actions_json["actions"][0]["source"], "goal");

    let suggestions_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 16,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/suggestions"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(suggestions_resource.status().is_success());
    let suggestions_resource_payload: Value = suggestions_resource
        .json()
        .expect("suggestions resource json");
    let suggestions_contents = suggestions_resource_payload["result"]["contents"]
        .as_array()
        .expect("suggestions contents");
    let suggestions_json: Value = serde_json::from_str(
        suggestions_contents[0]["text"]
            .as_str()
            .expect("suggestions text"),
    )
    .expect("suggestions payload");
    assert_eq!(suggestions_json["status"], "idle");
    assert_eq!(suggestions_json["count"], 0);

    let goal_onboarding_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 17,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/onboarding?goal=%D0%BD%D0%B0%D0%B9%D0%B4%D0%B8%20%D0%BF%D1%80%D0%B8%D0%B3%D0%BB%D0%B0%D1%88%D0%B5%D0%BD%D0%B8%D0%B5"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(goal_onboarding_resource.status().is_success());
    let goal_onboarding_resource_payload: Value = goal_onboarding_resource
        .json()
        .expect("goal onboarding resource json");
    let goal_onboarding_contents = goal_onboarding_resource_payload["result"]["contents"]
        .as_array()
        .expect("goal onboarding contents");
    let goal_onboarding_json: Value = serde_json::from_str(
        goal_onboarding_contents[0]["text"]
            .as_str()
            .expect("goal onboarding text"),
    )
    .expect("goal onboarding payload");
    assert_eq!(goal_onboarding_json["goal"], "найди приглашение");
    assert_eq!(
        goal_onboarding_json["goal_route"]["best_match"]["workflow"]["id"],
        "invite-to-calendar"
    );

    let goal_doctor_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 18,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/doctor?goal=%D0%BD%D0%B0%D0%B9%D0%B4%D0%B8%20%D0%BF%D1%80%D0%B8%D0%B3%D0%BB%D0%B0%D1%88%D0%B5%D0%BD%D0%B8%D0%B5"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(goal_doctor_resource.status().is_success());
    let goal_doctor_resource_payload: Value = goal_doctor_resource
        .json()
        .expect("goal doctor resource json");
    let goal_doctor_contents = goal_doctor_resource_payload["result"]["contents"]
        .as_array()
        .expect("goal doctor contents");
    let goal_doctor_json: Value = serde_json::from_str(
        goal_doctor_contents[0]["text"]
            .as_str()
            .expect("goal doctor text"),
    )
    .expect("goal doctor payload");
    assert_eq!(goal_doctor_json["goal"], "найди приглашение");
    assert_eq!(
        goal_doctor_json["goal_route"]["remediation"]["status"],
        "needs_setup"
    );
    assert!(
        goal_doctor_json["focus_checks"]
            .as_array()
            .expect("goal doctor focus checks")
            .iter()
            .any(|item| item["id"] == "mail" || item["id"] == "calendar")
    );
}

#[test]
fn mcp_http_sse_stream_receives_resource_update_notifications() {
    let temp = tempfile::tempdir().expect("tempdir");
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
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let sse = open_sse(&client, &server.url(), &session_id);
    assert!(sse.status().is_success());
    let mut reader = BufReader::new(sse);

    let subscribe = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/subscribe",
            "params": {
                "uri": "resource://yacli/account/personal"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(subscribe.status().is_success());

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

    let notification = read_sse_json_message(&mut reader);
    assert_eq!(notification["method"], "notifications/resources/updated");
    assert_eq!(
        notification["params"]["uri"],
        "resource://yacli/account/personal"
    );
}

#[test]
fn mcp_http_goal_route_tool_matches_invite_workflow_for_russian_goal() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.goal.route",
                "arguments": {
                    "goal": "найди приглашение в письме и добавь встречу в календарь team"
                }
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    assert_eq!(
        payload["result"]["structuredContent"]["best_match"]["workflow"]["id"],
        "invite-to-calendar"
    );
    assert_eq!(
        payload["result"]["structuredContent"]["hints"]["calendar"],
        "team"
    );
    assert_eq!(
        payload["result"]["structuredContent"]["best_match"]["route"]["tool_arguments"]["calendar"],
        "team"
    );
    assert!(payload["result"]["structuredContent"]["remediation"].is_object());
}

#[test]
fn mcp_http_doctor_apply_safe_tool_installs_detected_claude_desktop() {
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let home_dir = tempfile::tempdir().expect("home tempdir");
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
    std::fs::create_dir_all(expected_config.parent().expect("parent")).expect("create parent");

    let server = TestHttpServer::spawn_with_envs(&[
        (
            "YACLI_CONFIG_DIR",
            config_dir.path().to_str().expect("config dir"),
        ),
        ("HOME", home_dir.path().to_str().expect("home dir")),
    ]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("session id")
        .to_string();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.doctor.apply_safe",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(response.status().is_success());
    let payload: Value = response.json().expect("tools/call json");
    assert_eq!(payload["result"]["structuredContent"]["status"], "partial");
    assert!(
        payload["result"]["structuredContent"]["doctor_after"]["mcp_clients"]
            .as_array()
            .expect("mcp clients")
            .iter()
            .any(|item| item["client"] == "claude-desktop" && item["status"] == "installed")
    );
    let activity_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/activity"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(activity_resource.status().is_success());
    let activity_resource_payload: Value = activity_resource.json().expect("activity resource");
    let activity_contents = activity_resource_payload["result"]["contents"]
        .as_array()
        .expect("activity contents");
    let activity_json: Value = serde_json::from_str(
        activity_contents[0]["text"]
            .as_str()
            .expect("activity text"),
    )
    .expect("activity payload");
    assert_eq!(activity_json["items"][0]["operation"], "doctor.apply_safe");
    assert_eq!(activity_json["items"][0]["source"], "mcp");
    assert_eq!(
        activity_json["items"][0]["replay_command"],
        "yacli doctor --apply-safe"
    );
    assert!(expected_config.exists());
}
