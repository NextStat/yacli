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

        for _ in 0..200 {
            if let Some(status) = child.try_wait().expect("poll child status") {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
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
        panic!("HTTP MCP server did not start at {addr}. stderr: {stderr}");
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
    assert_eq!(skills_catalog_payload["count"], 8);

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
